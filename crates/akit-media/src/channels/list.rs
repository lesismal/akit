//! Where a channel is in its list (fit's `listState`), and the order a shuffled list plays in.

use super::source::{Choice, FROM_LIST};

/// Where a channel is in its list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ListState {
    /// The list `order` and `pos` are of.
    items: Vec<String>,
    /// `order` is shuffled.
    shuffled: bool,
    /// Indexes of `items`, in the order they play.
    pub(crate) order: Vec<usize>,
    /// The next in `order`.
    pub(crate) pos: usize,
    /// The last taken.
    last: String,
    /// `last` was taken, and not given back.
    taken: bool,
}

impl ListState {
    /// The next item of `items`, and `None` when there is none: an empty list, or one played
    /// out without `looped`. A list changed by a reload goes on after the item taken last, when
    /// it still has it.
    pub(crate) fn next(
        &mut self,
        items: &[String],
        shuffle: bool,
        looped: bool,
        rng: &mut dyn FnMut(&mut [usize]),
    ) -> Option<String> {
        if self.items != items || self.shuffled != shuffle || self.order.len() != items.len() {
            self.reset(items, shuffle, rng);
        }
        if self.items.is_empty() {
            return None;
        }
        if self.pos >= self.order.len() {
            if !looped {
                return None;
            }
            self.pos = 0;
            if shuffle {
                rng(&mut self.order);
                let n = self.order.len();
                let first = self.order.first().and_then(|&i| self.items.get(i));
                if n > 1 && first == Some(&self.last) {
                    // Not the same item twice in a row, across a round.
                    self.order.swap(0, n.saturating_sub(1));
                }
            }
        }
        let id = self
            .order
            .get(self.pos)
            .and_then(|&i| self.items.get(i))?
            .clone();
        self.last.clone_from(&id);
        self.taken = true;
        self.pos = self.pos.saturating_add(1);
        Some(id)
    }

    /// Makes the order from `items`, going on after the item taken last when `items` has it and
    /// is not shuffled.
    fn reset(&mut self, items: &[String], shuffle: bool, rng: &mut dyn FnMut(&mut [usize])) {
        self.items = items.to_vec();
        self.shuffled = shuffle;
        self.order = (0..items.len()).collect();
        self.pos = 0;
        if shuffle {
            rng(&mut self.order);
            return;
        }
        if !self.last.is_empty() {
            if let Some(i) = items.iter().position(|x| *x == self.last) {
                self.pos = i.saturating_add(1);
            }
        }
    }

    /// Gives back the item `ch` took from the list, to be taken again.
    pub(crate) fn undo(&mut self, ch: &Choice) {
        if ch.from != FROM_LIST || !self.taken || self.pos == 0 || self.pos > self.order.len() {
            return;
        }
        let at = self
            .order
            .get(self.pos.saturating_sub(1))
            .and_then(|&i| self.items.get(i));
        if at == Some(&ch.item) {
            self.pos = self.pos.saturating_sub(1);
            self.taken = false;
        }
    }
}

/// A small PCG generator (PCG-XSH-RR 64/32) for shuffles: seeded once, from the system's
/// randomness unless a test names the seed.
#[derive(Debug)]
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Rng {
        let mut r = Rng(seed.wrapping_add(0x9e37_79b9_7f4a_7c15));
        r.next();
        r
    }

    fn next(&mut self) -> u32 {
        let old = self.0;
        self.0 = old
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let x = u32::try_from((((old >> 18) ^ old) >> 27) & 0xffff_ffff).unwrap_or(0);
        x.rotate_right(u32::try_from(old >> 59).unwrap_or(0))
    }

    /// Puts `v` in a random order (Fisher-Yates).
    pub(crate) fn shuffle(&mut self, v: &mut [usize]) {
        for i in (1..v.len()).rev() {
            let n = u64::try_from(i).unwrap_or(u64::MAX).saturating_add(1);
            let j = usize::try_from(u64::from(self.next()).checked_rem(n).unwrap_or(0)).unwrap_or(0);
            v.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| (*x).to_owned()).collect()
    }

    /// fit's `TestReloadKicksIdle`, the list's part: a reload keeps its place, an undo gives
    /// an item back.
    #[test]
    fn list_state() {
        let mut l = ListState::default();
        let mut none = |_: &mut [usize]| {};
        let a = v(&["a", "b", "c"]);
        l.next(&a, false, true, &mut none);
        l.next(&a, false, true, &mut none);
        let b = v(&["x", "b", "y"]);
        assert_eq!(l.next(&b, false, true, &mut none).as_deref(), Some("y"));
        let ch = |item: &str| Choice {
            item: item.into(),
            from: FROM_LIST,
            offset: 0,
            slot: 0,
        };
        l.undo(&ch("y"));
        assert_eq!(l.next(&b, false, true, &mut none).as_deref(), Some("y"));
        l.undo(&ch("z"));
        assert_eq!(l.next(&[], false, true, &mut none), None);
        // Played out without a loop.
        let mut l = ListState::default();
        assert_eq!(
            l.next(&a[..1], false, false, &mut none).as_deref(),
            Some("a")
        );
        assert_eq!(l.next(&a[..1], false, false, &mut none), None);
        // A shuffle is a permutation.
        let mut r = Rng::new(7);
        let mut o: Vec<usize> = (0..10).collect();
        r.shuffle(&mut o);
        let mut s = o.clone();
        s.sort_unstable();
        assert_eq!(s, (0..10).collect::<Vec<_>>());
    }
}
