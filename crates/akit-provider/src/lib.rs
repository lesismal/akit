#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's config providers: the framework (I57), and Docker, Kubernetes, Consul and etcd on it
//! (I58–I61).
//!
//! A provider watches a source of config other than the files and the Admin API and sends what
//! it finds there as a [`Part`]: a JSON object with `listeners`, `routes`, `upstreams` and
//! `services`, and nothing else. Parts are merged into the base config, the one the files and
//! the Admin API write, by fit's rules (fit's decision 009, [`merge`]): the base config wins over
//! every provider, and a provider over those after it; the merged config is checked, compiled and
//! applied as a whole, through the path every config takes (`akit_admin::Configs`), and a part
//! that would leave it invalid is refused, the provider's part before it staying. Where each
//! element of the merged config came from is recorded, for the Admin API's
//! `GET /config/effective`.
//!
//! Providers run in the control plane, never blocking an akio I/O thread: their parts reach the
//! control thread through [`Updates`], at most one waiting per provider.
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations
//! that fill this crate in.

pub mod docker;
pub mod kubernetes;
mod labels;
mod merge;
mod run;

pub use merge::{KINDS, MAX_PART_BYTES, Merged, Part, Shadowed, Source, merge, parse};
pub use run::{Kinds, Make, Provider, Update, Updates};
