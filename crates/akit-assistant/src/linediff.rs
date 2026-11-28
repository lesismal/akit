//! A line diff of two texts, as unified diffs show it (fit's `internal/linediff`): a proposal's
//! diff in each format.
//!
//! Lines and diagonals are bounded by the texts' line counts and [`MAX_EDITS`], far below
//! `isize::MAX`, so the arithmetic of Myers' walk below cannot overflow.
#![allow(clippy::arithmetic_side_effects)]

use std::fmt::Write as _;

/// The unchanged lines shown around a change.
const CONTEXT: usize = 3;

/// The edits the diff looks for, past the lines both texts begin and end with: past it, what lies
/// between is shown as removed and added whole, rather than the work growing with it.
pub const MAX_EDITS: usize = 1000;

/// A line of a diff: `' '` kept, `'-'` removed, `'+'` added.
#[derive(Clone, Copy)]
struct Op<'a> {
    kind: char,
    line: &'a str,
}

fn split_lines(s: &str) -> Vec<&str> {
    if s.is_empty() {
        return Vec::new();
    }
    s.strip_suffix('\n').unwrap_or(s).split('\n').collect()
}

/// The unified diff of `a` and `b`, texts, labelled `from` and `to`; `""` when they are the same.
#[must_use]
pub fn unified(from: &str, to: &str, a: &str, b: &str) -> String {
    unified_within(from, to, a, b, MAX_EDITS)
}

fn unified_within(from: &str, to: &str, a: &str, b: &str, max_edits: usize) -> String {
    if a == b {
        return String::new();
    }
    let ops = diff_lines(&split_lines(a), &split_lines(b), max_edits);
    let mut out = format!("--- {from}\n+++ {to}\n");
    // The lines of a and b each op is at, from 1.
    let mut pos = Vec::with_capacity(ops.len());
    let (mut al, mut bl) = (1usize, 1usize);
    for op in &ops {
        pos.push((al, bl));
        if op.kind != '+' {
            al = al.saturating_add(1);
        }
        if op.kind != '-' {
            bl = bl.saturating_add(1);
        }
    }
    // Each hunk runs from CONTEXT lines before a change to CONTEXT after the last change within
    // 2*CONTEXT of the one before.
    let mut i = 0;
    while i < ops.len() {
        if ops.get(i).is_some_and(|o| o.kind == ' ') {
            i = i.saturating_add(1);
            continue;
        }
        let start = i.saturating_sub(CONTEXT);
        let mut end = i;
        for (j, op) in ops.iter().enumerate().skip(i) {
            if op.kind != ' ' {
                end = j;
            } else if j.saturating_sub(end) > 2 * CONTEXT {
                break;
            }
        }
        end = ops.len().saturating_sub(1).min(end.saturating_add(CONTEXT));
        let hunk = ops.get(start..=end).unwrap_or_default();
        let na = hunk.iter().filter(|o| o.kind != '+').count();
        let nb = hunk.iter().filter(|o| o.kind != '-').count();
        let (mut as_, mut bs) = pos.get(start).copied().unwrap_or((1, 1));
        if na == 0 {
            as_ = as_.saturating_sub(1);
        }
        if nb == 0 {
            bs = bs.saturating_sub(1);
        }
        let _ = writeln!(out, "@@ -{as_},{na} +{bs},{nb} @@");
        for op in hunk {
            out.push(op.kind);
            out.push_str(op.line);
            out.push('\n');
        }
        i = end.saturating_add(1);
    }
    out
}

/// The edits from `a` to `b`: the lines both begin and end with kept, and those between by
/// Myers' algorithm, within `max_edits`.
fn diff_lines<'a>(a: &[&'a str], b: &[&'a str], max_edits: usize) -> Vec<Op<'a>> {
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suf = a
        .iter()
        .skip(pre)
        .rev()
        .zip(b.iter().skip(pre).rev())
        .take_while(|(x, y)| x == y)
        .count();
    let keep = |l: &&'a str| Op { kind: ' ', line: l };
    let mut ops: Vec<Op<'a>> = a.iter().take(pre).map(keep).collect();
    let am = a.get(pre..a.len().saturating_sub(suf)).unwrap_or_default();
    let bm = b.get(pre..b.len().saturating_sub(suf)).unwrap_or_default();
    ops.extend(myers(am, bm, max_edits));
    ops.extend(a.iter().skip(a.len().saturating_sub(suf)).map(keep));
    ops
}

/// The shortest edits from `a` to `b`, or, past `max_edits`, `a` removed whole and `b` added
/// whole.
fn myers<'a>(a: &[&'a str], b: &[&'a str], max_edits: usize) -> Vec<Op<'a>> {
    let whole = || {
        a.iter()
            .map(|l| Op { kind: '-', line: l })
            .chain(b.iter().map(|l| Op { kind: '+', line: l }))
            .collect::<Vec<_>>()
    };
    let (n, m) = (a.len(), b.len());
    if n == 0 || m == 0 {
        return whole();
    }
    let n_i = isize::try_from(n).unwrap_or(isize::MAX);
    let m_i = isize::try_from(m).unwrap_or(isize::MAX);
    let limit = n.saturating_add(m).min(max_edits);
    let limit_i = isize::try_from(limit).unwrap_or(isize::MAX);
    let off = limit_i.saturating_add(1);
    let mut v = vec![0isize; limit.saturating_mul(2).saturating_add(3)];
    let at = |v: &[isize], k: isize| -> isize {
        usize::try_from(off.saturating_add(k))
            .ok()
            .and_then(|i| v.get(i).copied())
            .unwrap_or(0)
    };
    // trace[d] is v's diagonals -d..=d as round d began.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    for d in 0..=limit_i {
        trace.push((-d..=d).map(|k| at(&v, k)).collect());
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && at(&v, k - 1) < at(&v, k + 1)) {
                at(&v, k + 1)
            } else {
                at(&v, k - 1) + 1
            };
            let mut y = x - k;
            while x < n_i && y < m_i && ix(a, x) == ix(b, y) {
                x += 1;
                y += 1;
            }
            if let Some(slot) = usize::try_from(off + k).ok().and_then(|i| v.get_mut(i)) {
                *slot = x;
            }
            if x >= n_i && y >= m_i {
                return backtrack(a, b, &trace, d);
            }
            k += 2;
        }
    }
    whole()
}

