#!/usr/bin/env python3
"""akit's config reference: the generated tables of docs/reference/config.md and its Chinese twin.

The prose of the reference (each chapter's text and its examples) is written by hand in the two
files; every table of fields is generated, between markers

    <!-- BEGIN GENERATED: <key> -->
    ...
    <!-- END GENERATED -->

where <key> is the inventory path of the object whose fields the table lists (`listeners[]`,
`routes[].handler<proxy>.params`; `config` for the top level), or one of `summary`,
`extensions`, `modules:routes[].handler` and `modules:routes[].middleware[]`.

What a table says comes from three places, so that it cannot drift from what akit reads:

- fit's JSON Schema at the pinned commit (tests/compat/fixtures/fit/schema/fit.schema.json): every
  field fit has, its type, default and description, walked as tools/compat-inventory.py walks it;
- tests/compat/supported.txt: the fields akit reads, which the compatibility gate's test
  `akit_config_lists_match_the_inventory` holds equal to akit-config's lists of fields
  (crates/akit-config/src/model.rs); every other field of an object akit reads is refused as
  "not supported yet";
- tools/config-reference-notes.json: akit's extensions (fields fit does not have, which
  `reference_matches_akit_config` holds equal to the `*_AKIT_ONLY` lists of akit-config) and
  akit's notes on fields it reads differently from fit, in both languages.

Each object akit reads must have its table in both files, and each table must name an object akit
reads: a field supported, refused or added changes the tables, and the check fails until they are
regenerated. Run from anywhere:

    python3 tools/config-reference.py            # regenerate the tables
    python3 tools/config-reference.py --check    # fail if a table is out of date

Python 3.8 or later, standard library only: a dev tool, not a dependency of akit.
"""

import argparse
import importlib.util
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
NOTES = "tools/config-reference-notes.json"
DOCS = {"en": "docs/reference/config.md", "zh": "docs/reference/config.zh-CN.md"}
BEGIN_RE = re.compile(r"<!-- BEGIN GENERATED: (\S+) -->")
END = "<!-- END GENERATED -->"
KEY_RE = re.compile(r"^[a-z0-9_]+$")
MODULE_BLOCKS = ("routes[].handler", "routes[].middleware[]")


def load_inventory_module():
    # The inventory's module is loaded from its file; no bytecode is left beside it.
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location(
        "compat_inventory", os.path.join(HERE, "compat-inventory.py")
    )
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


INV = load_inventory_module()


class Fields(INV.Inventory):
    """The inventory, keeping each field's schema node for its type, default and description."""

    def __init__(self, schema):
        super().__init__(schema)
        self.nodes = {}

    def add(self, path, section, node, required=False):
        super().add(path, section, node, required)
        self.nodes[path] = (node, required)


TEXT = {
    "en": {
        "head": "| Field | Type | Default | Description |",
        "required": "required",
        "deprecated": "Deprecated. ",
        "not_yet": "**Not supported yet**: refused by name. ",
        "extension": "**akit's extension** ([decision {decision}](../decisions/{file}.md)). ",
        "note": " **akit:** ",
        "string": "string",
        "integer": "integer",
        "number": "number",
        "boolean": "boolean",
        "duration": "duration",
        "object": "object",
        "any": "any",
        "bytes_percent": "bytes or percent",
        "list": "list of {}",
        "map": "map of {}",
        "or": " or ",
        "enum": "{}: {}",
        "min": "{}, {} or more",
        "max": "{}, at most {}",
        "range": "{}, {} to {}",
        "sep": ", ",
        "describe": "",
        "summary": (
            "akit reads **{read}** of the {total} fields of fit's config (fit `{pin}`), "
            "refuses the rest by name as not supported yet, and adds **{ext}** of its own. "
            "Of the {objects} objects akit reads, each has its table below."
        ),
        "mod_head": "| Module | Status |",
        "mod_yes": "supported: [params](#{anchor})",
        "mod_no": "**not supported yet**: refused by name",
        "ext_head": "| Field | Type | Since | Decision |",
    },
    "zh": {
        "head": "| 字段 | 类型 | 默认值 | 说明 |",
        "required": "必填",
        "deprecated": "已弃用。",
        "not_yet": "**尚不支持**（not supported yet）：按名称拒绝。",
        "extension": "**akit 扩展**（[决定 {decision}](../decisions/{file}.zh-CN.md)）。",
        "note": "**akit：**",
        "string": "字符串",
        "integer": "整数",
        "number": "数",
        "boolean": "布尔值",
        "duration": "时长",
        "object": "对象",
        "any": "任意",
        "bytes_percent": "字节数或百分比",
        "list": "{}的列表",
        "map": "值为{}的映射",
        "or": "或",
        "enum": "{}：{}",
        "min": "{}，≥ {}",
        "max": "{}，≤ {}",
        "range": "{}，{} 到 {}",
        "sep": "、",
        "describe": "各字段的说明见英文版的[同一张表](config.md#{anchor})。",
        "summary": (
            "fit 的配置共有 {total} 个字段（fit `{pin}`），akit 读取其中 **{read}** 个，"
            "其余按名称拒绝为“尚不支持”（not supported yet），另有 **{ext}** 个 akit 自己的字段。"
            "akit 读取的 {objects} 个对象，下文各有一张表。"
        ),
        "mod_head": "| 模块 | 状态 |",
        "mod_yes": "已支持：[参数](#{anchor})",
        "mod_no": "**尚不支持**：按名称拒绝",
        "ext_head": "| 字段 | 类型 | 引入于 | 决定 |",
    },
}


