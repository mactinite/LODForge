#!/usr/bin/env python3
"""Generate THIRD-PARTY-NOTICES.txt for LODForge.

Run from the repository root with an isolated interpreter:

    python3 -I scripts/third_party_notices.py            # writes THIRD-PARTY-NOTICES.txt
    python3 -I scripts/third_party_notices.py --check    # exits 1 when the committed file is stale

The file has two parts:

1. `licenses/NOTICES-PREAMBLE.txt`, maintained by hand: the source-level components incorporated
   into LODForge (AnvilLOD, xEdit, s-hull, Terra, ...), what is *not* covered, and how the Rust
   dependency inventory below was produced.
2. The Rust dependency inventory, generated from `cargo metadata` for the Windows release target:
   every non-workspace crate with its version, declared license expression, the license(s) LODForge
   distributes it under (one choice per OR alternative, every member of an AND), authors,
   repository, and the copyright lines found in the crate's own LICENSE / COPYING / NOTICE files.
   The texts of the chosen licenses are in `licenses/<SPDX id>.txt`; the script fails when one is
   missing, so a new dependency license cannot slip through unnoticed.

Nothing here is legal advice; the inventory is what `cargo metadata` and the crates' own files say.
"""
from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PREAMBLE = ROOT / "licenses" / "NOTICES-PREAMBLE.txt"
OUT = ROOT / "THIRD-PARTY-NOTICES.txt"
LICENSE_DIR = ROOT / "licenses"
TARGET = "x86_64-pc-windows-msvc"

# Preference order when a crate offers alternatives (SPDX "OR" or the legacy "/" separator).
PREFERENCE = [
    "MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "Unicode-3.0",
    "MPL-2.0", "CC0-1.0", "Unlicense", "0BSD", "MIT-0", "BSL-1.0",
    "Apache-2.0 WITH LLVM-exception", "LGPL-2.1-or-later",
]
# Texts we ship for an identifier (an expression "X WITH Y" needs X and the exception Y).
TEXT_FILES = {
    "MIT": ["MIT.txt"], "Apache-2.0": ["Apache-2.0.txt"], "BSD-2-Clause": ["BSD-2-Clause.txt"],
    "BSD-3-Clause": ["BSD-3-Clause.txt"], "ISC": ["ISC.txt"], "Zlib": ["Zlib.txt"],
    "Unicode-3.0": ["Unicode-3.0.txt"], "MPL-2.0": ["MPL-2.0.txt"], "CC0-1.0": ["CC0-1.0.txt"],
    "Apache-2.0 WITH LLVM-exception": ["Apache-2.0.txt", "LLVM-exception.txt"],
    "GPL-3.0-only": ["GPL-3.0-only.txt"],
    "BSL-1.0": ["BSL-1.0.txt"], "OFL-1.1": ["OFL-1.1.txt"], "Ubuntu-font-1.0": ["Ubuntu-font-1.0.txt"],
}
COPYRIGHT_RE = re.compile(r"copyright\s*(\(c\)|©|\d{4})", re.I)
NOTICE_GLOBS = ["LICENSE*", "LICENCE*", "COPYING*", "NOTICE*", "license*", "licence*"]


def metadata() -> dict:
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", TARGET],
        cwd=ROOT, check=True, capture_output=True, text=True, encoding="utf-8",
    ).stdout
    return json.loads(out)


def normalise(expr: str) -> list[list[str]]:
    """'(A OR B) AND C' -> [['A', 'B'], ['C']]: a list of AND groups, each a list of OR choices."""
    e = expr.replace("(", " ").replace(")", " ")
    e = re.sub(r"\s*/\s*", " OR ", e)  # legacy "MIT/Apache-2.0"
    groups = []
    for part in re.split(r"\s+AND\s+", e):
        choices = [c.strip() for c in re.split(r"\s+OR\s+", part) if c.strip()]
        if choices:
            groups.append(choices)
    return groups


def choose(expr: str) -> list[str]:
    chosen = []
    for choices in normalise(expr):
        for pref in PREFERENCE:
            if pref in choices:
                chosen.append(pref)
                break
        else:
            chosen.append(choices[0])  # unknown identifier: keep it so the missing text is reported
    return chosen


