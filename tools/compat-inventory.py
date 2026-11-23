#!/usr/bin/env python3
"""The field inventory of the compatibility gate (docs/compatibility.md).

Reads fit's JSON Schema from the pinned fixtures (tests/compat/fixtures/fit/schema/fit.schema.json,
which fit generates from its config types and modules, as it does its config reference) and writes:

- tests/compat/fit-fields.txt: every field path fit accepts, one a line, tab-separated:
  path, section, type, flags;
- the generated tables of docs/compatibility.md and docs/compatibility.zh-CN.md: every field,
  marked supported when tests/compat/supported.txt lists it and "not supported yet" otherwise.

Run from anywhere, with no argument but --root (default: the repository holding this script):

    python3 tools/compat-inventory.py            # regenerate
    python3 tools/compat-inventory.py --check    # fail if anything generated is out of date

Python 3.8 or later, standard library only: a dev tool, not a dependency of akit.

Path notation: `a.b` an object's field; `a[]` the items of an array; `a.*` the values of a map;
`h<m>` the object at `h` whose `module` is `m` (fit's handlers, middleware and sync modules,
whose params depend on the module). So `routes[].handler<respond>.params.status` is the status
param of a route's handler when the handler is respond.
"""

import argparse
import json
import os
import re
import sys

SCHEMA = "tests/compat/fixtures/fit/schema/fit.schema.json"
FIELDS = "tests/compat/fit-fields.txt"
SUPPORTED = "tests/compat/supported.txt"
DOCS = {"en": "docs/compatibility.md", "zh": "docs/compatibility.zh-CN.md"}
BEGIN = "<!-- BEGIN GENERATED: tools/compat-inventory.py; do not edit -->"
END = "<!-- END GENERATED -->"

DURATION_UNITS = "(ns|us|"
PERCENT = "%$"


class Inventory:
    def __init__(self, schema):
        self.defs = schema.get("$defs", {})
        self.rows = []  # (path, section, type, flags)

    def ref(self, node):
        r = node.get("$ref")
        if not r:
            return None, node
        name = r.rsplit("/", 1)[-1]
        return name, self.defs[name]

    def type_of(self, node):
        """A short type for a schema node: what a reader of the inventory needs."""
        name, node = self.ref(node)
        if "oneOf" in node:
            kinds = [self.type_of(n) for n in node["oneOf"]]
            if any(PERCENT in n.get("pattern", "") for n in node["oneOf"]):
                return "bytes|percent"
            return "|".join(kinds)
        if "enum" in node:
            return "enum(" + "|".join(str(v) for v in node["enum"]) + ")"
        if "const" in node:
            return "const(" + str(node["const"]) + ")"
        t = node.get("type")
        if t == "string" and DURATION_UNITS in node.get("pattern", ""):
            return "duration"
        if t == "array":
            return "array<" + self.type_of(node.get("items", {})) + ">"
        if t == "object" or "properties" in node:
            ap = node.get("additionalProperties")
            if isinstance(ap, dict) and "properties" not in node:
                return "map<" + self.type_of(ap) + ">"
            return "object"
        if t is None:
            return "any"
        if isinstance(t, list):
            return "|".join(t)
        return t

    def flags(self, node, required):
        _, target = self.ref(node)
        out = []
        if required:
            out.append("required")
        if node.get("deprecated") or target.get("deprecated"):
            out.append("deprecated")
        return ",".join(out) or "-"

    def add(self, path, section, node, required=False):
        self.rows.append((path, section, self.type_of(node), self.flags(node, required)))

    def walk(self, path, section, node, stack):
        """The fields under node, an object, array or map at path."""
        name, node = self.ref(node)
        if name is not None:
            if name in stack:
                return  # recursive: the fields are those of the enclosing occurrence
            stack = stack + (name,)
        if node.get("type") == "array":
            self.walk(path + "[]", section, node.get("items", {}), stack)
            return
        props = node.get("properties", {})
        ap = node.get("additionalProperties")
        if isinstance(ap, dict) and not props:
            self.walk(path + ".*", section, ap, stack)
            return
        required = set(node.get("required", []))
        modules = self.modules(node)
        for key in sorted(props):
            child = props[key]
            if modules and key == "params":
                self.add(f"{path}.params", section, {"type": "object"}, key in required)
                continue
            self.add(f"{path}.{key}", section, child, key in required)
            self.walk(f"{path}.{key}", section, child, stack)
        for module, ns, params in modules:
            mpath = f"{path}<{module}>"
            msec = f"{ns}/{module}"
            self.rows.append((mpath, msec, "module", "-"))
            self.add(f"{mpath}.params", msec, params)
            self.walk(f"{mpath}.params", msec, params, stack)

    def modules(self, node):
        """[(module, namespace, params schema)] when node dispatches its params on module."""
        out = []
        for clause in node.get("allOf", []):
            const = clause.get("if", {}).get("properties", {}).get("module", {}).get("const")
            params = clause.get("then", {}).get("properties", {}).get("params")
            if const is None or params is None:
                continue
            ref = params.get("$ref", "")
            ns = ref.rsplit("/", 1)[-1].rsplit(".", 1)[0] if ref else "module"
            out.append((const, ns, params))
        return out

    def build(self, schema):
        required = set(schema.get("required", []))
        for key in sorted(schema.get("properties", {})):
            node = schema["properties"][key]
            self.add(key, key, node, key in required)
            self.walk(key, key, node, ())
        seen = set()
        for row in self.rows:
            if row[0] in seen:
                raise SystemExit(f"duplicate field path {row[0]}")
            seen.add(row[0])
        return self.rows


