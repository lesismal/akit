#!/usr/bin/env python3
"""Copies fit's media pages into akit, with akit's names (I125, decisions 037 and 040).

fit's handler media_pages serves its pages, plain HTML, CSS and JavaScript with no build step, from
handler/mediapages/web, embedded in fit's binary. akit's media_pages serves the same files with
fit's names renamed as akit names them, by the renames of tools/console-pages.py (RENAMES, KEEP),
so that a name the pages and the console share is renamed alike: this script copies them, with
fit's licence, at the fit commit the compatibility gate pins (tests/compat/fixtures/PINS), into
crates/akit-media/pages, marks each file it changed with a notice, writes the list of the changes
as crates/akit-media/pages/CHANGES, and writes crates/akit-media/src/pages/files.rs, which builds
the files into akit. hls.js, which the player page loads, is the console's copy (fit's one copy is
its workbench's too).

    python3 -I tools/media-pages.py            # copy, from FIT_REPO (a clone, or the URL)
    python3 -I tools/media-pages.py --check    # the tree is the renamed copy

FIT_REPO names a local clone of fit to read the commit from (git archive); by default fit is
cloned from GitHub into a temporary directory, which is removed. --check compares with fit's
files byte for byte only when FIT_REPO, or a clone of fit beside akit's checkout, has the commit.
"""

import importlib.util
import os
import subprocess
import sys
import tarfile
import tempfile
from io import BytesIO
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEST = ROOT / "crates/akit-media/pages"
FILES_RS = ROOT / "crates/akit-media/src/pages/files.rs"
SRC = "handler/mediapages/web/"

sys.dont_write_bytecode = True  # console-pages.py is loaded, not cached beside it
_spec = importlib.util.spec_from_file_location("console_pages", ROOT / "tools/console-pages.py")
console = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(console)

NOTICE = "Changed by akit: names renamed by tools/media-pages.py (decision 037); see ../CHANGES."


def notice(rel: str) -> str:
    start, end = console.COMMENTS[os.path.splitext(rel)[1]]
    up = "../" * rel.count("/")
    return start + NOTICE.replace("../CHANGES", up + "../CHANGES") + end + "\n"


def transform(rel: str, data: bytes) -> bytes:
    """A file of fit's web/, at rel, as akit serves it."""
    if not rel.endswith(console.TEXT):
        return data
    text = data.decode("utf-8")
    out = console.rename(text)
    if out == text:
        return data
    if rel.endswith(".html"):
        head, _, rest = out.partition("\n")
        return (head + "\n" + notice(rel) + rest).encode("utf-8")
    if os.path.splitext(rel)[1] in console.COMMENTS:
        return (notice(rel) + out).encode("utf-8")
    return out.encode("utf-8")


def changes(commit: str) -> str:
    out = [
        f"fit's media pages (handler/mediapages/web) at fit {commit},",
        "as akit's media_pages handler serves them: copied by tools/media-pages.py, which changed",
        "them as follows (akit's decision 037). fit's licence is LICENSE, beside this file.",
        "",
        "Each text file under web/ had these renames applied, in order (tools/console-pages.py's,",
        "the console's); each file they changed starts with a notice saying so. Nothing else was",
        "changed.",
        "",
    ]
    for pat, rep, what in console.RENAMES:
        out.append(f"  {pat}  ->  {rep}")
        out.append(f"      {what}")
    out += ["", "These stay as fit wrote them:", ""]
    for k, why in console.KEEP:
        out.append(f"  {k}: {why}")
    return "\n".join(out) + "\n"


def listing() -> list:
    web = DEST / "web"
    return sorted(p.relative_to(web).as_posix() for p in web.rglob("*") if p.is_file())


def files_rs(commit: str, names: list) -> str:
    out = [
        "// Written by tools/media-pages.py from fit's handler/mediapages/web at the commit below; do",
        "// not edit. The files are fit's, under fit's licence (../../pages/LICENSE), with fit's names",
        "// renamed as akit names them (../../pages/CHANGES, decision 037).",
        "",
        "/// The fit commit the files were copied from: the compatibility gate's pin.",
        f'pub const FIT_COMMIT: &str = "{commit}";',
        "",
        "/// Each file: its name under the pages' prefix, and its content.",
        "#[rustfmt::skip]",
        "pub(crate) const FILES: &[(&str, &[u8])] = &[",
    ]
    for n in names:
        out.append(f'    ("{n}", include_bytes!("../../pages/web/{n}")),')
    out.append("];")
    return "\n".join(out) + "\n"


def archive(repo: str, commit: str) -> bytes:
    return subprocess.run(
        ["git", "-C", repo, "archive", commit, SRC.rstrip("/"), "LICENSE"],
        check=True,
        capture_output=True,
    ).stdout


def copy(data: bytes, commit: str) -> dict:
    out = {}
    with tarfile.open(fileobj=BytesIO(data)) as t:
        for m in t.getmembers():
            if not m.isfile():
                continue
            body = t.extractfile(m).read()
            if m.name == "LICENSE":
                out["LICENSE"] = body
            else:
                rel = m.name[len(SRC):]
                out["web/" + rel] = transform(rel, body)
    out["CHANGES"] = changes(commit).encode("utf-8")
    out["FIT_COMMIT"] = (commit + "\n").encode("utf-8")
    return out


def check() -> int:
    commit = (DEST / "FIT_COMMIT").read_text().strip()
    problems = []
    if FILES_RS.read_text() != files_rs(commit, listing()):
        problems.append("files.rs does not list the files copied")
    if commit != console.pinned_commit():
        problems.append(f"the pages are fit {commit}, the gate pins {console.pinned_commit()}")
    for rel in listing():
        if rel.endswith(console.TEXT):
            text = (DEST / "web" / rel).read_text(encoding="utf-8")
            if console.rename(text) != text:
                problems.append(f"web/{rel} holds a name of fit's the renames change")
    if (DEST / "CHANGES").read_text(encoding="utf-8") != changes(commit):
        problems.append("CHANGES does not list the renames")
    compared = "not compared with fit's files (no clone of fit has the commit; set FIT_REPO)"
    repo = console.local_clone(commit)
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
            print("media-pages: " + p + "; run tools/media-pages.py")
        return 1
    print(f"media pages: {len(listing())} files of fit {commit[:8]}, renamed; {compared}")
    return 0


def main() -> int:
    if sys.argv[1:] == ["--check"]:
        return check()
    commit = console.pinned_commit()
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
    FILES_RS.parent.mkdir(parents=True, exist_ok=True)
    FILES_RS.write_text(files_rs(commit, listing()))
    print(f"media pages: {len(listing())} files of fit {commit[:8]} copied and renamed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
