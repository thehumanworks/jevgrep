#!/usr/bin/env bash
# Install reviewed binaries only; routine checks never download tools.
# Digests: upstream GitHub release asset metadata, reviewed 2026-09-18.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
destination=${1:-"$root/target/qa-tools/bin"}
mkdir -p "$destination"
destination=$(cd "$destination" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
os=$(uname -s | tr '[:upper:]' '[:lower:]')
arch=$(uname -m)
case "$os/$arch" in
  linux/x86_64)
    action_arch=amd64; shell_arch=x86_64
    action_sha=8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8
    shell_sha=b7af85e41cc99489dcc21d66c6d5f3685138f06d34651e6d34b42ec6d54fe6f6 ;;
  linux/aarch64|linux/arm64)
    action_arch=arm64; shell_arch=aarch64
    action_sha=325e971b6ba9bfa504672e29be93c24981eeb1c07576d730e9f7c8805afff0c6
    shell_sha=68a8133197a50beb8803f8d42f9908d1af1c5540d4bb05fdfca8c1fa47decefc ;;
  darwin/arm64|darwin/aarch64)
    action_arch=arm64; shell_arch=aarch64
    action_sha=aba9ced2dee8d27fecca3dc7feb1a7f9a52caefa1eb46f3271ea66b6e0e6953f
    shell_sha=339b930feb1ea764467013cc1f72d09cd6b869ebf1013296ba9055ab2ffbd26f ;;
  darwin/x86_64)
    action_arch=amd64; shell_arch=x86_64
    action_sha=5b44c3bc2255115c9b69e30efc0fecdf498fdb63c5d58e17084fd5f16324c644
    shell_sha=c2c15e08df0e8fbc374c335b230a7ee958c313fa5714817a59aa59f1aa594f51 ;;
  *) echo "unsupported QA tool platform: $os/$arch" >&2; exit 1 ;;
esac
fetch() {
  local url=$1 expected=$2 output=$3 actual
  curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 "$url" -o "$output"
  if command -v sha256sum >/dev/null; then
    actual=$(sha256sum "$output")
  else
    actual=$(shasum -a 256 "$output")
  fi
  [[ ${actual%% *} == "$expected" ]] || { echo "checksum mismatch: $url" >&2; exit 1; }
}
fetch "https://github.com/rhysd/actionlint/releases/download/v1.7.12/actionlint_1.7.12_${os}_${action_arch}.tar.gz" "$action_sha" "$work/actionlint.tar.gz"
fetch "https://github.com/koalaman/shellcheck/releases/download/v0.11.0/shellcheck-v0.11.0.${os}.${shell_arch}.tar.gz" "$shell_sha" "$work/shellcheck.tar.gz"
tar -xzf "$work/actionlint.tar.gz" -C "$work" actionlint
tar -xzf "$work/shellcheck.tar.gz" -C "$work" shellcheck-v0.11.0/shellcheck
install -m 755 "$work/actionlint" "$destination/actionlint"
install -m 755 "$work/shellcheck-v0.11.0/shellcheck" "$destination/shellcheck"
printf 'Installed actionlint 1.7.12 and ShellCheck 0.11.0 in %s\n' "$destination"
printf 'Run: PATH="%s:%s" scripts/check.sh\n' "$destination" "\$PATH"
