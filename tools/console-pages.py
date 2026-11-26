#!/usr/bin/env python3
"""Copies fit's workbench into akit, with akit's names (I91, decisions 036 and 037).

fit's console serves its workbench, plain HTML, CSS and JavaScript with no build step, from
app/workbench/web, embedded in fit's binary. akit's console serves the same files with fit's names
renamed as akit names them: this script copies them, with fit's licence, at the fit commit the
compatibility gate pins (tests/compat/fixtures/PINS), into crates/akit-admin/console/web, applies
the renames below (RENAMES, KEEP) to every text file but vendor/'s, marks each file it changed
with a notice (NOTICE), writes fit's config schema at the same pin (the gate's fit.schema.json),
renamed alike, as crates/akit-admin/console/schema.json, writes the list of the changes as
crates/akit-admin/console/CHANGES, and writes crates/akit-admin/src/console/files.rs, which
builds the files into akit.

The renames are textual and the same on every file, so that a name the pages share (a header
field, a global, an i18n key, an English description the Chinese table is keyed by) is renamed
alike wherever it is written; akit's server sends and reads the renamed header fields and cookie.

    python3 -I tools/console-pages.py            # copy, from FIT_REPO (a clone, or the URL)
    python3 -I tools/console-pages.py --check    # the tree is the renamed copy (see check())

FIT_REPO names a local clone of fit to read the commit from (git archive); by default fit is
cloned from GitHub into a temporary directory, which is removed. --check compares with fit's
files byte for byte only when FIT_REPO, or a clone of fit beside akit's checkout, has the commit.
"""

import os
import re
import subprocess
import sys
import tarfile
import tempfile
from io import BytesIO
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEST = ROOT / "crates/akit-admin/console"
FILES_RS = ROOT / "crates/akit-admin/src/console/files.rs"
PINS = ROOT / "tests/compat/fixtures/PINS"
SCHEMA = ROOT / "tests/compat/fixtures/fit/schema/fit.schema.json"

# What stays as fit wrote it: English words and fit's own format, each with why.
KEEP = [
    ("auto-fit", "CSS grid's repeat(auto-fit, ...)"),
    ("see fit", 'English: "as you see fit"'),
    ("fitstore", "fit's store format, which akit's store is to read and write (decision 036)"),
]

# akit's renames, applied in order, each with what it renames. A name is "fit" not inside a
# longer ASCII word: "fits" and "FITNESS" stay.
RENAMES = [
    (
        r"(?<![A-Za-z0-9])FIT(?=_)",
        "AKIT",
        "environment variables: FIT_LLM_API_KEY, FIT_STORE_PASSWORD",
    ),
    (
        r"(?<![A-Za-z0-9])Fit(?=[A-Z-])",
        "Akit",
        "header fields and the pages' globals: X-Fit-Csrf, Fit-Config-Version, "
        "Fit-Store-Password, X-Fit-Signature, window.FitAI, FitConfig, FitCluster, FitSchemaZh, ...",
    ),
    (r"(?<![A-Za-z0-9])fit(?=[te]1\.)", "akit", "token prefixes: fitt1., fite1."),
    (r"(?<![A-Za-z0-9])([Aa]) fit(?![a-z0-9])", r"\1n akit", "English's article: a fit -> an akit"),
    (
        r"(?:(?<=other )|(?<=many )|(?<=the )|(?<=several ))fits(?![a-z0-9])",
        "akits",
        "the plural, of the gateway (other fits, many fits, several fits, the fits this one "
        "manages), not of the verb",
    ),
    (
        r"(?<![A-Za-z0-9])fit(?![a-z0-9])",
        "akit",
        "the name everywhere else: the brand, titles and text in both languages, fit_* and fit-* "
        "names (cookies, storage keys, CSS classes, JSON members, the WASM ABI's names), "
        "fit.json, fit run",
    ),
]

# The notice a changed file carries, as Apache-2.0's section 4(b) asks, in its language's comment.
NOTICE = "Changed by akit: names renamed by tools/console-pages.py (decision 037); see ../CHANGES."
COMMENTS = {".js": ("// ", ""), ".css": ("/* ", " */"), ".html": ("<!-- ", " -->")}
TEXT = (".js", ".css", ".html", ".txt", ".json", ".svg")


