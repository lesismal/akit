//! Fuzz entry points (feature `fuzzing`; not a public API): the `fuzz/` cargo-fuzz project calls
//! them, and the tests below replay `fuzz/corpus/<target>` on stable. What the providers read
//! from Docker, Kubernetes, Consul and etcd: whatever those send, a provider writes a part the
//! merge reads, and the merge of it onto a config is a config akit reads without a panic.
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use akit_config::value;
use akit_config::{DockerEndpoint, DockerProvider, ProviderKind};

use crate::consul::Instance;
use crate::kvpart::{self, Entry};
use crate::{docker, merge};

/// The base config parts are merged onto.
const BASE: &str = r#"{"listeners": [{"name": "web", "address": ":8080"}],
 "upstreams": [{"name": "up", "addresses": ["127.0.0.1:9000"]}],
 "routes": [{"name": "r", "listeners": ["web"], "upstream": "up"}]}"#;

/// Reads `part`, what provider `name` wrote, merges it onto [`BASE`] and reads the result.
fn merged(name: &str, part: &[u8]) {
    let p = match merge::parse(name, part) {
        Ok(p) => p,
        Err(e) => panic!(
            "{name}'s part does not read: {e}\n{}",
            String::from_utf8_lossy(part)
        ),
    };
    let base: Arc<str> = BASE.into();
    if let Ok(m) = merge(&base, &Arc::from("file"), &[Arc::new(p)]) {
        let _ = akit_config::from_json(m.data(), None);
    }
}

/// `provider_kubernetes`: see [`crate::kubernetes::fuzz`].
pub fn provider_kubernetes(data: &[u8]) {
    crate::kubernetes::fuzz::kubernetes(data);
}

/// `provider_docker`: a Docker daemon's container listing and event stream (I58). `data[0]`
/// picks the options; up to a NUL, the listing; after it, event lines.
pub fn provider_docker(data: &[u8]) {
    let Some((&flags, rest)) = data.split_first() else {
        return;
    };
    let (list, events) = match rest.iter().position(|&b| b == 0) {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, &[][..]),
    };
    for line in events.split(|&b| b == b'\n') {
        let _ = docker::event_changes(line);
    }
    let Ok(cs) = docker::parse_list(list) else {
        return;
    };
    let opts = DockerProvider {
        endpoint: DockerEndpoint::Unix(PathBuf::from("/var/run/docker.sock")),
        expose_all: flags & 1 != 0,
        network: if flags & 2 != 0 {
            "front".to_owned()
        } else {
            String::new()
        },
        use_published_ports: flags & 4 != 0,
        label_prefix: "fit".to_owned(),
        max_containers: usize::from(flags >> 3).max(1),
        refresh_interval: Duration::from_secs(30),
        request_timeout: Duration::from_secs(10),
    };
    let (part, _) = docker::translate(cs, &opts);
    merged("docker", &part);
}

/// `provider_consul`: a Consul catalog's health answers (I61): a JSON object of services, each
/// an array of `/v1/health/service` entries. `data[0]` picks the options.
pub fn provider_consul(data: &[u8]) {
    let Some((&flags, rest)) = data.split_first() else {
        return;
    };
    let Some(v) = std::str::from_utf8(rest)
        .ok()
        .and_then(|t| value::parse(t).ok())
    else {
        return;
    };
    let value::Value::Object(services) = v else {
        return;
    };
    let mut instances: BTreeMap<String, Vec<Instance>> = BTreeMap::new();
    for (name, entries) in services {
        let es = match entries {
            value::Value::Array(a) => a.iter().map(Instance::from_json).collect(),
            _ => Vec::new(),
        };
        instances.insert(name, es);
    }
    let config = akit_config::from_json(
        if flags & 1 != 0 {
            r#"{"listeners": [{"name": "web", "address": ":80"}], "providers": [{"consul": {"expose_all": true}}]}"#
        } else {
            r#"{"listeners": [{"name": "web", "address": ":80"}], "providers": [{"consul": {}}]}"#
        },
        None,
    )
    .expect("the consul provider's config");
    let Some(ProviderKind::Consul(opts)) = config.providers.first().map(|p| &p.kind) else {
        panic!("no consul provider");
    };
    let (part, _) = crate::consul::translate(instances, opts);
    merged("consul", &part);
}

/// `provider_kv`: the keys of Consul's KV store or etcd (I61), each holding a part: NUL-separated
/// keys and values; and, after a `\x01`, the lines of an etcd watch.
pub fn provider_kv(data: &[u8]) {
    let Some((&max, rest)) = data.split_first() else {
        return;
    };
    let (kv, watch) = match rest.iter().position(|&b| b == 1) {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, &[][..]),
    };
    let (mut started, mut ready, mut changed) = (false, 0, 0);
    for line in watch.split(|&b| b == b'\n') {
        if crate::etcd::watch_message(line, &mut started, &mut || ready += 1, &mut || changed += 1)
            .is_err()
        {
            break;
        }
    }
    assert!(ready <= 1, "ready {ready} times");
    let mut it = kv.split(|&b| b == 0);
    let mut entries = Vec::new();
    while let (Some(k), Some(v)) = (it.next(), it.next()) {
        entries.push(Entry {
            key: String::from_utf8_lossy(k).into_owned(),
            value: v.to_vec(),
        });
    }
    let (part, _) = kvpart::part(entries, usize::from(max).max(1));
    merged("kv", &part);
}

/// `provider_merge`: a part of any bytes reads, or not, without a panic; parts that read merge
/// onto the base config, or not, and what merges is a config akit reads without a panic.
pub fn provider_merge(data: &[u8]) {
    let parts: Vec<Arc<merge::Part>> = data
        .split(|&b| b == 0)
        .take(4)
        .filter_map(|p| merge::parse("p", p).ok())
        .map(Arc::new)
        .collect();
    let base: Arc<str> = BASE.into();
    if let Ok(m) = merge(&base, &Arc::from("file"), &parts) {
        let _ = akit_config::from_json(m.data(), None);
        let _ = m.sources();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `f` on every file of `fuzz/corpus/<target>`.
    fn replay(target: &str, f: fn(&[u8])) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fuzz/corpus")
            .join(target);
        let mut n = 0;
        for e in std::fs::read_dir(&dir).unwrap() {
            f(&std::fs::read(e.unwrap().path()).unwrap());
            n += 1;
        }
        assert!(n > 0, "{} is empty", dir.display());
    }

    #[test]
    fn corpus_provider_kubernetes() {
        replay("provider_kubernetes", provider_kubernetes);
    }

    #[test]
    fn corpus_provider_docker() {
        replay("provider_docker", provider_docker);
    }

    #[test]
    fn corpus_provider_consul() {
        replay("provider_consul", provider_consul);
    }

    #[test]
    fn corpus_provider_kv() {
        replay("provider_kv", provider_kv);
    }

    #[test]
    fn corpus_provider_merge() {
        replay("provider_merge", provider_merge);
    }
}
