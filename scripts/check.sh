#!/usr/bin/env bash
# Required local and CI QA. Install tools separately with install-qa-tools.sh.
# Usage: check.sh [quality | target TARGET | verify ARCHIVE TARGET]
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
# An explicit installation is usable without changing global mise defaults.
export PATH="$root/target/qa-tools/bin:$PATH"

# Re-exec with an allowlist, not a credential denylist. Rust and explicitly selected
# compiler/cache locations survive; application/user configuration does not.
if [[ ${1:-} != --isolated ]]; then
  isolated=$(mktemp -d)
  trap 'rm -rf "$isolated"' EXIT
  for directory in home config cache data state config-dirs data-dirs runtime tmp bin; do
    mkdir -p "$isolated/$directory"
  done
  # mise shims need the original HOME to resolve tool selections. Resolve them
  # before isolation, so the check itself never loads user mise configuration.
  tools=(python3 cargo rustc)
  if [[ ${1:-quality} == quality ]]; then tools+=(actionlint shellcheck); fi
  for tool in "${tools[@]}"; do
    executable=$(command -v "$tool" || true)
    if [[ "$executable" == */mise/shims/* ]]; then
      if [[ "$tool" == python3 ]]; then
        executable=$(python3 -c 'import sys; print(sys.executable)')
      else
        executable=$(mise which "$tool")
      fi
    fi
    if [[ -n "$executable" ]]; then ln -s "$executable" "$isolated/bin/$tool"; fi
  done
  # Bash 3.2 (macOS) treats an empty array as unbound under set -u.
  # Keep a mandatory isolation setting here so optional overrides can be absent.
  preserved=("JG_NO_FNOX=1")
  for variable in CARGO_TARGET_DIR RUSTC_WRAPPER SCCACHE_DIR CC AR; do
    if [[ -n ${!variable:-} ]]; then preserved+=("$variable=${!variable}"); fi
  done
  env -i \
    "PATH=$isolated/bin:$root/target/qa-tools/bin:$PATH" \
    "HOME=$isolated/home" "TMPDIR=$isolated/tmp" \
    "XDG_CONFIG_HOME=$isolated/config" "XDG_CACHE_HOME=$isolated/cache" \
    "XDG_DATA_HOME=$isolated/data" "XDG_STATE_HOME=$isolated/state" \
    "XDG_CONFIG_DIRS=$isolated/config-dirs" "XDG_DATA_DIRS=$isolated/data-dirs" \
    "XDG_RUNTIME_DIR=$isolated/runtime" \
    "CARGO_HOME=${CARGO_HOME:-$HOME/.cargo}" \
    "RUSTUP_HOME=${RUSTUP_HOME:-$HOME/.rustup}" \
    GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1 \
    JG_BASE_URL=http://127.0.0.1:9 \
    LANG=C LC_ALL=C TERM=dumb COLUMNS=80 PYTHONUTF8=1 PYTHONDONTWRITEBYTECODE=1 \
    "${preserved[@]}" /bin/bash "$root/scripts/check.sh" --isolated "$@"
  exit
fi
shift
mode=${1:-quality}
if [[ $# -gt 0 ]]; then shift; fi
pin=$(python3 scripts/check-toolchain.py --print-rust)
step() { printf '\n==> %s\n' "$*"; }
rust_versions() {
  rustc +"$pin" --version --verbose
  cargo +"$pin" --version
}
assert_target() {
  local target=$1 expected_os expected_arch actual_arch
  case "$target" in
    aarch64-apple-darwin) expected_os=Darwin; expected_arch=aarch64 ;;
    x86_64-unknown-linux-musl) expected_os=Linux; expected_arch=x86_64 ;;
    aarch64-unknown-linux-musl) expected_os=Linux; expected_arch=aarch64 ;;
    *) echo "unsupported native target: $target" >&2; exit 2 ;;
  esac
  actual_arch=$(uname -m)
  if [[ "$actual_arch" == arm64 ]]; then actual_arch=aarch64; fi
  [[ $(uname -s) == "$expected_os" && "$actual_arch" == "$expected_arch" ]] || {
    echo "runner OS/architecture does not match $target: $(uname -sm)" >&2; exit 1;
  }
  printf 'Native runner verified: %s (%s)\n' "$target" "$(uname -sm)"
}
case "$mode" in
  quality)
    [[ $# == 0 ]] || { echo 'quality takes no arguments' >&2; exit 2; }
    step 'Pinned tools and helper checks'
    actionlint_version=$(actionlint --version)
    [[ ${actionlint_version%%$'\n'*} == 1.7.12 ]] || {
      echo 'actionlint 1.7.12 required; run scripts/install-qa-tools.sh' >&2; exit 1;
    }
    shellcheck_version=$(shellcheck --version)
    grep -qx 'version: 0.11.0' <<< "$shellcheck_version" || {
      echo 'ShellCheck 0.11.0 required; run scripts/install-qa-tools.sh' >&2; exit 1;
    }
    actionlint
    shellcheck scripts/*.sh scripts/githooks/*
    python3 -m unittest discover -s scripts -p 'test_*.py'
    git diff --check
    rust_versions
    step 'Formatting'
    cargo +"$pin" fmt --all -- --check
    step 'Clippy'
    # Lint levels are declared in Cargo.toml (ADR 0006). -D warnings is the shared fail-closed flag.
    cargo +"$pin" clippy --locked --all-targets --all-features -- -D warnings
    step 'Tests (ignored live tests stay ignored)'
    cargo +"$pin" test --locked --all-targets --all-features
    step 'Documentation tests and warnings-free documentation'
    cargo +"$pin" test --locked --doc --all-features
    RUSTDOCFLAGS='-D warnings' cargo +"$pin" doc --locked --no-deps --all-features
    step 'Host release binary and fake API / PTY smoke'
    cargo +"$pin" build --locked --release
    binary="${CARGO_TARGET_DIR:-$root/target}/release/jg"
    "$binary" --help >/dev/null
    "$binary" --version
    python3 scripts/terminal-smoke.py --binary "$binary" --pty
    ;;
  target)
    [[ $# == 1 ]] || { echo 'usage: check.sh target TARGET' >&2; exit 2; }
    target=$1
    assert_target "$target"
    rust_versions
    step 'Native helper and shell-portability tests'
    python3 -m unittest discover -s scripts -p 'test_*.py'
    step "Native integration tests: $target"
    cargo +"$pin" test --locked --all-targets --all-features --target "$target"
    step "Locked release build: $target"
    cargo +"$pin" build --locked --release --target "$target"
    scripts/package.sh "$target"
    ;;
  verify)
    [[ $# == 2 ]] || { echo 'usage: check.sh verify ARCHIVE TARGET' >&2; exit 2; }
    assert_target "$2"
    python3 scripts/verify-package.py --archive "$1" --target "$2"
    ;;
  *) echo "unknown QA mode: $mode" >&2; exit 2 ;;
esac