def rename(text: str) -> str:
    kept = []

    def hold(m):
        kept.append(m.group(0))
        return f"\0{len(kept) - 1}\0"

    text = re.sub("|".join(re.escape(k) for k, _ in KEEP), hold, text)
    for pat, rep, _ in RENAMES:
        text = re.sub(pat, rep, text)
    return re.sub("\0([0-9]+)\0", lambda m: kept[int(m.group(1))], text)


def renamed(rel: str) -> bool:
    """Whether the file at rel, under web/, is one the renames apply to."""
    return not rel.startswith("vendor/") and rel.endswith(TEXT)


def notice(rel: str) -> str:
    """The notice line of the file at rel, under web/, with the path to CHANGES from its directory."""
    start, end = COMMENTS[os.path.splitext(rel)[1]]
    up = "../" * rel.count("/")
    return start + NOTICE.replace("../CHANGES", up + "../CHANGES") + end + "\n"


def transform(rel: str, data: bytes) -> bytes:
    """A file of fit's web/, at rel, as akit serves it."""
    if not renamed(rel):
        return data
    text = data.decode("utf-8")
    out = rename(text)
    if out == text:
        return data
    if rel.endswith(".html"):
        # After the doctype, which comes first.
        head, _, rest = out.partition("\n")
        return (head + "\n" + notice(rel) + rest).encode("utf-8")
    if os.path.splitext(rel)[1] in COMMENTS:
        return (notice(rel) + out).encode("utf-8")
    return out.encode("utf-8")


def changes(commit: str) -> str:
    out = [
        f"fit's workbench (app/workbench/web) and config schema at fit {commit},",
        "as akit's console serves them: copied by tools/console-pages.py, which changed them as",
        "follows (akit's decision 037). fit's licence is LICENSE, beside this file.",
        "",
        "Each text file under web/ but vendor/'s, and fit's config schema (written as schema.json),",
        "had these renames applied, in order; each file under web/ they changed starts with a",
        "notice saying so. Nothing else was changed.",
        "",
    ]
    for pat, rep, what in RENAMES:
        out.append(f"  {pat}  ->  {rep}")
        out.append(f"      {what}")
    out += ["", "These stay as fit wrote them:", ""]
    for k, why in KEEP:
        out.append(f"  {k}: {why}")
    return "\n".join(out) + "\n"


def pinned_commit() -> str:
    for line in PINS.read_text().splitlines():
        if line.startswith("fit="):
            return line[len("fit="):].strip()
    sys.exit("console-pages: no fit= line in " + str(PINS))


def listing() -> list:
    web = DEST / "web"
    return sorted(p.relative_to(web).as_posix() for p in web.rglob("*") if p.is_file())


def files_rs(commit: str, names: list) -> str:
    out = [
        "// Written by tools/console-pages.py from fit's app/workbench/web at the commit below; do not",
        "// edit. The files are fit's, under fit's licence (../../console/LICENSE), with fit's names",
        "// renamed as akit names them (../../console/CHANGES, decision 037).",
        "",
        "/// The fit commit the files were copied from: the compatibility gate's pin.",
        f'pub(crate) const FIT_COMMIT: &str = "{commit}";',
        "",
        "/// Each file: its path as served, and its content.",
        "#[rustfmt::skip]",
        "pub(crate) const FILES: &[(&str, &[u8])] = &[",
    ]
    for n in names:
        out.append(f'    ("/{n}", include_bytes!("../../console/web/{n}")),')
    out.append("];")
    return "\n".join(out) + "\n"


def archive(repo: str, commit: str) -> bytes:
    return subprocess.run(
        ["git", "-C", repo, "archive", commit, "app/workbench/web", "LICENSE"],
        check=True,
        capture_output=True,
    ).stdout