fn ix<'a>(s: &[&'a str], i: isize) -> Option<&'a str> {
    usize::try_from(i).ok().and_then(|i| s.get(i)).copied()
}

/// Walks `trace` back from the end, `d` edits away, into the edits.
fn backtrack<'a>(a: &[&'a str], b: &[&'a str], trace: &[Vec<isize>], d: isize) -> Vec<Op<'a>> {
    let mut x = isize::try_from(a.len()).unwrap_or(isize::MAX);
    let mut y = isize::try_from(b.len()).unwrap_or(isize::MAX);
    let mut rev = Vec::new();
    let mut d = d;
    while d >= 0 {
        let vd = usize::try_from(d)
            .ok()
            .and_then(|i| trace.get(i))
            .map_or(&[][..], Vec::as_slice);
        let at = |k: isize| -> isize {
            usize::try_from(k + d)
                .ok()
                .and_then(|i| vd.get(i).copied())
                .unwrap_or(0)
        };
        let k = x - y;
        let pk = if k == -d || (k != d && at(k - 1) < at(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let px = if d > 0 { at(pk) } else { 0 };
        let py = px - pk;
        while x > px && y > py {
            if let Some(l) = ix(a, x - 1) {
                rev.push(Op { kind: ' ', line: l });
            }
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            if x == px {
                if let Some(l) = ix(b, py) {
                    rev.push(Op { kind: '+', line: l });
                }
            } else if let Some(l) = ix(a, px) {
                rev.push(Op { kind: '-', line: l });
            }
        }
        x = px;
        y = py;
        d -= 1;
    }
    rev.reverse();
    rev
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn lines(n: usize, f: impl Fn(usize) -> String) -> String {
        (0..n).map(|i| f(i) + "\n").collect()
    }

    /// fit's `TestUnifiedDiff`: the edits found are the fewest, shown in hunks with three lines
    /// of context, those near each other in one; past the edits it looks for, the middle is shown
    /// removed and added whole.
    #[test]
    fn unified_diff() {
        assert_eq!(unified("a", "b", "x\n", "x\n"), "");
        let a = lines(30, |i| format!("line {i}"));
        let b = lines(30, |i| match i {
            2 => "changed 2".into(),
            4 => "changed 4".into(),
            25 => "changed 25".into(),
            _ => format!("line {i}"),
        });
        let want = "--- v1\n+++ proposed\n@@ -1,8 +1,8 @@\n line 0\n line 1\n-line 2\n+changed 2\n line 3\n-line 4\n+changed 4\n line 5\n line 6\n line 7\n".to_owned()
            + "@@ -23,7 +23,7 @@\n line 22\n line 23\n line 24\n-line 25\n+changed 25\n line 26\n line 27\n line 28\n";
        assert_eq!(unified("v1", "proposed", &a, &b), want);
        // From nothing, and to nothing.
        assert!(unified("a", "b", "", "x\ny\n").contains("@@ -0,0 +1,2 @@\n+x\n+y\n"));
        assert!(unified("a", "b", "x\n", "").contains("@@ -1,1 +0,0 @@\n-x\n"));
        // Insertions and removals between kept lines.
        let d = unified("a", "b", "a\nb\nc\nd\n", "a\nx\nc\ny\nz\nd\n");
        assert!(d.contains("-b\n+x\n c\n+y\n+z\n d\n"), "{d}");
        // Too many edits: the middle whole.
        let d = unified_within("a", "b", "k\na\nb\nc\nk\n", "k\nx\ny\nz\nk\n", 2);
        assert!(d.contains("-a\n-b\n-c\n+x\n+y\n+z\n"), "{d}");
    }
}
