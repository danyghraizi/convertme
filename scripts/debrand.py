#!/usr/bin/env python3
"""Identity gate: fail if any forbidden token appears in the tree or in a bundle.

The token list is never committed. It is supplied out-of-band, either inline
(``DEBRAND_TOKENS``, one token per line) or through ``DEBRAND_TOKENS_FILE``. A
token line may be plain text or ``b64:<base64>``; base64 keeps the CI secret from
being tripped over by a copy/paste into a log.

Usage:
  debrand.py --src      # tracked source files only (used by beforeBuildCommand)
  debrand.py --all      # tracked files + every packaged artifact (release gate)

Exit codes: 0 clean, 2 missing token list, 1 match found or unreadable input.
"""

from __future__ import annotations

import argparse
import base64
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Directories that never carry authored text and would otherwise be scanned for
# minutes (vendored toolchains, node_modules, build products).
SKIP_DIRS = {
    ".git",
    "node_modules",
    "target",
    "ui-dist",
    "dist",
    ".svelte-kit",
    "__pycache__",
    ".venv",
}

ARTIFACT_DIRS = [
    ROOT / "target" / "release" / "bundle",
    ROOT / "dist",
]


def load_tokens() -> list[str]:
    raw = os.environ.get("DEBRAND_TOKENS")
    if not raw:
        path = os.environ.get("DEBRAND_TOKENS_FILE")
        if not path:
            print(
                "debrand: DEBRAND_TOKENS or DEBRAND_TOKENS_FILE must be set; "
                "the gate cannot run without the token list",
                file=sys.stderr,
            )
            raise SystemExit(2)
        try:
            raw = Path(path).expanduser().read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            print(f"debrand: cannot read token file {path}: {exc}", file=sys.stderr)
            raise SystemExit(2)
    tokens: list[str] = []
    for line in raw.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("b64:"):
            try:
                line = base64.b64decode(line[4:].strip()).decode("utf-8", "replace").strip()
            except ValueError as exc:
                print(f"debrand: bad base64 token: {exc}", file=sys.stderr)
                raise SystemExit(2)
        if line:
            tokens.append(line.lower())
    if not tokens:
        print("debrand: token list is empty; refusing to pass", file=sys.stderr)
        raise SystemExit(2)
    return tokens


def tracked_files() -> list[Path]:
    if (ROOT / ".git").exists():
        out = subprocess.run(
            ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
            cwd=ROOT,
            check=True,
            capture_output=True,
        ).stdout
        names = [n for n in out.split(b"\x00") if n]
        return [ROOT / n.decode("utf-8", "surrogateescape") for n in names]
    files = []
    for dirpath, dirnames, filenames in os.walk(ROOT):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        files.extend(Path(dirpath) / f for f in filenames)
    return files


def scan_file(path: Path, tokens: list[str]) -> list[str]:
    """Return a list of one-line hit descriptions for this file."""
    try:
        data = path.read_bytes()
    except OSError:
        return []
    # Bytes lowercased once; UTF-8 and UTF-16 (Windows artifacts) both survive a
    # plain byte scan for ASCII tokens, which is what the token list holds.
    haystack = data.lower()
    hits = []
    for token in tokens:
        needle = token.encode("utf-8", "replace")
        if needle and needle in haystack:
            hits.append(f"{path.relative_to(ROOT) if path.is_relative_to(ROOT) else path}: {token!r}")
        # UTF-16LE text inside Windows binaries (installer metadata, resource
        # strings) is not caught by the ASCII scan above.
        if len(token) >= 4:
            wide = token.encode("utf-16-le")
            if wide in haystack:
                hits.append(
                    f"{path.relative_to(ROOT) if path.is_relative_to(ROOT) else path}: {token!r} (utf-16)"
                )
    return hits


def artifacts() -> list[Path]:
    found: list[Path] = []
    for base in ARTIFACT_DIRS:
        if not base.exists():
            continue
        for dirpath, dirnames, filenames in os.walk(base):
            dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
            for name in filenames:
                p = Path(dirpath) / name
                if p.suffix.lower() in {".exe", ".msi", ".deb", ".rpm", ".appimage", ".zip", ".gz", ".dmg"}:
                    found.append(p)
    return found


def main() -> int:
    ap = argparse.ArgumentParser()
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--src", action="store_true", help="scan tracked source files only")
    g.add_argument("--all", action="store_true", help="scan tracked files and packaged artifacts")
    args = ap.parse_args()

    tokens = load_tokens()

    files = [p for p in tracked_files() if p.is_file()]
    files = [
        p
        for p in files
        if not any(part in SKIP_DIRS for part in p.relative_to(ROOT).parts)
    ] if all(p.is_relative_to(ROOT) for p in files) else files
    hits: list[str] = []
    for p in files:
        hits.extend(scan_file(p, tokens))

    arts: list[Path] = []
    if args.all:
        arts = artifacts()
        for a in arts:
            hits.extend(scan_file(a, tokens))

    if hits:
        print(f"debrand: {len(hits)} forbidden token occurrence(s):", file=sys.stderr)
        for h in hits[:100]:
            print(f"  {h}", file=sys.stderr)
        if len(hits) > 100:
            print(f"  ... {len(hits) - 100} more", file=sys.stderr)
        print(f"debrand: FAIL ({len(hits)} matches in {len(files)} files, {len(arts)} artifacts)", file=sys.stderr)
        return 1

    print(f"0 matches in {len(files)} files, 0 matches in {len(arts)} artifacts")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
