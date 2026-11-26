#![allow(clippy::unwrap_used, clippy::panic, clippy::arithmetic_side_effects)]
//! fit's `login` tests (`login_test.go`, `admit_test.go`, `backoff_test.go`), ported, with a fake
//! clock.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::*;

struct Clock(Arc<AtomicU64>);

impl Clock {
    fn add(&self, d: Duration) {
        self.0
            .fetch_add(u64::try_from(d.as_nanos()).unwrap(), Ordering::SeqCst);
    }
}

fn hash(pw: &str) -> String {
    bcrypt::hash(pw.as_bytes(), bcrypt::MIN_COST).unwrap()
}

fn logins_with(users: &[(&str, &str)], max_sessions: usize) -> (Arc<Logins>, Clock) {
    let now = Arc::new(AtomicU64::new(
        u64::try_from(ORIGIN_OFFSET.as_nanos()).unwrap(),
    ));
    let read = Arc::clone(&now);
    let o = Options {
        users: users
            .iter()
            .map(|(u, p)| ((*u).to_owned(), hash(p)))
            .collect(),
        idle: Duration::from_secs(60),
        lifetime: Duration::from_secs(3600),
        max_sessions,
    };
    let l = Logins::with_clock(
        &o,
        Box::new(move || Duration::from_nanos(read.load(Ordering::SeqCst))),
    );
    (Arc::new(l), Clock(now))
}

fn logins() -> (Arc<Logins>, Clock) {
    logins_with(&[("admin", "secret")], 2)
}

#[test]
fn check() {
    let (l, _) = logins();
    for (user, pw, ok) in [
        ("admin", "secret", true),
        ("admin", "wrong", false),
        ("nobody", "secret", false),
        ("", "", false),
    ] {
        assert_eq!(l.check(user, pw), ok, "{user:?} {pw:?}");
    }
    // At most CHECKS at once: many threads checking all finish.
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let l = Arc::clone(&l);
            std::thread::spawn(move || l.check("admin", "secret"))
        })
        .collect();
    for h in handles {
        assert!(h.join().unwrap());
    }
}

#[test]
fn sessions() {
    let (l, now) = logins();
    let (token, s) = l.start("admin").unwrap();
    let got = l.lookup(&token).unwrap();
    assert_eq!(got.user, "admin");
    assert!(csrf_ok(&got, &s.csrf));
    assert!(!csrf_ok(&got, "x"));
    assert!(!csrf_ok(&got, ""));
    assert_eq!(l.lookup(""), None);
    // The token is 32 bytes, the CSRF token 24, both base64url.
    assert_eq!((token.len(), s.csrf.len()), (43, 32));
    // Idle too long.
    now.add(Duration::from_secs(120));
    assert_eq!(l.lookup(&token), None, "an idle session lives");
    // Too many: the oldest goes.
    let (a, _) = l.start("admin").unwrap();
    now.add(Duration::from_secs(1));
    let (b, _) = l.start("admin").unwrap();
    now.add(Duration::from_secs(1));
    let (c, _) = l.start("admin").unwrap();
    assert_eq!(l.lookup(&a), None, "the oldest lives past max_sessions");
    assert!(l.lookup(&b).is_some() && l.lookup(&c).is_some());
    assert!(l.end(&b).is_some());
    assert_eq!(l.lookup(&b), None, "an ended session lives");
    // Past its lifetime, however busy.
    for _ in 0..70 {
        now.add(Duration::from_secs(59));
        let _ = l.lookup(&c);
    }
    assert_eq!(l.lookup(&c), None, "a session lives past its lifetime");
}

#[test]
fn lockout() {
    let (l, now) = logins();
    for _ in 0..MAX_FAILURES - 1 {
        l.failed("10.0.0.1");
    }
    assert!(l.locked_out("10.0.0.1").is_zero(), "locked out early");
    l.failed("10.0.0.1");
    assert!(!l.locked_out("10.0.0.1").is_zero());
    assert!(l.locked_out("10.0.0.2").is_zero());
    now.add(LOCKOUT + Duration::from_secs(1));
    assert!(l.locked_out("10.0.0.1").is_zero(), "still locked out");
    for i in 0..MAX_ADDRESSES + 10 {
        l.failed(&format!("x{i}"));
    }
    assert!(l.addresses() <= MAX_ADDRESSES);
}

#[test]
fn cookie_names() {
    assert_eq!(cookie_name("akit_console", 2020), "akit_console_2020");
    assert_eq!(cookie_name("akit_console", 0), "akit_console");
}

