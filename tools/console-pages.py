#!/usr/bin/env python3
"""Copies fit's workbench into akit, unchanged (I91, decision 036).

fit's console serves its workbench, plain HTML, CSS and JavaScript with no build step, from
app/workbench/web, embedded in fit's binary. akit's console serves the same files byte for byte:
this script copies them, with fit's licence, at the fit commit the compatibility gate pins
(tests/compat/fixtures/PINS), into crates/akit-admin/console/web, and writes
crates/akit-admin/src/console/files.rs, which builds them into akit.

    python3 -I tools/console-pages.py            # copy, from FIT_REPO (a clone, or the URL)
    python3 -I tools/console-pages.py --check    # files.rs lists exactly the files copied

FIT_REPO names a local clone of fit to read the commit from (git archive); by default fit is
cloned from GitHub into a temporary directory, which is removed.
"""

import os
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
        "// edit. The files are fit's, unchanged, under fit's licence (../../console/LICENSE).",
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


def check() -> int:
    commit = (DEST / "FIT_COMMIT").read_text().strip()
    want = files_rs(commit, listing())
    if FILES_RS.read_text() != want:
        print("console-pages: files.rs does not list the files copied; run tools/console-pages.py")
        return 1
    if commit != pinned_commit():
        print(f"console-pages: the pages are fit {commit}, the gate pins {pinned_commit()}")
        return 1
    print(f"console pages: {len(listing())} files of fit {commit[:8]}")
    return 0


def archive(repo: str, commit: str) -> bytes:
    return subprocess.run(
        ["git", "-C", repo, "archive", commit, "app/workbench/web", "LICENSE"],
        check=True,
        capture_output=True,
    ).stdout


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
        data = archive(repo, commit)
    if DEST.exists():
        for p in sorted(DEST.rglob("*"), reverse=True):
            if p.is_file():
                p.unlink()
            else:
                p.rmdir()
    (DEST / "web").mkdir(parents=True, exist_ok=True)
    with tarfile.open(fileobj=BytesIO(data)) as t:
        for m in t.getmembers():
            if not m.isfile():
                continue
            if m.name == "LICENSE":
                target = DEST / "LICENSE"
            else:
                rel = m.name[len("app/workbench/web/"):]
                target = DEST / "web" / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(t.extractfile(m).read())
    (DEST / "FIT_COMMIT").write_text(commit + "\n")
    FILES_RS.write_text(files_rs(commit, listing()))
    print(f"console pages: {len(listing())} files of fit {commit[:8]} copied")
    return 0


if __name__ == "__main__":
    sys.exit(main())