def copy(data: bytes, commit: str) -> dict:
    """The console's files, by their path under DEST, from fit's archive at commit."""
    out = {}
    with tarfile.open(fileobj=BytesIO(data)) as t:
        for m in t.getmembers():
            if not m.isfile():
                continue
            body = t.extractfile(m).read()
            if m.name == "LICENSE":
                out["LICENSE"] = body
            else:
                rel = m.name[len("app/workbench/web/"):]
                out["web/" + rel] = transform(rel, body)
    out["schema.json"] = rename(SCHEMA.read_text(encoding="utf-8")).encode("utf-8")
    out["CHANGES"] = changes(commit).encode("utf-8")
    out["FIT_COMMIT"] = (commit + "\n").encode("utf-8")
    return out


def local_clone(commit: str):
    """A clone of fit that has commit: FIT_REPO, or fit beside akit's main checkout."""
    candidates = [os.environ.get("FIT_REPO", "")]
    common = subprocess.run(
        ["git", "-C", str(ROOT), "rev-parse", "--path-format=absolute", "--git-common-dir"],
        capture_output=True,
        text=True,
    )
    if common.returncode == 0:
        candidates.append(str(Path(common.stdout.strip()).parent.parent / "fit"))
    for c in candidates:
        if c and os.path.isdir(c):
            ok = subprocess.run(
                ["git", "-C", c, "cat-file", "-e", commit + "^{commit}"], capture_output=True
            )
            if ok.returncode == 0:
                return c
    return None


def check() -> int:
    """The tree is the renamed copy: files.rs lists the files, the commit is the pin, every text
    file is renamed (renaming it again changes nothing), schema.json is fit's schema at the pin
    renamed, CHANGES lists RENAMES and KEEP; and, given a clone of fit, every file is fit's
    renamed, byte for byte."""
    commit = (DEST / "FIT_COMMIT").read_text().strip()
    problems = []
    if FILES_RS.read_text() != files_rs(commit, listing()):
        problems.append("files.rs does not list the files copied")
    if commit != pinned_commit():
        problems.append(f"the pages are fit {commit}, the gate pins {pinned_commit()}")
    for rel in listing():
        if renamed(rel):
            text = (DEST / "web" / rel).read_text(encoding="utf-8")
            if rename(text) != text:
                problems.append(f"web/{rel} holds a name of fit's the renames change")
    if (DEST / "schema.json").read_text(encoding="utf-8") != rename(
        SCHEMA.read_text(encoding="utf-8")
    ):
        problems.append("schema.json is not fit's schema at the pin, renamed")
    if (DEST / "CHANGES").read_text(encoding="utf-8") != changes(commit):
        problems.append("CHANGES does not list the renames")
    compared = "not compared with fit's files (no clone of fit has the commit; set FIT_REPO)"
    repo = local_clone(commit)
    if repo and not problems:
        want = copy(archive(repo, commit), commit)
        have = {
            p.relative_to(DEST).as_posix(): p.read_bytes() for p in DEST.rglob("*") if p.is_file()
        }
        for rel in sorted(set(want) | set(have)):
            if want.get(rel) != have.get(rel):
                problems.append(f"{rel} is not fit's, renamed")
        compared = "the same as fit's renamed, byte for byte"
    if problems:
        for p in problems:
            print("console-pages: " + p + "; run tools/console-pages.py")
        return 1
    print(f"console pages: {len(listing())} files of fit {commit[:8]}, renamed; {compared}")
    return 0


def main() -> int:
    if sys.argv[1:] == ["--check"]:
        return check()
    commit = pinned_commit()
    repo = os.environ.get("FIT_REPO")
    with tempfile.TemporaryDirectory() as tmp:
        if not repo:
            repo = os.path.join(tmp, "fit")
            subprocess.run(
                ["git", "clone", "-q", "--no-checkout", "https://github.com/lesismal/fit", repo],
                check=True,
            )
        files = copy(archive(repo, commit), commit)
    if DEST.exists():
        for p in sorted(DEST.rglob("*"), reverse=True):
            if p.is_file():
                p.unlink()
            else:
                p.rmdir()
    for rel, body in files.items():
        target = DEST / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(body)
    FILES_RS.write_text(files_rs(commit, listing()))
    print(f"console pages: {len(listing())} files of fit {commit[:8]} copied and renamed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