def read_supported(root, known):
    path = os.path.join(root, SUPPORTED)
    out = set()
    with open(path, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            if line not in known:
                raise SystemExit(f"{SUPPORTED}:{n}: {line} is not a field fit accepts")
            out.add(line)
    return out


def fields_text(rows, pin):
    lines = [
        "# Every config field path fit accepts, generated by tools/compat-inventory.py from fit's",
        f"# JSON Schema at fit {pin}; do not edit. Columns, tab-separated: path, section, type, flags.",
        "# Notation: a.b field, a[] array items, a.* map values, h<m> the object whose module is m.",
    ]
    lines += ["\t".join(r) for r in rows]
    return "\n".join(lines) + "\n"


TEXT = {
    "en": {
        "summary": "| Section | Fields | Supported | Not supported yet |",
        "total": "**Total**",
        "fieldhead": "| Field | Type | Status |",
        "yes": "supported",
        "no": "not supported yet",
        "intro": "{n} fields in {s} sections, at fit `{pin}`; {k} supported.",
    },
    "zh": {
        "summary": "| 部分 | 字段 | 已支持 | 尚不支持 |",
        "total": "**合计**",
        "fieldhead": "| 字段 | 类型 | 状态 |",
        "yes": "已支持",
        "no": "尚不支持（not supported yet）",
        "intro": "共 {n} 个字段，分属 {s} 个部分，对应 fit `{pin}`；已支持 {k} 个。",
    },
}


def esc(s):
    return s.replace("|", "\\|")


def docs_block(rows, supported, pin, lang):
    t = TEXT[lang]
    sections = {}
    for row in rows:
        sections.setdefault(row[1], []).append(row)
    out = [BEGIN, ""]
    out.append(t["intro"].format(n=len(rows), s=len(sections), pin=pin, k=len(supported)))
    out += ["", t["summary"], "| --- | --- | --- | --- |"]
    for sec in sorted(sections):
        n = len(sections[sec])
        k = sum(1 for r in sections[sec] if r[0] in supported)
        anchor = "fields-" + re.sub(r"[^a-z0-9]+", "-", sec.lower()).strip("-")
        out.append(f"| [`{sec}`](#{anchor}) | {n} | {k} | {n - k} |")
    out.append(f"| {t['total']} | {len(rows)} | {len(supported)} | {len(rows) - len(supported)} |")
    for sec in sorted(sections):
        anchor = "fields-" + re.sub(r"[^a-z0-9]+", "-", sec.lower()).strip("-")
        out += ["", f'<a id="{anchor}"></a>', "", f"#### `{sec}`", "", t["fieldhead"], "| --- | --- | --- |"]
        for path, _, typ, flags in sections[sec]:
            status = t["yes"] if path in supported else t["no"]
            typ = typ if flags == "-" else f"{typ}, {flags}"
            out.append(f"| `{esc(path)}` | `{esc(typ)}` | {status} |")
    out += ["", END]
    return "\n".join(out)


def splice(text, block, name):
    i, j = text.find(BEGIN), text.find(END)
    if i < 0 or j < i:
        raise SystemExit(f"{name}: no generated block ({BEGIN} ... {END})")
    return text[:i] + block + text[j + len(END):]


def read_pin(root):
    with open(os.path.join(root, "tests/compat/fixtures/PINS"), encoding="utf-8") as f:
        for line in f:
            if line.startswith("fit="):
                return line.strip().split("=", 1)[1][:8]
    raise SystemExit("tests/compat/fixtures/PINS: no fit= line")


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--root", default=os.path.dirname(here))
    ap.add_argument("--check", action="store_true", help="fail if a generated file is out of date")
    args = ap.parse_args()
    root = args.root

    with open(os.path.join(root, SCHEMA), encoding="utf-8") as f:
        schema = json.load(f)
    pin = read_pin(root)
    rows = Inventory(schema).build(schema)
    supported = read_supported(root, {r[0] for r in rows})

    outputs = {FIELDS: fields_text(rows, pin)}
    for lang, doc in DOCS.items():
        with open(os.path.join(root, doc), encoding="utf-8") as f:
            outputs[doc] = splice(f.read(), docs_block(rows, supported, pin, lang), doc)

    stale = []
    for rel, text in outputs.items():
        path = os.path.join(root, rel)
        old = None
        if os.path.exists(path):
            with open(path, encoding="utf-8") as f:
                old = f.read()
        if old == text:
            continue
        if args.check:
            stale.append(rel)
        else:
            with open(path, "w", encoding="utf-8") as f:
                f.write(text)
    if stale:
        print("out of date, run python3 tools/compat-inventory.py: " + ", ".join(stale), file=sys.stderr)
        return 1
    print(f"compat inventory: {len(rows)} fields, {len(supported)} supported")
    return 0


if __name__ == "__main__":
    sys.exit(main())
