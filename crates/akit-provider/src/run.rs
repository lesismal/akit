//! Providers, and how their parts reach the control thread: fit's `provider.Provider`,
//! `provider.Run` and its kinds, on akio's model.
//!
//! fit runs each provider on a goroutine and applies their parts on another. akit has no
//! goroutines: a provider watches its source with akio's clients (their callbacks on akio's I/O
//! threads), on akio's logical pool, or on a thread of its own, and hands each part to an
//! [`Update`], which never blocks (a short lock, no I/O). The control thread, which applies every
//! config, takes the parts from [`Updates`] one at a time. As in fit, only a provider's latest
//! part waits: one sent while an earlier one waits replaces it, so a provider that changes faster
//! than configs are applied skips what is already out of date, and what waits is bounded by one
//! part per provider.

use std::fmt;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// A provider: something that watches a source of config and sends what it finds there as a
/// part ([`crate::parse`] reads it).
pub trait Provider: Send {
    /// The provider's name, unique among the providers of a server: the source of what it
    /// provides in the merged config.
    fn name(&self) -> &str;

    /// Starts watching the source, sending the whole of what it provides through `update` each
    /// time that changes. Called on the control thread: what blocks (connecting, reading files)
    /// goes to akio's I/O threads, its logical pool or a thread of the provider's own.
    ///
    /// # Errors
    ///
    /// When the watch cannot be started.
    fn start(&mut self, update: Update) -> io::Result<()>;

    /// Stops watching: no part is sent after it returns. Called on the control thread.
    fn stop(&mut self);
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
struct State {
    /// The providers with a part waiting, in the order they sent it, and that part.
    pending: Vec<(Arc<str>, Vec<u8>)>,
    closed: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Box<dyn Fn() + Send + Sync>,
}

/// The parts waiting for the control thread, at most one per provider.
#[derive(Clone)]
pub struct Updates {
    shared: Arc<Shared>,
}

impl fmt::Debug for Updates {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Updates")
            .field("pending", &self.pending())
            .finish_non_exhaustive()
    }
}

/// What a provider sends its parts through. Cloned, it is the same provider's.
#[derive(Clone)]
pub struct Update {
    shared: Arc<Shared>,
    name: Arc<str>,
}

impl fmt::Debug for Update {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Update")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Update {
    /// The provider's name.
    #[must_use]
    pub fn name(&self) -> &Arc<str> {
        &self.name
    }

    /// Sends `data`, the whole of what the provider provides now. It replaces the provider's part
    /// still waiting, if one is. Never blocks: callable on an akio I/O thread. Once the updates
    /// are closed, the part is dropped.
    pub fn send(&self, data: Vec<u8>) {
        let woke = {
            let mut st = lock(&self.shared.state);
            if st.closed {
                return;
            }
            if let Some(slot) = st.pending.iter_mut().find(|(n, _)| *n == self.name) {
                slot.1 = data;
                false
            } else {
                st.pending.push((Arc::clone(&self.name), data));
                st.pending.len() == 1
            }
        };
        // Once when parts start to wait: the control thread takes them all before it waits again.
        if woke {
            (self.shared.wake)();
        }
    }
}

impl Updates {
    /// Parts waiting for the control thread, `wake` called when the first starts to wait (it
    /// must not block: it may be called on an akio I/O thread). The control thread takes parts
    /// until none waits ([`take`](Updates::take)) after each wake-up, and checks again after
    /// anything else it does, so that a wake-up it could not be given is not lost.
    #[must_use]
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Updates {
        Updates {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                wake: Box::new(wake),
            }),
        }
    }

    /// What the provider `name` sends its parts through.
    #[must_use]
    pub fn update(&self, name: &str) -> Update {
        Update {
            shared: Arc::clone(&self.shared),
            name: Arc::from(name),
        }
    }

    /// The part that has waited longest, and its provider's name; `None` when none waits or the
    /// updates are closed.
    #[must_use]
    pub fn take(&self) -> Option<(Arc<str>, Vec<u8>)> {
        let mut st = lock(&self.shared.state);
        if st.closed || st.pending.is_empty() {
            return None;
        }
        Some(st.pending.remove(0))
    }

    /// How many parts wait.
    #[must_use]
    pub fn pending(&self) -> usize {
        lock(&self.shared.state).pending.len()
    }

    /// Drops the parts waiting, and every part sent from now on: when akit stops.
    pub fn close(&self) {
        let mut st = lock(&self.shared.state);
        st.closed = true;
        st.pending.clear();
    }
}

/// What makes a provider of one kind (`docker`, ...) from its config `C`, with the name it goes
/// by; with the provider, what it watches, for the log.
pub type Make<C> = fn(name: &str, config: &C) -> Result<(Box<dyn Provider>, String), String>;

/// The provider kinds a binary has, by the name a config gives them: fit's `RegisterKind`, made
/// explicitly from each provider crate's list rather than by `init` functions.
pub struct Kinds<C: 'static> {
    kinds: Vec<(&'static str, Make<C>)>,
}

impl<C> fmt::Debug for Kinds<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Kinds")
            .field("kinds", &self.names())
            .finish()
    }
}