def anchor(key):
    """The anchor of an object's table: `listeners[].tls` is `cfg-listeners-tls`."""
    if key == "config":
        return "cfg-config"
    return "cfg-" + re.sub(r"[^a-z0-9]+", "-", key.lower()).strip("-")


def esc(s):
    return s.replace("|", "\\|").replace("\n", " ")


# What fit's descriptions say of fit's own commands that akit has no twin of.
PHRASES = [
    ("as fit hash-password or htpasswd -B makes it", "as htpasswd -B makes it"),
    ("$XDG_DATA_HOME/fit, or ~/.local/share/fit", "$XDG_DATA_HOME/akit, or ~/.local/share/akit"),
    ("fib's workers", "akio's I/O threads"),
    ("goroutines", "threads"),
    ("goroutine", "thread"),
]


def as_akit(text, default=None):
    """fit's description of a field akit reads, said of akit. akit keeps fit's default names (a
    label prefix `fit`, a cookie `fit_sticky`), so a default named in the text, and a word joined
    to a path or a name (`fit/`, `fit_sticky`, `lesismal.github.io/fit`), stay as they are."""
    for old, new in PHRASES:
        text = text.replace(old, new)
    keep = []
    if isinstance(default, str) and "fit" in default:
        keep.append(default)
        text = text.replace(default, "\0")
    text = re.sub(r"(?<![/\w.-])fit's\b", "akit's", text)
    text = re.sub(r"(?<![/\w.-])fit(?![/\w-])", "akit", text)
    text = re.sub(r"(?<![/\w.-])fib's\b", "akio's", text)
    text = re.sub(r"(?<![/\w.-])fib(?![/\w-])", "akio", text)
    for k in keep:
        text = text.replace("\0", k)
    return text


