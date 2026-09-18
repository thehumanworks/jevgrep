#!/usr/bin/env python3
"""Check the canonical Rust pin and print values used by shell/CI helpers."""

import argparse
import re
import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:
    sys.exit("quality checks require Python 3.11+ (tomllib)")

ROOT = Path(__file__).resolve().parents[1]


def read_toml(path):
    with path.open("rb") as source:
        return tomllib.load(source)


def check_pins(root=ROOT):
    toolchain = read_toml(root / "rust-toolchain.toml")["toolchain"]
    cargo = read_toml(root / "Cargo.toml")
    mise = read_toml(root / "mise.toml")
    pin = toolchain["channel"]
    if not re.fullmatch(r"\d+\.\d+\.\d+", pin):
        raise ValueError(f"Rust toolchain must be an exact X.Y.Z pin, got {pin!r}")
    for label, value in (
        ("Cargo.toml package.rust-version", cargo["package"]["rust-version"]),
        ("mise.toml tools.rust", mise["tools"]["rust"]),
    ):
        if value != pin:
            raise ValueError(f"{label} is {value!r}; expected {pin!r}")
    if not {"rustfmt", "clippy"}.issubset(toolchain.get("components", [])):
        raise ValueError("rust-toolchain.toml must include rustfmt and clippy")
    if cargo["package"]["edition"] != "2021":
        raise ValueError("this migration retains Rust edition 2021")
    lock = read_toml(root / "Cargo.lock")
    versions = [p["version"] for p in lock["package"] if p["name"] == "jevgrep"]
    if versions != [cargo["package"]["version"]]:
        raise ValueError("Cargo.lock jevgrep version does not match Cargo.toml")
    return pin


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--print-rust", action="store_true")
    parser.add_argument("--print-package", action="store_true")
    args = parser.parse_args()
    try:
        pin = check_pins()
        if args.print_rust:
            print(pin)
        elif args.print_package:
            print(read_toml(ROOT / "Cargo.toml")["package"]["version"])
        else:
            print(f"Rust pins and package lock agree: {pin}")
    except (OSError, KeyError, ValueError) as error:
        sys.exit(f"pin check: {error}")


if __name__ == "__main__":
    main()
