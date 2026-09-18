#!/usr/bin/env bash
# Preserve the public archive contract; never publish.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
if [[ $# != 1 ]]; then echo 'usage: scripts/package.sh TARGET' >&2; exit 2; fi
target=$1
case "$target" in
  aarch64-apple-darwin|x86_64-unknown-linux-musl|aarch64-unknown-linux-musl) ;;
  *) echo "unsupported release target: $target" >&2; exit 2 ;;
esac
version=$(python3 scripts/check-toolchain.py --print-package)
name="jevgrep-v${version}-${target}"
# All generated artifacts live in ignored target/, never the source tree.
dist="$root/target/dist"
mkdir -p "$dist"
work=$(mktemp -d "$dist/.package.XXXXXX")
trap 'rm -rf "$work"' EXIT
mkdir "$work/$name"
install -m 755 "${CARGO_TARGET_DIR:-$root/target}/$target/release/jg" "$work/$name/jg"
install -m 644 README.md "$work/$name/README.md"
COPYFILE_DISABLE=1 tar -C "$work" -czf "$dist/$name.tar.gz" "$name"
(
  cd "$dist"
  if command -v sha256sum >/dev/null; then
    sha256sum "$name.tar.gz" > "$name.tar.gz.sha256"
  else
    shasum -a 256 "$name.tar.gz" > "$name.tar.gz.sha256"
  fi
)
printf '%s\n' "$name"
