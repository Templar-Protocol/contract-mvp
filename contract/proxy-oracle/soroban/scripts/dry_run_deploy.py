#!/usr/bin/env python3
"""Dry-run deploy validation for Soroban proxy-oracle release artifacts.

Validates that optimized WASM artifacts and the release manifest are present
and internally consistent (SHA-256 matches), then records the simulated
deploy/initialize commands that would be used for a real deployment.

This script:
  - Requires no secrets, private keys, seed phrases, or live RPC endpoints.
  - Validates artifacts offline using SHA-256 integrity checks.
  - Prints the stellar contract install/invoke commands that would be used.
  - Exits 0 on success.

The ``--simulate-only`` flag on stellar CLI commands prevents any broadcast
when a real network invocation is attempted. The commands printed here are
templates; replace placeholder values before executing.

Usage:
    python3 scripts/dry_run_deploy.py \\
        --manifest <target/proxy-oracle-soroban/release-manifest.json> \\
        --root <workspace_root> \\
        --out <evidence/task-8-dry-run.txt>
"""

import argparse
import hashlib
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

from release_artifacts import ARTIFACTS

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


# ---------------------------------------------------------------------------
# Validation
# ---------------------------------------------------------------------------


def validate_artifacts(manifest: dict, root: Path) -> list[str]:
    """Return a list of error strings; empty list means all checks passed."""
    errors: list[str] = []

    for key in (artifact.manifest_key for artifact in ARTIFACTS):
        entry = manifest.get(key)
        if not entry:
            errors.append(f"[{key}] missing from manifest (schema_version {manifest.get('schema_version', '?')})")
            continue
        rel_path = entry.get("path", "")
        expected_sha = entry.get("sha256", "")
        expected_size = entry.get("optimized_size", -1)

        wasm_path = root / rel_path
        if not wasm_path.exists():
            errors.append(f"[{key}] WASM not found: {wasm_path}")
            continue

        actual_size = wasm_path.stat().st_size
        if actual_size != expected_size:
            errors.append(
                f"[{key}] size mismatch: manifest={expected_size}  actual={actual_size}"
            )

        actual_sha = sha256_file(wasm_path)
        if actual_sha != expected_sha:
            errors.append(
                f"[{key}] sha256 mismatch:\n"
                f"  manifest: {expected_sha}\n"
                f"  actual:   {actual_sha}"
            )
        else:
            print(f"  [OK] {key}: sha256 verified ({actual_sha[:16]}...)")

    return errors


# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------


def format_report(manifest: dict, _root: Path, errors: list[str]) -> str:
    lines: list[str] = []
    now = datetime.now(timezone.utc).isoformat()

    lines.append("# Proxy-Oracle Soroban Dry-Run Deploy Report")
    lines.append(f"generated: {now}")
    lines.append("")

    lines.append("## Manifest Metadata")
    lines.append(f"  git_commit:     {manifest.get('git_commit', 'unknown')}")
    lines.append(f"  stellar_cli:    {manifest.get('stellar_cli', 'unknown')}")
    lines.append(f"  rust_toolchain: {manifest.get('rust_toolchain', 'unknown')}")
    lines.append("")

    for key in (artifact.manifest_key for artifact in ARTIFACTS):
        entry = manifest.get(key, {})
        lines.append(f"## {key}")
        lines.append(f"  package:        {entry.get('package', '?')}")
        lines.append(f"  version:        {entry.get('version', '?')}")
        lines.append(f"  path:           {entry.get('path', '?')}")
        lines.append(f"  sha256:         {entry.get('sha256', '?')}")
        lines.append(f"  optimized_size: {entry.get('optimized_size', '?')} bytes")
        lines.append("")

    lines.append("## Dry-Run Deploy Commands")
    lines.append("  (--simulate-only: no broadcast, no secret material required)")
    lines.append("")
    dry = manifest.get("dry_run_commands", {})
    lines.append(f"  note: {dry.get('note', '')}")
    lines.append("")
    for cmd_key in [artifact.install_key for artifact in ARTIFACTS] + [
        "initialize_runtime",
        "initialize_governance",
    ]:
        cmd = dry.get(cmd_key, "<not set>")
        lines.append(f"  {cmd_key}:")
        lines.append(f"    {cmd}")
        lines.append("")

    lines.append("## Artifact Integrity")
    if errors:
        lines.append("  FAIL: artifact validation errors:")
        for e in errors:
            lines.append(f"    {e}")
    else:
        lines.append("  PASS: all artifact integrity checks passed")

    lines.append("")
    lines.append(f"## Result: {'PASS' if not errors else 'FAIL'}")

    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Dry-run deploy validation for proxy-oracle Soroban release artifacts."
    )
    parser.add_argument(
        "--manifest",
        required=True,
        help="Path to release-manifest.json",
    )
    parser.add_argument(
        "--root",
        required=True,
        help="Workspace root directory (used to resolve relative WASM paths)",
    )
    parser.add_argument(
        "--out",
        required=True,
        help="Output path for the dry-run evidence file",
    )
    args = parser.parse_args()

    manifest_path = Path(args.manifest).resolve()
    root = Path(args.root).resolve()
    out_path = Path(args.out).resolve()

    if not manifest_path.exists():
        print(f"ERROR: manifest not found: {manifest_path}", file=sys.stderr)
        sys.exit(1)

    manifest = json.loads(manifest_path.read_text())

    print("Validating release artifact integrity...")
    errors = validate_artifacts(manifest, root)

    report = format_report(manifest, root, errors)

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(report)

    print(report)
    print(f"Dry-run evidence written: {out_path}")

    if errors:
        print("FAIL: artifact validation errors found.", file=sys.stderr)
        sys.exit(1)

    print(
        "PASS: dry-run deploy validation complete (no broadcast, no secrets required)."
    )


if __name__ == "__main__":
    main()
