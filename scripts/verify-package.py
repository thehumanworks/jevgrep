#!/usr/bin/env python3
"""Verify and exercise a downloaded release archive, without external API access."""

import argparse
import hashlib
import os
import re
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

import tomllib

ROOT = Path(__file__).resolve().parents[1]
TARGETS = (
    "aarch64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
)


def verify_checksum(archive):
    sidecar = Path(str(archive) + ".sha256").read_text().strip()
    match = re.fullmatch(r"([0-9a-f]{64}) [ *]([^\n]+)", sidecar)
    if not match or match[2] != archive.name:
        raise ValueError("malformed checksum sidecar or wrong archive name")
    with archive.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    if digest != match[1]:
        raise ValueError("archive SHA256 mismatch")


def extract_package(archive, destination, name):
    """Extract only the contract's two regular files; never follow archive links."""
    expected = {name, f"{name}/jg", f"{name}/README.md"}
    with tarfile.open(archive, "r:gz") as source:
        members = source.getmembers()
        names = [member.name.rstrip("/") for member in members]
        if len(names) != len(expected) or set(names) != expected:
            raise ValueError(
                "archive must contain exactly its directory, jg, and README.md"
            )
        for member in members:
            member_name = member.name.rstrip("/")
            if member_name == name:
                if not member.isdir():
                    raise ValueError("archive root must be a directory")
                continue
            if not member.isfile() or member.size > 100 * 1024 * 1024:
                raise ValueError("package members must be bounded regular files")
            if member_name.endswith("/jg") and not member.mode & 0o111:
                raise ValueError("packaged jg is not executable")
        folder = destination / name
        folder.mkdir()
        for leaf in ("jg", "README.md"):
            with source.extractfile(f"{name}/{leaf}") as stream:
                (folder / leaf).write_bytes(stream.read())
        (folder / "jg").chmod(0o755)
    return folder / "jg"


def check_static(binary):
    # Inspect metadata; never execute ldd against an untrusted binary.
    headers = subprocess.run(
        ["readelf", "--program-headers", "--wide", str(binary)],
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    ).stdout
    dynamic = subprocess.run(
        ["readelf", "--dynamic", "--wide", str(binary)],
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    ).stdout
    if "INTERP" in headers or "(NEEDED)" in dynamic:
        raise ValueError("Linux release binary is dynamically linked")
    if "LOAD" not in headers:
        raise ValueError("Linux release binary has no ELF load segments")


def smoke_environment(home):
    env = {
        "PATH": os.environ.get("PATH", os.defpath),
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(home / "config"),
        "XDG_CACHE_HOME": str(home / "cache"),
        "XDG_DATA_HOME": str(home / "data"),
        "XDG_STATE_HOME": str(home / "state"),
        "XDG_CONFIG_DIRS": str(home / "config-dirs"),
        "XDG_DATA_DIRS": str(home / "data-dirs"),
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_CONFIG_SYSTEM": os.devnull,
        "GIT_CONFIG_NOSYSTEM": "1",
        "JG_NO_FNOX": "1",
        "JG_BASE_URL": "http://127.0.0.1:9",
        "LANG": "C",
        "LC_ALL": "C",
        "TERM": "dumb",
        "COLUMNS": "80",
        "PYTHONUTF8": "1",
        "PYTHONDONTWRITEBYTECODE": "1",
    }
    for key in (
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CONFIG_DIRS",
        "XDG_DATA_DIRS",
    ):
        Path(env[key]).mkdir(parents=True, exist_ok=True)
    return env


def verify(archive, target):
    with (ROOT / "Cargo.toml").open("rb") as source:
        version = tomllib.load(source)["package"]["version"]
    name = f"jevgrep-v{version}-{target}"
    if archive.name != f"{name}.tar.gz":
        raise ValueError(f"expected archive {name}.tar.gz")
    verify_checksum(archive)
    with tempfile.TemporaryDirectory(prefix="jg-package-") as temporary:
        work = Path(temporary)
        binary = extract_package(archive, work, name)
        if target.endswith("-musl"):
            check_static(binary)
        env = smoke_environment(work / "home")
        for flag in ("--help", "--version"):
            result = subprocess.run(
                [str(binary), flag],
                env=env,
                cwd=work,
                check=True,
                capture_output=True,
                text=True,
                timeout=30,
            )
            if result.stderr or "\x1b" in result.stdout or not result.stdout.strip():
                raise ValueError(f"packaged {flag} must be nonempty plain stdout only")
            if flag == "--version" and result.stdout.strip() != f"jg {version}":
                raise ValueError("packaged binary version does not match Cargo.toml")
        subprocess.run(
            [
                sys.executable,
                str(ROOT / "scripts/terminal-smoke.py"),
                "--binary",
                str(binary),
            ],
            env=env,
            cwd=work,
            check=True,
            timeout=180,
        )
    print(
        f"Verified checksum, layout, help/version, fake API and linkage: {archive.name}"
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--target", choices=TARGETS, required=True)
    args = parser.parse_args()
    try:
        verify(args.archive.resolve(), args.target)
    except (OSError, ValueError, tarfile.TarError, subprocess.SubprocessError) as error:
        sys.exit(f"package verification: {error}")


if __name__ == "__main__":
    main()
