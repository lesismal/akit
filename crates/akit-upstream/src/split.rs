//! Requests spread over several upstreams by weight, with a failover (I46): fit's proxy
//! `pickPool`.

use crate::Pool;
use crate::balance::{below_u64, next_random};

/// The upstreams a proxy's requests go to: one, or several chosen at random by weight (fit's
/// `upstreams`, as a canary release spreads them), and a failover that takes the requests when
/// none of the chosen one's addresses is in rotation and one of its own is.
#[derive(Clone, Debug)]
pub struct Split {
    /// The first upstream, the one when there is one.
    first: Pool,
    /// Each upstream after the first, with the sum of the weights up to it and its own.
    rest: Box<[(Pool, u64)]>,
    /// The first's weight.
    first_weight: u64,
    failover: Option<Pool>,
}

impl Split {
    /// The upstreams of `pools`, each with its weight (a weight of 0 counts as 1), and the
    /// `failover`; `None` when `pools` is empty.
    #[must_use]
    pub fn new(
        pools: impl IntoIterator<Item = (Pool, u32)>,
        failover: Option<Pool>,
    ) -> Option<Split> {
        let mut pools = pools.into_iter();
        let (first, w) = pools.next()?;
        let first_weight = u64::from(w.max(1));
        let mut total = first_weight;
        let rest = pools
            .map(|(p, w)| {
                total = total.saturating_add(u64::from(w.max(1)));
                (p, total)
            })
            .collect();
        Some(Split {
            first,
            rest,
            first_weight,
            failover,
        })
    }

    /// One upstream, without a failover.
    #[must_use]
    pub fn one(pool: Pool) -> Split {
        Split {
            first: pool,
            rest: Box::new([]),
            first_weight: 1,
            failover: None,
        }
    }

    /// The upstream a request goes to: one of them by weight, at random, or the one; then the
    /// failover instead, if none of the chosen one's addresses is in rotation (for failing, or
    /// for its health checks) and one of the failover's is. Never blocks or allocates.
    #[must_use]
    pub fn pick(&self) -> &Pool {
        let pool = self.weighted();
        match &self.failover {
            Some(f) if !pool.available() && f.available() => f,
            _ => pool,
        }
    }

    /// The upstream by weight, before the failover is considered.
    fn weighted(&self) -> &Pool {
        let Some((_, total)) = self.rest.last() else {
            return &self.first;
        };
        let n = below_u64(*total);
        if n < self.first_weight {
            return &self.first;
        }
        self.rest
            .iter()
            .find(|(_, up_to)| n < *up_to)
            .map_or(&self.first, |(p, _)| p)
    }

    /// The upstreams, in the order given, and the failover.
    pub fn pools(&self) -> impl Iterator<Item = &Pool> + '_ {
        std::iter::once(&self.first)
            .chain(self.rest.iter().map(|(p, _)| p))
            .chain(&self.failover)
    }
}

/// Whether to act on a share of `percent` (0 to 100) of the cases, at random: always at 100 or
/// more, never at 0 or less (fit's mirror `percent`).
#[must_use]
pub fn chance(percent: f64) -> bool {
    if percent >= 100.0 {
        return true;
    }
    // 53 random bits, as a number in [0, 1).
    #[allow(clippy::cast_precision_loss)]
    let r = (next_random() >> 11) as f64 / (1u64 << 53) as f64;
    r * 100.0 < percent
}
