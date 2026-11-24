# Decisions waiting for review

[English](decisions-pending.md) | [简体中文](decisions-pending.zh-CN.md)

Decisions taken while the maintainer was away, each implemented as recommended and waiting for a
yes or a change. A decision confirmed moves to the [decision notes](decisions/README.md) when it
is one, or is struck from here; one changed is redone.

| # | Date | Where | Decision taken | The other choice |
| --- | --- | --- | --- | --- |
| 1 | 2026-11-24 | akio, merging akit's requests | The five akio branches (A1/A3, A2, A4/A5, A7, A8) merged into akio's main; akio's main had used ADR-093 and ADR-094 meanwhile, so A2's ADR became ADR-098 (`098-tls-cert-resolver`) and A1/A3's ADR-099 (`099-runtime-listeners-accept-filter`), every reference with them | keep the branches' numbers and renumber main's (main's were pushed first) |
| 2 | 2026-11-24 | akio, merging A8 | A8 and main's ICMP work both declared the macOS `ioctl` binding; A8's copy dropped. A8's `probe_gone` allows `needless_pass_by_ref_mut` off Linux, where it only reads | change the signature per platform |
| 3 | 2026-11-24 | akit, akio API requests | I09 and I14 both numbered their first request A9: I09's (a TLS server capped at TLS 1.2) kept A9, I14's became A10–A12 | — |
| 4 | 2026-11-24 | akit, I22 with I10, then on A1 | Merging I22 with I10 refused a reload adding an h3 listener and fixed `Listeners` to read addresses through I10's `addrs`. On akio's A1 now: a reload adds an h3 listener (TCP with `add_listener`, HTTP/3 with `bind_udp` on the engine's UDP threads, one such thread added to every engine that has no h3 listener at startup, and closed without a drain when a later reload removes it), and changes or removes the TCP listeners akit started with; it refuses only to remove or change a startup listener serving h3, whose UDP listener akio cannot remove (requested as A22) | keep the refusal of h3 on reload until akio removes UDP listeners too, saving the extra UDP thread |
| 5 | 2026-11-24 | akit, I15 | I15 stays `doing`: its three `Host` tests wait for akio's A11, written on akio branch `claude/i15-proxy-699f18`; the next group merges it into akio and moves akit's pin | rewrite `Host` in akit (rule 21 says what akio lacks goes into akio) |
| 6 | 2026-11-24 | akit, I13 | akio-http's `Files` cannot try candidate paths or list a directory: I13 supports `precompressed` now and refuses `try_files` and `browse` as not supported yet (akio requests A16–A18), staying `doing`; the I13 session writes A13 and A16–A18 into akio on a branch, merged with A11 when akit's akio pin moves, and I13's akit side is finished then | build `try_files` and listing in akit around `Files` (rule 21 says what akio lacks goes into akio) |
| 7 | 2026-11-24 | akit, merging I16 | I13 and I16 both numbered their akio requests A16–A18. I13's kept A16–A18, since its akio branch (ADR-100) already uses them. I16's became A19–A21. I16 merged while still `doing`, because its `limits.max_body_bytes` is done and tested; its streaming waits for A19–A21 | renumber I13's instead, along with its akio branch |
