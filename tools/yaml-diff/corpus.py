#!/usr/bin/env python3
"""The corpus of akit's differential check of its YAML reader against fit's (decision 005).

Writes <out>/cases.hex, one hex-encoded document a line: hand-written cases, every YAML file in
fit's repository and every YAML block in its docs, akit's copies of fit's YAML templates with their
commented-out fields written out, and random edits (characters replaced, inserted, deleted or
copied) of the small ones.

    corpus.py <out> <fit dir> <templates dir> [seed] [edits]
"""
import os
import random
import re
import sys

HAND = r'''a: 1
---
a: 1
b: [1, 2, {c: d}]
---
x: &a
  k: v
y: *a
z:
  <<: *a
  w: 1
---
- a
- - b
  - c
- d: e
  f: g
-
---
s: |
  line
   more

  last
f: >-
  one
  two

  three
k: |+
  keep

---
q: "a\tbé \
  c"
r: 'it''s
  folded'
---
n: [0x1F, 0o17, 017, 08, 1_000, +5, -0, -0.0, 1e3, .5, 1., 99999999999999999999, 18446744073709551615, -9223372036854775808, -9223372036854775809]
---
t: [yes, no, on, off, y, n, True, FALSE, Null, NULL, ~, '', null]
---
d: [2001-12-14, 2001-12-14t21:59:43.10-05:00, 2001-12-14 21:59:43.10, 2002-12-14T00:00:00Z, 2001-02-30]
---
2001-12-14: x
---
? a
: b
---
a: !!str 1
b: !!int "2"
c: !!float 3
d: !!bool true
e: !!null x
f: !!binary aGk=
g: !!timestamp 2001-01-01
h: !custom x
---
a: !<tag:yaml.org,2002:str> 1
---
%YAML 1.2
---
a: 1
...
---
%TAG !e! tag:example.com,2000:
---
a: !e!x 1
---
key with spaces: value with spaces
"quoted key": 1
'single': 2
---
a: b: c
---
a:
  - 1
 - 2
---
a: 1
 b: 2
---
[a, b]: 1
---
{a: 1}: 2
---
a: [1, 2,]
b: {x: 1, y, z: }
c: [a: 1, b]
---
url: http://example.com:8080/x
port: :8080
colon: a:b
hash: a#b
comment: a # b
---
empty:
also:
---
- &x 1
- *x
- &y [1, 2]
- *y
---
a: &x 1
b: &x 2
c: *x
---
<<: {a: 1}
b: 2
---
a:
  <<: [{a: 1}, {b: 2}, {a: 3}]
---
a: <<
---
a:	1
---
	a: 1
---
a: "unterminated
---
a: 'x
---
- |
 x
- >
 y
---
a: |2
   x
  y
---
a: |
    
  x
---
- a
b: c
---
a: b
- c
---
plain: multi
  line
  value
---
a: x
  b: y
---
{a: 1, b: [2, 3], "c": {d: e}}
---
[1, [2, [3, [4]]]]
---
--- 5
---
!!map {a: 1}
---
&anchor
a: 1
---
a: &x
b: *x
---
"a":1
---
{"a":1, "b":2}
---
a: [b
  c, d]
---
a: "x
  y"
---
a: 'x

  y'
---
- - - a
---
a: [1, 2] x
---
a: "x" y
---
a:
- b
-   c: 1
    d: 2
---
'''

# A YAML line of a template's commented-out block: a nested comment, a key, an entry or a value.
CODE = re.compile(r'''^(# |[a-z0-9_"'./-]+:( |$)|- |-$|[\]\}\[\{]|"|\|)''')


def written_out(text):
    out = []
    for line in text.split("\n"):
        t = line.lstrip(" ")
        indent = line[: len(line) - len(t)]
        out.append(indent + t[2:] if t.startswith("# ") and CODE.match(t[2:]) else line)
    return "\n".join(out)


def main():
    out, fit, templates = sys.argv[1], sys.argv[2], sys.argv[3]
    random.seed(int(sys.argv[4]) if len(sys.argv) > 4 else 1)
    edits = int(sys.argv[5]) if len(sys.argv) > 5 else 100000
    seeds = [d + "\n" for d in HAND.split("\n---\n")]
    for root, dirs, files in os.walk(fit):
        dirs[:] = [d for d in dirs if not d.startswith(".")]
        for f in sorted(files):
            p = os.path.join(root, f)
            if f.endswith((".yaml", ".yml")):
                seeds.append(open(p, encoding="utf-8", errors="replace").read())
            elif f.endswith(".md"):
                text = open(p, encoding="utf-8", errors="replace").read()
                seeds += [m.group(1) for m in re.finditer(r"```ya?ml\n(.*?)```", text, re.S)]
    for root, dirs, files in os.walk(templates):
        for f in sorted(files):
            seeds.append(written_out(open(os.path.join(root, f), encoding="utf-8").read()))
    cases = list(seeds)
    small = [s for s in seeds if 0 < len(s) < 600]
    chars = "\"'{}[],:-?&*!|>#\n \t%@`\\~0.<"
    for _ in range(edits):
        b = random.choice(small)
        for _ in range(random.randint(1, 3)):
            i = random.randrange(len(b) + 1)
            op = random.random()
            c = random.choice(chars)
            if op < 0.4:
                b = b[:i] + c + b[i + 1 :]
            elif op < 0.7:
                b = b[:i] + c + b[i:]
            elif op < 0.85:
                b = b[:i] + b[i + 1 :]
            else:
                j = random.randrange(len(b) + 1)
                b = b[:i] + b[min(i, j) : max(i, j)] + b[i:]
        cases.append(b)
    with open(os.path.join(out, "cases.hex"), "w") as w:
        for c in cases:
            w.write(c.encode().hex() + "\n")
    print(f"{len(cases)} cases ({len(seeds)} seeds)")


main()
