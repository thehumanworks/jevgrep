#!/usr/bin/env bash
#
# Cut a jevgrep release: bump the version, land it on main, tag it, wait for the
# GitHub Actions run that builds the binaries, and verify the published release.
#
#   scripts/release.sh patch|minor|major|X.Y.Z [--dry-run] [--skip-tests] [--yes]
#
# The version in Cargo.toml is the source of truth; the tag is vX.Y.Z. Pushing the
# tag is what triggers .github/workflows/release.yml. See the `release` skill in
# .claude/skills/release/SKILL.md for when a release is warranted and which bump to
# pick.
set -euo pipefail

TARGETS=(aarch64-apple-darwin x86_64-unknown-linux-musl aarch64-unknown-linux-musl)

die() { printf 'release: %s\n' "$*" >&2; exit 1; }
step() { printf '\n==> %s\n' "$*"; }

bump=""
dry_run=0
skip_tests=0
assume_yes=0

for arg in "$@"; do
  case "$arg" in
    --dry-run) dry_run=1 ;;
    --skip-tests) skip_tests=1 ;;
    --yes|-y) assume_yes=1 ;;
    -h|--help) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    major|minor|patch|[0-9]*) [ -n "$bump" ] && die "two versions given: $bump and $arg"; bump="$arg" ;;
    *) die "unknown argument: $arg" ;;
  esac
done
[ -n "$bump" ] || die "usage: scripts/release.sh patch|minor|major|X.Y.Z [--dry-run] [--skip-tests] [--yes]"

cd "$(git rev-parse --show-toplevel 2>/dev/null)" || die "not inside a git repository"
for tool in git gh cargo awk; do
  command -v "$tool" > /dev/null || die "$tool is not installed"
done
gh auth status > /dev/null 2>&1 || die "gh is not authenticated (run: gh auth login)"

# ---------------------------------------------------------------- preconditions
step "Checking the working tree"
branch=$(git symbolic-ref --short HEAD 2>/dev/null || echo "DETACHED")
[ "$branch" = "main" ] || die "on branch '$branch'; releases are cut from main (merge your change first)"
git diff --quiet && git diff --cached --quiet || die "working tree is dirty; commit or stash first"

git fetch --quiet origin main
git merge-base --is-ancestor origin/main HEAD ||
  die "main is behind or has diverged from origin/main; pull and rebase first"

current=$(awk -F'"' '/^version = /{print $2; exit}' Cargo.toml)
[ -n "$current" ] || die "could not read the version from Cargo.toml"

case "$bump" in
  major|minor|patch)
    IFS=. read -r major minor patch <<< "$current"
    case "$bump" in
      major) new="$((major + 1)).0.0" ;;
      minor) new="${major}.$((minor + 1)).0" ;;
      patch) new="${major}.${minor}.$((patch + 1))" ;;
    esac
    ;;
  *)
    [[ "$bump" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "'$bump' is not a X.Y.Z version"
    new="$bump"
    ;;
esac
tag="v${new}"

[ "$new" != "$current" ] || die "version is already $current"
git rev-parse -q --verify "refs/tags/$tag" > /dev/null && die "tag $tag already exists locally"
[ -z "$(git ls-remote --tags origin "refs/tags/$tag")" ] || die "tag $tag already exists on origin"

printf '\n  %s -> %s   (tag %s, commit %s)\n' "$current" "$new" "$tag" "$(git rev-parse --short HEAD)"
ahead=$(git rev-list --count origin/main..HEAD)
[ "$ahead" -gt 0 ] && printf '  %s local commit(s) will be pushed to main first\n' "$ahead"

if [ "$dry_run" -eq 0 ] && [ "$assume_yes" -eq 0 ]; then
  [ -t 0 ] || die "refusing to publish without confirmation; pass --yes (or --dry-run)"
  read -r -p "  Publish this release? [y/N] " reply
  case "$reply" in [yY]*) ;; *) die "aborted" ;; esac
fi

# Undo the version edit if anything fails before the release commit is made.
committed=0
cleanup() {
  local rc=$?
  [ $rc -ne 0 ] && [ $committed -eq 0 ] && git checkout -- Cargo.toml Cargo.lock 2> /dev/null
  return $rc
}
trap cleanup EXIT

# ------------------------------------------------------------------ bump + test
step "Setting the version to $new"
awk -v v="$new" '!done && /^version = / { print "version = \"" v "\""; done = 1; next } { print }' \
  Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml

if [ "$skip_tests" -eq 1 ]; then
  step "Refreshing Cargo.lock (tests skipped)"
  cargo check --quiet
else
  step "Running cargo test"        # also refreshes Cargo.lock with the new version
  cargo test --quiet
fi

locked=$(awk '/^name = "jevgrep"$/ { getline; print }' Cargo.lock | awk -F'"' '{print $2}')
[ "$locked" = "$new" ] || die "Cargo.lock still says '$locked'; expected $new"

if [ "$dry_run" -eq 1 ]; then
  step "Dry run: reverting the version bump, nothing was pushed"
  git checkout -- Cargo.toml Cargo.lock
  printf '  would commit, tag %s and push to origin\n' "$tag"
  exit 0
fi

# ------------------------------------------------------------- commit, tag, push
step "Committing and pushing $tag"
git add Cargo.toml Cargo.lock
git commit --quiet -m "release $tag"
committed=1
git push --quiet origin main
git tag -a "$tag" -m "jevgrep $tag"
git push --quiet origin "$tag"

# ------------------------------------------------------------------- build watch
step "Waiting for the release workflow"
run=""
for _ in $(seq 1 40); do
  run=$(gh run list --workflow release.yml --limit 20 \
    --json databaseId,headBranch --jq "[.[] | select(.headBranch == \"$tag\")][0].databaseId" 2> /dev/null || true)
  [ -n "$run" ] && [ "$run" != "null" ] && break
  sleep 3
done
[ -n "$run" ] && [ "$run" != "null" ] ||
  die "no workflow run appeared for $tag; check https://github.com/$(gh repo view --json nameWithOwner --jq .nameWithOwner)/actions"

gh run watch "$run" --exit-status --interval 15 > /dev/null ||
  die "the release build failed: $(gh run view "$run" --json url --jq .url)"

# ---------------------------------------------------------------------- verify
step "Verifying the published release"
assets=$(gh release view "$tag" --json assets --jq '.assets[].name')
for target in "${TARGETS[@]}"; do
  printf '%s\n' "$assets" | grep -qx "jevgrep-${tag}-${target}.tar.gz" ||
    die "release $tag has no asset for $target"
done
[ "$(gh release view "$tag" --json isLatest --jq .isLatest)" = "true" ] ||
  die "release $tag is not marked as the latest release"

# The end-to-end check users actually run: does mise serve the new version?
if command -v mise > /dev/null; then
  got=$(mise exec "github:$(gh repo view --json nameWithOwner --jq .nameWithOwner)@${new}" -- jg --version 2>&1 | tail -1 || true)
  if [ "$got" = "jg ${new}" ]; then
    printf '  mise exec ... -- jg --version -> %s\n' "$got"
  else
    printf '  warning: mise returned "%s", expected "jg %s" (a stale mise cache is the usual cause)\n' "$got" "$new"
  fi
fi

printf '\nReleased %s: %s\n' "$tag" "$(gh release view "$tag" --json url --jq .url)"
for target in "${TARGETS[@]}"; do printf '  jevgrep-%s-%s.tar.gz\n' "$tag" "$target"; done
