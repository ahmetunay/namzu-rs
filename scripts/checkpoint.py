#!/usr/bin/env python3
"""Snapshot only committed source; prove bundle restore. This is a LOCAL copy.

Usage: python3 scripts/checkpoint.py
Never archives untracked files, target, environment or credentials. Remote
backup must be configured separately. Refuses a dirty tracked working tree.
"""
import gzip
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args], stderr=subprocess.PIPE)


def main():
    root = Path(__file__).resolve().parents[1]
    if git(root, "status", "--porcelain", "--untracked-files=no").strip():
        raise SystemExit("Commit tracked changes before checkpointing.")
    commit = git(root, "rev-parse", "HEAD").decode().strip()
    tree = git(root, "rev-parse", "HEAD^{tree}").decode().strip()
    output = root / "artifacts" / "checkpoints" / commit
    output.mkdir(parents=True, exist_ok=True)
    source = output / "namzu-rs-source.tar.gz"
    source.write_bytes(gzip.compress(git(root, "archive", "--format=tar", "--prefix=namzu-rs/", "HEAD"), mtime=0))
    bundle = output / "namzu-rs.bundle"
    if bundle.exists():
        bundle.unlink()
    git(root, "bundle", "create", str(bundle), "--all")
    git(root, "bundle", "verify", str(bundle))
    with tempfile.TemporaryDirectory(prefix="namzu-restore-") as restore:
        subprocess.run(["git", "clone", "--quiet", str(bundle), restore], check=True, capture_output=True)
        restored_tree = git(restore, "rev-parse", "HEAD^{tree}").decode().strip()
        if restored_tree != tree:
            raise SystemExit("Restored source tree differs.")
    summary = {
        "commit": commit,
        "tree": tree,
        "bundle_restore_verified": True,
        "remote_backup_verified": False,
        "files": [
            {"name": path.name, "bytes": path.stat().st_size, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
            for path in (source, bundle)
        ],
    }
    (output / "manifest.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps({"directory": str(output), **summary}, indent=2))


if __name__ == "__main__":
    main()
