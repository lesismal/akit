//! A listener's services by server name (I52; fit's `l4.TestEntryPick`): exact names, `*.`
//! wildcards one label deep, without case or a trailing dot, and the fallback.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use akit_l4::{Entry, ProxyVersion, Service};
use akit_upstream::{EngineSlot, Pool};

/// The services of `config`, by name.
fn services(config: &akit_config::Config) -> Vec<Service> {
    let pool = Pool::new(&config.upstreams[0], EngineSlot::new());
    config
        .services
        .iter()
        .map(|s| Service::new(s, pool.clone()))
        .collect()
}

fn config() -> akit_config::Config {
    akit_config::from_json(
        r#"{
  "listeners": [{"name": "t", "address": "127.0.0.1:0", "protocols": ["tcp"]}],
  "upstreams": [{"name": "up", "addresses": ["127.0.0.1:1"]}],
  "services": [
    {"name": "a", "listeners": ["t"], "upstream": "up", "server_names": ["A.example.com"], "proxy_protocol": "v2"},
    {"name": "w", "listeners": ["t"], "upstream": "up", "server_names": ["*.w.example.com", "x.example.com"]},
    {"name": "f", "listeners": ["t"], "upstream": "up", "proxy_protocol": "v1"}
  ]
}"#,
        None,
    )
    .unwrap()
}

#[test]
fn pick() {
    let c = config();
    let s = services(&c);
    let e = Entry::of(
        s.iter()
            .cloned()
            .zip(c.services.iter().map(|x| x.server_names.as_slice())),
    );
    assert!(e.routes());
    let name = |got: Option<&Service>| got.map(|s| s.name().to_owned());
    for (n, want) in [
        ("a.example.com", Some("a")),
        ("A.EXAMPLE.COM.", Some("a")),
        ("x.w.example.com", Some("w")),
        ("x.example.com", Some("w")),
        ("w.example.com", None),
        ("y.x.w.example.com", None),
        ("", None),
        ("b.example.com", None),
    ] {
        assert_eq!(name(e.named(n)), want.map(str::to_owned), "named({n:?})");
    }
    assert_eq!(name(e.fallback()), Some("f".to_owned()));
    assert_eq!(name(e.pick(Some("b.example.com"))), Some("f".to_owned()));
    assert_eq!(name(e.pick(None)), Some("f".to_owned()));
    assert_eq!(name(e.pick(Some("x.w.example.com"))), Some("w".to_owned()));
    // Each service once, though w has two names.
    assert_eq!(e.services().len(), 3);
    assert_eq!(e.conns(), 0);
    assert_eq!(s[0].proxy_protocol(), Some(ProxyVersion::V2));
    assert_eq!(s[1].proxy_protocol(), None);
    assert_eq!(s[2].proxy_protocol(), Some(ProxyVersion::V1));

    // Without names: no routing, the one service takes everything; none: nothing does.
    let only = Entry::of([(s[2].clone(), &[][..])]);
    assert!(!only.routes());
    assert_eq!(name(only.pick(Some("a.example.com"))), Some("f".to_owned()));
    assert!(Entry::new().pick(None).is_none());
}
