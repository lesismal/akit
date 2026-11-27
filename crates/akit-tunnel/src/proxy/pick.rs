//! Which hub a proxy's tunnel to a node goes through (I109b, fit's `tunnel/proxy/pick.go`): the
//! paths to the node, each through a hub, weighed by their round trips and losses.

use crate::msg::Stats;

/// A way to a node: through a hub, which carries `tunnels` of the proxy's tunnels already.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Path {
    /// The round trip between the proxy and the hub.
    pub hub: Stats,
    /// The round trip between the hub and the node.
    pub node: Stats,
    /// The proxy's tunnels through the hub.
    pub tunnels: usize,
}

/// Added to every path's round trip, so that paths of a few milliseconds, within a host or a LAN,
/// count as about as fast.
const LATENCY_FLOOR: f64 = 10.0;

/// How much a path's cost grows by for each part of its pings lost: a path that loses one ping in
/// ten costs 1.5 times as much.
const LOSS_COST: f64 = 5.0;

/// How slow each path is (fit's `Cost`): its round trip, proxy to hub to node, plus
/// [`LATENCY_FLOOR`], grown by its loss. A link not measured yet counts as the slowest one of its
/// kind that was, or as 0 when none was.
#[must_use]
pub fn cost(paths: &[Path]) -> Vec<f64> {
    let (mut worst_hub, mut worst_node) = (0.0_f64, 0.0_f64);
    for p in paths {
        if p.hub.known() {
            worst_hub = worst_hub.max(p.hub.avg);
        }
        if p.node.known() {
            worst_node = worst_node.max(p.node.avg);
        }
    }
    paths
        .iter()
        .map(|p| {
            let mut rtt = worst_hub + worst_node;
            if p.hub.known() {
                rtt += p.hub.avg - worst_hub;
            }
            if p.node.known() {
                rtt += p.node.avg - worst_node;
            }
            let loss = 1.0 - (1.0 - p.hub.loss) * (1.0 - p.node.loss);
            (rtt + LATENCY_FLOOR) * (1.0 + LOSS_COST * loss)
        })
        .collect()
}

/// The share of the tunnels each path should carry (fit's `Weights`): in inverse proportion to
/// the square of its cost, so that a path twice as slow gets a quarter as many. They add up to 1.
#[must_use]
pub fn weights(paths: &[Path]) -> Vec<f64> {
    let mut out: Vec<f64> = cost(paths).iter().map(|c| 1.0 / (c * c)).collect();
    let sum: f64 = out.iter().sum();
    for w in &mut out {
        *w /= sum;
    }
    out
}

/// The index of the path a new tunnel takes (fit's `Choose`): the one that carries the fewest
/// tunnels for its weight once it has the new one too, which brings the tunnels the paths carry
/// toward their [`weights`]. A tie goes to the faster path, then to the first. 0 for no path.
#[must_use]
pub fn choose(paths: &[Path]) -> usize {
    let (w, cost) = (weights(paths), cost(paths));
    let (mut best, mut best_load) = (0, f64::INFINITY);
    for (i, p) in paths.iter().enumerate() {
        let (Some(wi), Some(ci)) = (w.get(i), cost.get(i)) else {
            break;
        };
        #[allow(clippy::cast_precision_loss)]
        let load = (p.tunnels.saturating_add(1)) as f64 / wi;
        let faster = cost.get(best).is_some_and(|cb| ci < cb);
        if load < best_load * (1.0 - 1e-9) || load <= best_load * (1.0 + 1e-9) && faster {
            best = i;
            best_load = load;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, clippy::arithmetic_side_effects)]
    use super::*;

    fn rtt(avg: f64, loss: f64) -> Stats {
        Stats {
            last: avg,
            avg,
            min: avg,
            loss,
            samples: 1,
        }
    }

    fn path(hub: Stats, node: Stats) -> Path {
        Path {
            hub,
            node,
            tunnels: 0,
        }
    }

    /// Has `n` tunnels choose among `paths`, one after the other: how many each got.
    fn spread(mut paths: Vec<Path>, n: usize) -> Vec<usize> {
        for _ in 0..n {
            let i = choose(&paths);
            paths[i].tunnels += 1;
        }
        paths.iter().map(|p| p.tunnels).collect()
    }

    /// fit's `TestChoose`.
    #[test]
    fn choose_spreads_by_weight() {
        let none = Stats::default();
        for (name, paths, n, want) in [
            (
                "unmeasured paths alternate",
                vec![Path::default(), Path::default()],
                4,
                vec![2, 2],
            ),
            (
                "equal paths alternate",
                vec![path(rtt(5.0, 0.0), rtt(5.0, 0.0)); 2],
                5,
                vec![3, 2],
            ),
            // Costs 20 and 40: weights 0.8 and 0.2.
            (
                "a path twice as slow gets a quarter as many",
                vec![
                    path(rtt(5.0, 0.0), rtt(5.0, 0.0)),
                    path(rtt(10.0, 0.0), rtt(20.0, 0.0)),
                ],
                10,
                vec![8, 2],
            ),
            (
                "the node's leg counts as much as the proxy's",
                vec![
                    path(rtt(30.0, 0.0), rtt(0.0, 0.0)),
                    path(rtt(0.0, 0.0), rtt(30.0, 0.0)),
                ],
                4,
                vec![2, 2],
            ),
            // An unmeasured leg counts as the slowest measured one.
            (
                "an unmeasured leg counts as the slowest",
                vec![
                    path(rtt(10.0, 0.0), none),
                    path(rtt(10.0, 0.0), rtt(30.0, 0.0)),
                    path(rtt(10.0, 0.0), rtt(0.0, 0.0)),
                ],
                18,
                vec![2, 2, 14],
            ),
            // A loss of 0.2 doubles the cost: weights 0.8 and 0.2 again.
            (
                "loss costs",
                vec![path(rtt(10.0, 0.2), none), path(rtt(10.0, 0.0), none)],
                10,
                vec![2, 8],
            ),
            // The tunnels already open count.
            (
                "loaded paths wait",
                vec![
                    Path {
                        tunnels: 3,
                        ..Path::default()
                    },
                    Path::default(),
                ],
                3,
                vec![3, 3],
            ),
        ] {
            let w = weights(&paths);
            assert_eq!(spread(paths, n), want, "{name} (weights {w:?})");
        }
        assert_eq!(choose(&[]), 0);
    }

    /// fit's `TestWeights`.
    #[test]
    fn weights_add_up() {
        let w = weights(&[
            path(rtt(5.0, 0.0), rtt(5.0, 0.0)),
            path(rtt(10.0, 0.0), rtt(20.0, 0.0)),
        ]);
        assert!(
            (w[0] - 0.8).abs() < 1e-9 && (w[1] - 0.2).abs() < 1e-9,
            "{w:?}"
        );
    }
}