impl<C> Kinds<C> {
    /// The kinds of every list in `sets`.
    ///
    /// # Errors
    ///
    /// When a kind is given twice, a mistake in a provider's code.
    pub fn new(sets: &[&[(&'static str, Make<C>)]]) -> Result<Kinds<C>, String> {
        let mut kinds: Vec<(&'static str, Make<C>)> = Vec::new();
        for &(name, make) in sets.iter().flat_map(|s| s.iter()) {
            if kinds.iter().any(|(n, _)| *n == name) {
                return Err(format!("provider: kind {name} is registered twice"));
            }
            kinds.push((name, make));
        }
        kinds.sort_by_key(|(n, _)| *n);
        Ok(Kinds { kinds })
    }

    /// What makes a provider of `kind`, if this binary has it.
    #[must_use]
    pub fn lookup(&self, kind: &str) -> Option<Make<C>> {
        self.kinds.iter().find(|(n, _)| *n == kind).map(|(_, m)| *m)
    }

    /// The kinds, sorted.
    #[must_use]
    pub fn names(&self) -> Vec<&'static str> {
        self.kinds.iter().map(|(n, _)| *n).collect()
    }
}

#[cfg(test)]
mod tests {
    //! fit's `TestRun`, `TestRunCanceledWhileApplying` and `TestKinds`.
    #![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

    use std::collections::HashMap;
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use super::{Kinds, Make, Provider, Update, Updates};

    /// Parts are taken one at a time, in the order their providers first sent them; a
    /// provider's latest part replaces one still waiting; the control thread is woken once per
    /// run of parts.
    #[test]
    fn latest_part_waits() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = Arc::clone(&wakes);
        let ups = Updates::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        });
        let (a, b) = (ups.update("a"), ups.update("b"));
        a.send(b"first".to_vec());
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        let (name, data) = ups.take().unwrap();
        assert_eq!((&*name, data.as_slice()), ("a", &b"first"[..]));
        // While "first" is applied, more arrive.
        for i in 0..100 {
            a.send(format!("a{i}").into_bytes());
            b.send(format!("b{i}").into_bytes());
        }
        b.send(b"fail".to_vec());
        assert_eq!((ups.pending(), wakes.load(Ordering::SeqCst)), (2, 2));
        let mut applied: Vec<(String, String)> = Vec::new();
        while let Some((n, d)) = ups.take() {
            applied.push((n.to_string(), String::from_utf8(d).unwrap()));
        }
        assert_eq!(
            applied,
            [("a".into(), "a99".into()), ("b".into(), "fail".into())]
        );
        assert_eq!(ups.take(), None);
    }

    /// Providers on threads of their own, sending as fast as they can, and a control thread
    /// applying their parts: each provider's last part is applied, and no more parts wait than
    /// there are providers.
    #[test]
    fn concurrent_providers() {
        let (tx, rx) = mpsc::sync_channel::<()>(1);
        let ups = Updates::new(move || {
            let _ = tx.try_send(());
        });
        let senders: Vec<_> = (0..4)
            .map(|p| {
                let u = ups.update(&format!("p{p}"));
                std::thread::spawn(move || {
                    for i in 0..1000 {
                        u.send(format!("{i}").into_bytes());
                    }
                })
            })
            .collect();
        let mut last: HashMap<String, String> = HashMap::new();
        let mut applies = 0;
        loop {
            while let Some((n, d)) = ups.take() {
                applies += 1;
                assert!(ups.pending() <= 4);
                last.insert(n.to_string(), String::from_utf8(d).unwrap());
            }
            if last.len() == 4 && last.values().all(|v| v == "999") {
                break;
            }
            // A wake-up lost to a full channel is made up for by the timeout.
            let _ = rx.recv_timeout(Duration::from_millis(10));
        }
        for s in senders {
            s.join().unwrap();
        }
        assert!(applies <= 4000, "{applies}");
    }

    /// fit's `TestRunCanceledWhileApplying`: once closed, a part waiting is not taken, and one
    /// sent later is dropped.
    #[test]
    fn closed() {
        let ups = Updates::new(|| {});
        let u = ups.update("s");
        u.send(b"1".to_vec());
        assert_eq!(ups.take().unwrap().1, b"1");
        u.send(b"2".to_vec());
        ups.close();
        assert_eq!(ups.take(), None);
        u.send(b"3".to_vec());
        assert_eq!((ups.take(), ups.pending()), (None, 0));
    }

    struct Fake {
        name: String,
        update: Option<Update>,
    }

    impl Provider for Fake {
        fn name(&self) -> &str {
            &self.name
        }

        fn start(&mut self, update: Update) -> io::Result<()> {
            update.send(b"{}".to_vec());
            self.update = Some(update);
            Ok(())
        }

        fn stop(&mut self) {
            self.update = None;
        }
    }

    fn make_fake(
        name: &str,
        watching: &&'static str,
    ) -> Result<(Box<dyn Provider>, String), String> {
        Ok((
            Box::new(Fake {
                name: name.to_owned(),
                update: None,
            }),
            (*watching).to_owned(),
        ))
    }

    /// fit's `TestKinds`: a kind made known is found and makes its providers; one given twice is
    /// refused.
    #[test]
    fn kinds() {
        const ONE: &[(&str, Make<&str>)] = &[("test_kind", make_fake)];
        const TWO: &[(&str, Make<&str>)] = &[("b_kind", make_fake), ("a_kind", make_fake)];
        let kinds = Kinds::new(&[ONE, TWO]).unwrap();
        let make = kinds.lookup("test_kind").unwrap();
        let (mut p, watching) = make("p", &"watching nothing").unwrap();
        assert_eq!((p.name(), watching.as_str()), ("p", "watching nothing"));
        let ups = Updates::new(|| {});
        p.start(ups.update(p.name())).unwrap();
        assert_eq!(&*ups.take().unwrap().0, "p");
        p.stop();
        assert!(kinds.lookup("no_such_kind").is_none());
        assert_eq!(kinds.names(), ["a_kind", "b_kind", "test_kind"]);
        let err = Kinds::new(&[ONE, ONE]).unwrap_err();
        assert!(err.contains("kind test_kind is registered twice"), "{err}");
    }
}