def copyright_lines(pkg_dir: Path) -> list[str]:
    lines: list[str] = []
    for pattern in NOTICE_GLOBS:
        for f in sorted(pkg_dir.glob(pattern)):
            if not f.is_file() or f.stat().st_size > 200_000:
                continue
            try:
                text = f.read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            for line in text.splitlines():
                s = line.strip().lstrip("#/*- ").strip()
                if COPYRIGHT_RE.search(s) and len(s) < 200 and s not in lines:
                    lines.append(s)
                if len(lines) >= 8:
                    return lines
    return lines


def build() -> str:
    md = metadata()
    members = set(md["workspace_members"])
    pkgs = sorted((p for p in md["packages"] if p["id"] not in members), key=lambda p: (p["name"].lower(), p["version"]))
    preamble = PREAMBLE.read_text(encoding="utf-8").rstrip() + "\n"

    needed_texts: set[str] = set()
    unknown: list[str] = []
    rows = []
    for p in pkgs:
        expr = p.get("license") or ""
        if not expr:
            lic_file = p.get("license_file") or "(no license field; see the crate's license file)"
            chosen = []
            unknown.append(f"{p['name']} {p['version']}: {lic_file}")
        else:
            chosen = choose(expr)
            for c in chosen:
                if c in TEXT_FILES:
                    needed_texts.update(TEXT_FILES[c])
                else:
                    unknown.append(f"{p['name']} {p['version']}: {c}")
        pkg_dir = Path(p["manifest_path"]).parent
        rows.append((p, expr, chosen, copyright_lines(pkg_dir)))

    missing = sorted(t for t in needed_texts if not (LICENSE_DIR / t).exists())
    if missing or unknown:
        sys.stderr.write("third_party_notices: cannot complete the inventory\n")
        for t in missing:
            sys.stderr.write(f"  missing license text: licenses/{t}\n")
        for u in unknown:
            sys.stderr.write(f"  unhandled license: {u}\n")
        sys.exit(2)

    out = [preamble, "\n", "=" * 78, "\nPART 2. RUST DEPENDENCIES (generated)\n", "=" * 78, "\n\n"]
    out.append(
        f"Generated by scripts/third_party_notices.py from `cargo metadata --locked --filter-platform {TARGET}`\n"
        f"for the workspace at Cargo.lock state; {len(pkgs)} third-party crates. Where a crate offers a choice of\n"
        "licenses, LODForge distributes it under the one named after 'used under'. The texts of every license\n"
        "named here are in licenses/<SPDX identifier>.txt. Copyright lines are quoted from each crate's own\n"
        "license or notice files where present; the crate authors as declared in its manifest follow.\n\n"
    )
    by_license: dict[str, int] = {}
    for p, expr, chosen, cr in rows:
        for c in chosen:
            by_license[c] = by_license.get(c, 0) + 1
    out.append("Summary of licenses used:\n")
    for c, n in sorted(by_license.items(), key=lambda kv: (-kv[1], kv[0])):
        out.append(f"  {n:4d}  {c}\n")
    out.append("\n")
    for p, expr, chosen, cr in rows:
        out.append(f"{p['name']} {p['version']}\n")
        out.append(f"  declared license: {expr}\n")
        out.append(f"  used under:       {' AND '.join(chosen)}\n")
        if p.get("authors"):
            out.append(f"  authors:          {', '.join(a for a in p['authors'] if a)}\n")
        if p.get("repository"):
            out.append(f"  repository:       {p['repository']}\n")
        for line in cr:
            out.append(f"  {line}\n")
        out.append("\n")
    out.append("=" * 78 + "\nLICENSE TEXTS\n" + "=" * 78 + "\n\n")
    out.append("Full texts, in licenses/:\n")
    for t in sorted(needed_texts | {"GPL-3.0-only.txt"}):
        out.append(f"  licenses/{t}\n")
    return "".join(out)


def main(argv: list[str]) -> int:
    text = build()
    if "--check" in argv:
        current = OUT.read_text(encoding="utf-8") if OUT.exists() else ""
        if current != text:
            sys.stderr.write("THIRD-PARTY-NOTICES.txt is stale; run scripts/third_party_notices.py\n")
            return 1
        print("THIRD-PARTY-NOTICES.txt is current")
        return 0
    OUT.write_text(text, encoding="utf-8", newline="\n")
    print(f"wrote {OUT.relative_to(ROOT)} ({len(text.splitlines())} lines)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