class Reference:
    def __init__(self, root):
        with open(os.path.join(root, INV.SCHEMA), encoding="utf-8") as f:
            schema = json.load(f)
        self.pin = INV.read_pin(root)
        inv = Fields(schema)
        self.inv = inv
        self.rows = inv.build(schema)
        self.paths = [r[0] for r in self.rows]
        self.known = set(self.paths)
        self.supported = INV.read_supported(root, self.known)
        with open(os.path.join(root, NOTES), encoding="utf-8") as f:
            notes = json.load(f)
        self.extensions = notes["extensions"]
        self.notes = notes["notes"]
        # Each object's direct fields, in the order fit's schema writes them (by name).
        self.children = {}
        for p in self.paths + sorted(self.extensions):
            parent, key = split(p)
            if parent is not None:
                self.children.setdefault(parent, []).append(p)
        self.objects = [o for o in self.children if self.reads(o) and not o.endswith(">")]
        self.check_notes()

    def reads(self, obj):
        """Whether akit reads the object at `obj` (an inventory path, "" for the top)."""
        if obj == "":
            return True
        base = re.sub(r"(\[\]|\.\*)+$", "", obj)
        return base in self.supported

    def check_notes(self):
        for p, e in self.extensions.items():
            if p in self.known:
                raise SystemExit(f"{NOTES}: extension {p} is a field fit has: list it with fit's")
            parent, _ = split(p)
            if parent is None or not self.reads(parent):
                raise SystemExit(f"{NOTES}: extension {p} is not a field of an object akit reads")
            for k in ("type", "since", "decision", "file", "en", "zh", "en_short", "zh_short"):
                if k not in e:
                    raise SystemExit(f"{NOTES}: extension {p} has no {k}")
        for p, n in self.notes.items():
            if p not in self.known and p not in self.extensions:
                raise SystemExit(f"{NOTES}: note on {p}, which is not a field of fit's or akit's")
            parent, _ = split(p)
            if not (p in self.supported or (parent is not None and self.reads(parent))):
                raise SystemExit(f"{NOTES}: note on {p}, which no table shows")
            if set(n) != {"en", "zh"}:
                raise SystemExit(f"{NOTES}: the note on {p} needs en and zh, and nothing else")

    # Types and defaults, from fit's schema.

    def type_of(self, node, lang, path):
        t = TEXT[lang]
        _, n = self.inv.ref(node)
        if "oneOf" in n:
            if any(INV.PERCENT in m.get("pattern", "") for m in n["oneOf"]):
                return t["bytes_percent"]
            return t["or"].join(self.type_of(m, lang, path) for m in n["oneOf"])
        if "enum" in n:
            base = self.scalar(n, lang)
            return t["enum"].format(base, t["sep"].join(f"`{v}`" for v in n["enum"]))
        ty = n.get("type")
        if ty == "array":
            return t["list"].format(self.type_of(n.get("items", {}), lang, path + "[]"))
        if ty == "object" or "properties" in n:
            ap = n.get("additionalProperties")
            if isinstance(ap, dict) and "properties" not in n:
                return t["map"].format(self.type_of(ap, lang, path + ".*"))
            if path in self.children and self.reads(path):
                return f"[{t['object']}](#{anchor(path)})"
            return t["object"]
        return self.scalar(n, lang)

    def scalar(self, n, lang):
        t = TEXT[lang]
        ty = n.get("type")
        if ty == "string" and INV.DURATION_UNITS in n.get("pattern", ""):
            return t["duration"]
        if isinstance(ty, list):
            return t["or"].join(t.get(x, x) for x in ty)
        if ty is None:
            if "enum" in n:
                return t["string"]
            return t["any"]
        word = t.get(ty, ty)
        if ty in ("integer", "number"):
            lo, hi = n.get("minimum"), n.get("maximum")
            if lo is not None and hi is not None:
                return t["range"].format(word, lo, hi)
            if lo is not None:
                return t["min"].format(word, lo)
            if hi is not None:
                return t["max"].format(word, hi)
        return word

    def raw_default(self, node):
        _, target = self.inv.ref(node)
        return node.get("default", target.get("default"))

    def default_of(self, node):
        d = self.raw_default(node)
        if d is None:
            return ""
        return "`" + esc(json.dumps(d, ensure_ascii=False)) + "`"

    def description_of(self, node):
        _, target = self.inv.ref(node)
        return node.get("description") or target.get("description") or ""

    # The generated blocks.

    def row(self, path, lang):
        t = TEXT[lang]
        _, key = split(path)
        if path in self.extensions:
            e = self.extensions[path]
            ty = e["type"] if lang == "en" else e.get("type_zh", e["type"])
            desc = t["extension"].format(decision=e["decision"], file=e["file"]) + e[lang]
            return f"| `{key}` | {ty} | {e.get('default', '')} | {esc(desc)} |"
        node, required = self.inv.nodes[path]
        ty = self.type_of(node, lang, path)
        default = t["required"] if required else self.default_of(node)
        flags = INV.Inventory.flags(self.inv, node, False)
        prefix = t["deprecated"] if "deprecated" in flags else ""
        if path in self.supported:
            desc = as_akit(self.description_of(node), self.raw_default(node)) if lang == "en" else ""
            desc = prefix + desc
        else:
            desc = t["not_yet"] + (self.description_of(node) if lang == "en" else "")
        if path in self.notes:
            desc = (desc + t["note"] if desc else t["note"].strip()) + self.notes[path][lang]
        return f"| `{key}` | {esc(ty)} | {default} | {esc(desc.strip())} |"

    def object_block(self, obj, lang):
        t = TEXT[lang]
        fields = self.children[obj]
        # The required first, then by name: what a config must write comes first.
        fields = sorted(fields, key=lambda p: (not self.is_required(p), split(p)[1]))
        out = [f'<a id="{anchor(obj or "config")}"></a>', ""]
        if t["describe"]:
            out += [t["describe"].format(anchor=anchor(obj or "config")), ""]
        out += [t["head"], "| --- | --- | --- | --- |"]
        out += [self.row(p, lang) for p in fields]
        return out

    def is_required(self, path):
        return path in self.inv.nodes and self.inv.nodes[path][1]

    def modules_block(self, at, lang):
        t = TEXT[lang]
        mods = sorted(
            p[len(at) + 1 : -1]
            for p in self.paths
            if p.startswith(at + "<") and p.endswith(">") and "<" not in p[len(at) + 1 :]
        )
        out = [t["mod_head"], "| --- | --- |"]
        for m in mods:
            if f"{at}<{m}>" in self.supported:
                status = t["mod_yes"].format(anchor=anchor(f"{at}<{m}>.params"))
            else:
                status = t["mod_no"]
            out.append(f"| `{m}` | {status} |")
        return out

    def extensions_block(self, lang):
        t = TEXT[lang]
        out = [t["ext_head"], "| --- | --- | --- | --- |"]
        for p in sorted(self.extensions):
            e = self.extensions[p]
            ty = e["type"] if lang == "en" else e.get("type_zh", e["type"])
            twin = "" if lang == "en" else ".zh-CN"
            link = f"[{e['decision']}](../decisions/{e['file']}{twin}.md)"
            out.append(f"| `{p}` | {ty} | {e['since']} | {link}: {esc(e[lang + '_short'])} |")
        return out

    def summary_block(self, lang):
        read = sum(1 for p in self.supported if not (p.endswith(">") or p.endswith(">.params")))
        text = TEXT[lang]["summary"].format(
            read=read,
            total=sum(1 for p in self.paths if not (p.endswith(">") or p.endswith(">.params"))),
            pin=self.pin,
            ext=len(self.extensions),
            objects=len(self.objects),
        )
        return [text]

    def block(self, key, lang):
        if key == "summary":
            return self.summary_block(lang)
        if key == "extensions":
            return self.extensions_block(lang)
        if key.startswith("modules:"):
            at = key[len("modules:") :]
            if at not in MODULE_BLOCKS:
                raise SystemExit(f"no modules at {at}")
            return self.modules_block(at, lang)
        obj = "" if key == "config" else key
        if obj not in self.objects:
            raise SystemExit(f"{DOCS[lang]}: a table for {key}, which is not an object akit reads")
        return self.object_block(obj, lang)

    def render(self, text, lang):
        name = DOCS[lang]
        out, pos, seen = [], 0, set()
        for m in BEGIN_RE.finditer(text):
            if m.start() < pos:
                continue
            key = m.group(1)
            end = text.find(END, m.end())
            if end < 0:
                raise SystemExit(f"{name}: {key}'s block has no {END}")
            if key in seen:
                raise SystemExit(f"{name}: two tables for {key}")
            seen.add(key)
            out.append(text[pos : m.end()])
            out.append("\n" + "\n".join(self.block(key, lang)) + "\n")
            pos = end
        out.append(text[pos:])
        want = {"config" if o == "" else o for o in self.objects}
        want |= {"summary", "extensions"} | {f"modules:{m}" for m in MODULE_BLOCKS}
        missing = sorted(want - seen)
        if missing:
            raise SystemExit(f"{name}: no table for " + ", ".join(missing))
        return "".join(out)


def split(path):
    """(parent object, key) of a field's path, or (None, path) for what is not a field."""
    m = re.match(r"^(.*)\.([^.\[\]<>*]+)$", path)
    if m:
        if not KEY_RE.match(m.group(2)):
            return None, path
        return m.group(1), m.group(2)
    if KEY_RE.match(path):
        return "", path
    return None, path


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--root", default=os.path.dirname(HERE))
    ap.add_argument("--check", action="store_true", help="fail if a table is out of date")
    args = ap.parse_args()
    ref = Reference(args.root)
    stale = []
    for lang, rel in DOCS.items():
        path = os.path.join(args.root, rel)
        with open(path, encoding="utf-8") as f:
            old = f.read()
        new = ref.render(old, lang)
        if new == old:
            continue
        if args.check:
            stale.append(rel)
        else:
            with open(path, "w", encoding="utf-8") as f:
                f.write(new)
    if stale:
        print(
            "out of date, run python3 tools/config-reference.py: " + ", ".join(stale),
            file=sys.stderr,
        )
        return 1
    print(f"config reference: {len(ref.objects)} objects, {len(ref.supported)} fields supported")
    return 0


if __name__ == "__main__":
    sys.exit(main())