#[test]
fn users_and_passwords() {
    let (l, _) = logins();
    assert_eq!(l.users(), ["admin"]);
    assert!(l.has_user("admin") && !l.has_user("nobody") && !l.has_user(""));
    l.set_password("admin", &hash("new"));
    assert!(l.check("admin", "new"));
    assert!(!l.check("admin", "secret"), "the old password still checks");
    l.set_password("bob", &hash("bob"));
    assert_eq!(l.users(), ["admin", "bob"]);
    assert!(l.check("bob", "bob"));
}

#[test]
fn end_user_sessions() {
    let (l, _) = logins_with(&[("admin", "pw"), ("bob", "pw")], 8);
    let (mine, _) = l.start("admin").unwrap();
    let (other, _) = l.start("admin").unwrap();
    let (third, _) = l.start("admin").unwrap();
    let (bobs, _) = l.start("bob").unwrap();
    assert_eq!(l.end_user_sessions("admin", &mine), 2);
    for (name, token, live) in [
        ("the caller's", &mine, true),
        ("another", &other, false),
        ("a third", &third, false),
        ("another user's", &bobs, true),
    ] {
        assert_eq!(l.lookup(token).is_some(), live, "{name}");
    }
    assert_eq!(l.end_user_sessions("admin", ""), 1);
    assert_eq!(l.end_user_sessions("nobody", ""), 0);
    assert!(l.lookup(&bobs).is_some());
}

#[test]
fn keys() {
    for (addr, want) in [
        ("10.0.0.1", "10.0.0.1"),
        ("::ffff:10.0.0.1", "10.0.0.1"),
        ("2001:db8:1:2:3:4:5:6", "2001:db8:1:2::/64"),
        ("2001:db8:1:2:ffff::1", "2001:db8:1:2::/64"),
        ("fe80::1%eth0", "fe80::/64"),
        ("@", "@"),
        ("", ""),
    ] {
        assert_eq!(key(addr), want, "{addr}");
    }
}

#[test]
fn admit_one_at_a_time() {
    let (l, _) = logins();
    let a = l.admit("2001:db8::1").unwrap();
    for addr in ["2001:db8::1", "2001:db8::ffff:1"] {
        let r = l.admit(addr).unwrap_err();
        assert!(r.busy && !r.wait.is_zero(), "{addr}");
    }
    l.admit("2001:db8:0:1::1").unwrap().done(false);
    a.done(true);
    l.admit("2001:db8::2").unwrap().done(false);
    assert_eq!(l.addresses(), 1, "the one that failed is kept");
}

#[test]
fn admit_attack() {
    let (l, _) = logins();
    let mut checked = 0;
    for i in 0..100 {
        let held: Vec<Ticket> = (0..10)
            .filter_map(|j| l.admit(&format!("2001:db8::{:x}", i * 10 + j + 1)).ok())
            .collect();
        for t in held {
            checked += 1;
            assert!(t.locked_out().is_zero(), "admitted while locked out");
            t.done(true);
        }
    }
    assert_eq!(checked, MAX_FAILURES);
    assert!(!l.locked_out("2001:db8::9999").is_zero());
    let r = l.admit("2001:db8::1").unwrap_err();
    assert!(!r.busy && !r.wait.is_zero());
}

fn ip(prefix: &str, i: usize) -> String {
    format!("{prefix}.{}.{}", i / 256, i % 256)
}

#[test]
fn admit_full_table() {
    let (l, _) = logins();
    for _ in 0..MAX_FAILURES {
        l.failed("10.9.9.9");
    }
    for i in 0..MAX_ADDRESSES + 10 {
        l.failed(&ip("10.0", i));
    }
    assert!(l.addresses() <= MAX_ADDRESSES);
    assert!(
        !l.locked_out("10.9.9.9").is_zero(),
        "eviction ended a lockout"
    );

    // Every address locked out: a new one is refused, a while.
    let (l2, now2) = logins();
    for i in 0..MAX_ADDRESSES {
        for _ in 0..MAX_FAILURES {
            l2.failed(&ip("10.1", i));
        }
    }
    let r = l2.admit("10.2.0.1").unwrap_err();
    assert!(!r.busy && !r.wait.is_zero() && r.wait <= LOCKOUT, "{r:?}");
    assert!(
        l2.admit("10.2.0.2").is_err(),
        "a second new address admitted"
    );
    now2.add(LOCKOUT + Duration::from_secs(1));
    l2.admit("10.2.0.1").unwrap().done(false);

    // Addresses being checked are not evicted either.
    let (l3, _) = logins();
    let mut held: Vec<Ticket> = (0..MAX_ADDRESSES)
        .map(|i| l3.admit(&ip("10.3", i)).unwrap())
        .collect();
    let r = l3.admit("10.4.0.1").unwrap_err();
    assert!(!r.wait.is_zero());
    held.remove(0).done(false);
    assert!(l3.admit("10.4.0.1").is_ok(), "refused once a check ended");
}

