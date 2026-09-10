#!/usr/bin/env python3
"""Build and write a deterministic release manifest for the Soroban proxy-oracle.

Reads the optimized runtime, governance, SEP-40 adapter, Pyth Lazer source, and
batcher WASM artifacts,
computes SHA-256 checksums, collects package version, git commit, stellar CLI
version, rust toolchain, and optimized sizes, then writes a JSON manifest to:

    target/proxy-oracle-soroban/release-manifest.json

Usage:
    python3 scripts/make_release_manifest.py \\
        --root <workspace_root> \\
        --wasm-dir <dir holding <package>.optimized.wasm per artifact> \\
        --out <target/proxy-oracle-soroban/release-manifest.json>
"""

import argparse
import hashlib
import json
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

from release_artifacts import ARTIFACTS

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def sha256_file(path: Path) -> str:
    """Return the SHA-256 hex digest of a file."""
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def run(args: list[str], cwd: str | None = None) -> str:
    """Run a command and return stripped stdout, or '' on failure."""
    try:
        result = subprocess.run(
            args,
            cwd=cwd,
            capture_output=True,
            text=True,
            timeout=30,
        )
        return result.stdout.strip()
    except Exception:
        return ""


def get_git_commit(root: Path) -> str:
    """Return the current HEAD commit hash (short), or 'unknown'."""
    out = run(["git", "rev-parse", "--short", "HEAD"], cwd=str(root))
    return out if out else "unknown"


def get_git_commit_full(root: Path) -> str:
    """Return the full HEAD commit hash, or 'unknown'."""
    out = run(["git", "rev-parse", "HEAD"], cwd=str(root))
    return out if out else "unknown"


def get_stellar_cli_version() -> str:
    """Return the stellar CLI version string, or 'unknown'."""
    out = run(["stellar", "--version"])
    if out:
        # First line: "stellar 25.1.0 (...)"
        return out.splitlines()[0].strip()
    return "unknown"


def get_rust_toolchain(root: Path) -> str:
    """Return the active Rust toolchain, or 'unknown'."""
    # Check rust-toolchain.toml first
    toolchain_file = root / "rust-toolchain.toml"
    if toolchain_file.exists():
        content = toolchain_file.read_text()
        m = re.search(r'channel\s*=\s*"([^"]+)"', content)
        if m:
            return m.group(1)
    # Fall back to `rustup show active-toolchain`
    out = run(["rustup", "show", "active-toolchain"])
    if out:
        return out.splitlines()[0].strip()
    # Fall back to `rustc --version`
    out = run(["rustc", "--version"])
    return out if out else "unknown"


def package_versions(root: Path) -> dict[str, str]:
    """Map every workspace package name to its version via one `cargo metadata` call."""
    out = run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=str(root),
    )
    if not out:
        return {}
    try:
        meta = json.loads(out)
    except Exception:
        return {}
    return {pkg.get("name"): pkg.get("version", "unknown") for pkg in meta.get("packages", [])}


def build_deploy_command(
    wasm_path: str,
    network: str = "<network>",
    source: str = "<source-identity>",
) -> str:
    """Return the stellar contract install command (dry-run template, no broadcast)."""
    return (
        f"stellar contract install"
        f" --wasm {wasm_path}"
        f" --network {network}"
        f" --source {source}"
        f" --simulate-only"
    )


def build_initialize_command(
    contract_id: str = "<contract-id>",
    governance_id: str = "<governance-contract-id>",
    network: str = "<network>",
    source: str = "<source-identity>",
) -> str:
    """Return a template stellar contract invoke initialize command."""
    return (
        f"stellar contract invoke"
        f" --id {contract_id}"
        f" --network {network}"
        f" --source {source}"
        f" --simulate-only"
        f" -- initialize"
        f" --governance_id {governance_id}"
    )


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Write a deterministic release manifest for proxy-oracle Soroban WASMs."
    )
    parser.add_argument("--root", required=True, help="Workspace root directory")
    parser.add_argument(
        "--wasm-dir", required=True, help="Directory holding <package>.optimized.wasm per artifact"
    )
    parser.add_argument(
        "--rust-toolchain",
        help="Rust toolchain used to build the WASM artifacts",
    )
    parser.add_argument(
        "--out", required=True, help="Output path for release-manifest.json"
    )
    args = parser.parse_args()

    root = Path(args.root).resolve()
    wasm_dir = Path(args.wasm_dir).resolve()
    out_path = Path(args.out).resolve()

    paths = {artifact.manifest_key: wasm_dir / artifact.optimized_wasm for artifact in ARTIFACTS}
    errors = [f"{key} not found: {path}" for key, path in paths.items() if not path.exists()]
    if errors:
        for e in errors:
            print(f"ERROR: {e}", file=sys.stderr)
        sys.exit(1)

    versions = package_versions(root)

    def rel_or_abs(p: Path) -> str:
        try:
            return str(p.relative_to(root))
        except ValueError:
            return str(p)

    manifest = {
        "schema_version": "3",
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "git_commit": get_git_commit_full(root),
        "git_commit_short": get_git_commit(root),
        "stellar_cli": get_stellar_cli_version(),
        "rust_toolchain": args.rust_toolchain or get_rust_toolchain(root),
    }
    dry_run_commands = {
        "note": (
            "These commands use --simulate-only and do not broadcast. "
            "Replace <network>, <source-identity>, <contract-id>, "
            "<governance-contract-id>, <owner>, <parent-oracle-id>, "
            "<asset>, <decimals>, <resolution>, <base> with real values "
            "for actual deployment."
        ),
    }
    for artifact in ARTIFACTS:
        path = paths[artifact.manifest_key]
        rel = rel_or_abs(path)
        manifest[artifact.manifest_key] = {
            "package": artifact.package,
            "version": versions.get(artifact.package, "unknown"),
            "path": rel,
            "sha256": sha256_file(path),
            "optimized_size": path.stat().st_size,
        }
        dry_run_commands[artifact.install_key] = build_deploy_command(rel)
    dry_run_commands["initialize_runtime"] = build_initialize_command()
    dry_run_commands["initialize_governance"] = build_initialize_command()
    manifest["dry_run_commands"] = dry_run_commands

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(manifest, indent=2) + "\n")

    print(f"Release manifest written: {out_path}")
    print(f"  git_commit:      {manifest['git_commit']}")
    print(f"  stellar_cli:     {manifest['stellar_cli']}")
    print(f"  rust_toolchain:  {manifest['rust_toolchain']}")
    for artifact in ARTIFACTS:
        entry = manifest[artifact.manifest_key]
        print(f"  {artifact.manifest_key:<20} sha256: {entry['sha256']}  ({entry['optimized_size']} bytes)")


if __name__ == "__main__":
    main()