fn fail(l: &Arc<Logins>, addr: &str, user: &str) {
    l.admit_user(addr, user)
        .unwrap_or_else(|r| panic!("{user} from {addr} refused: {r:?}"))
        .done(true);
}

fn spread(i: u32) -> String {
    format!("10.0.1.{}", i % 3 + 1)
}

#[test]
fn user_backoff() {
    let (l, now) = logins();
    for i in 0..USER_FAILURES - 1 {
        fail(&l, &spread(i), "admin");
    }
    l.admit_user("10.0.2.1", "admin").unwrap().done(true); // the tenth
    let r = l.admit_user("10.0.2.2", "admin").unwrap_err();
    assert!(r.slowed && !r.busy && r.wait == USER_BACKOFF, "{r:?}");
    l.admit_user("10.0.2.2", "nobody").unwrap().done(false);

    now.add(USER_BACKOFF);
    fail(&l, "10.0.3.1", "admin");
    let r = l.admit_user("10.0.3.2", "admin").unwrap_err();
    assert!(r.slowed && r.wait == 2 * USER_BACKOFF, "{r:?}");
    for i in 0..20 {
        now.add(MAX_USER_BACKOFF);
        fail(&l, &format!("10.0.4.{}", i / 4 + 1), "admin");
    }
    let r = l.admit_user("10.0.5.1", "admin").unwrap_err();
    assert!(r.slowed && r.wait == MAX_USER_BACKOFF, "the cap: {r:?}");

    now.add(MAX_USER_BACKOFF);
    let queued = l.admit_user("10.0.6.1", "admin").unwrap();
    assert!(queued.locked_out().is_zero());
    fail(&l, "10.0.6.2", "admin");
    assert!(
        !queued.locked_out().is_zero(),
        "a queued check sees the user slowed"
    );
    queued.done(false);
    assert!(
        l.admit_user("10.0.6.3", "admin").unwrap_err().slowed,
        "a check that did not run ended the count"
    );

    now.add(MAX_USER_BACKOFF);
    let t = l.admit_user("10.0.7.1", "admin").unwrap();
    assert!(l.check("admin", "secret"));
    t.passed();
    fail(&l, "10.0.7.2", "admin");
    l.admit_user("10.0.7.3", "admin").unwrap().done(false);
}

#[test]
fn user_backoff_quiet() {
    let (l, now) = logins();
    for i in 0..USER_FAILURES - 1 {
        fail(&l, &spread(i), "admin");
    }
    now.add(WINDOW + Duration::from_secs(1));
    fail(&l, "10.0.2.1", "admin");
    l.admit_user("10.0.2.2", "admin").unwrap().done(false);
    for i in 0..USER_FAILURES {
        fail(&l, &spread(i), &format!("guess{i}"));
    }
    assert!(
        l.admit_user("10.0.2.3", "yet-another").unwrap_err().slowed,
        "names of no user are counted together"
    );
    l.admit_user("10.0.2.3", "admin").unwrap().done(false);
}

#[test]
fn address_backoff() {
    let (l, now) = logins();
    let addr = "10.0.0.9";
    let mut want = LOCKOUT;
    for round in 0..5 {
        for _ in 0..MAX_FAILURES {
            l.failed(addr);
        }
        assert_eq!(l.locked_out(addr), want, "lockout {}", round + 1);
        now.add(want);
        l.admit(addr).unwrap().done(false);
        want = (2 * want).min(MAX_LOCKOUT);
    }
    now.add(WINDOW + Duration::from_secs(1));
    for _ in 0..MAX_FAILURES {
        l.failed(addr);
    }
    assert_eq!(l.locked_out(addr), LOCKOUT, "after a quiet window");
}

#[test]
fn admit_forgets_quiet() {
    let (l, now) = logins();
    for i in 0..MAX_ADDRESSES {
        l.failed(&ip("10.5", i));
    }
    now.add(WINDOW + Duration::from_secs(1));
    l.admit("10.6.0.1").unwrap().done(false);
    assert_eq!(l.addresses(), 0);
}

#[test]
fn a_dropped_ticket_counts_nothing() {
    let (l, _) = logins();
    drop(l.admit_user("10.0.0.1", "admin").unwrap());
    assert_eq!(l.addresses(), 0);
    l.admit("10.0.0.1").unwrap().done(false);
}
