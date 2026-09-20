# ADR 0006: Correctness-first Clippy

- Status: Accepted
- Date: 2026-09-20
- Baseline: ADR 0003 (pinned 1.98.1, `clippy --all-targets -- -D warnings`, no blanket pedantic)
- Scope: Clippy lint levels and the commands that apply them. Not a toolchain upgrade.

## Context

ADR 0003 already fails the quality gate on any Clippy warning (`-D warnings`) and forbids
`unsafe`, `dbg!`, `todo!`, and `unimplemented!`. That is a good default, but the *policy*
was implicit: `clippy::all` is warn-by-default, and correctness lints are deny-by-default
only because Clippy says so. Two things were missing.

First, a local `cargo clippy` without `-D warnings` could land a `suspicious` finding as a
warning. Official Clippy guidance treats `suspicious` as “most likely wrong”; it should not
be optional in this crate.

Second, `pedantic` and `restriction` contain lints that catch real bugs here (UTF-8 string
slices, forgotten struct fields, off-by-one ranges, leftover `mem::forget`) and many more
that do not. Enabling either group whole would drown a 7k-line CLI in style noise and
`#[allow]`s, which is what ADR 0003 rejected. Official docs say the same: cherry-pick
restriction; treat pedantic as optional and FP-prone.

Sources consulted on 2026-09-20: the [Clippy lint groups](https://doc.rust-lang.org/clippy/lints.html),
[usage](https://doc.rust-lang.org/clippy/usage.html), [configuration](https://doc.rust-lang.org/clippy/configuration.html),
and the rust-clippy README category table. `correctness` is the only deny-by-default group.
`restriction` must not be enabled as a group. Cargo `[lints.clippy]` is the stable place to
declare levels so every invocation agrees.

A probe of this tree with the extra correctness-adjacent lints showed that crate-wide
`cast_*`, `map_err_ignore`, `wildcard_enum_match_arm`, `shadow_unrelated`, and
`clone_on_ref_ptr` fire dozens of times on intentional f64 scores, user-facing remaps,
`io::ErrorKind`, and tests. Those stay off.

## Decision

Declare lint levels in `Cargo.toml` so `cargo clippy`, `scripts/check.sh`, and
`scripts/pre-commit.sh` share one policy. Keep `scripts/check.sh` passing
`clippy --locked --all-targets --all-features -- -D warnings`. Do not pass extra `-D`
lint names on the command line: that is how local, hook, and CI drift.

- `unsafe_code = "forbid"` (unchanged).
- `clippy::all = warn` (style / complexity / perf stay on; `-D warnings` fail-closes them).
- `clippy::correctness = deny` and `clippy::suspicious = deny`, stated explicitly.
- Keep denying `dbg_macro`, `todo`, `unimplemented`.
- Cherry-pick only these extra lints, each because it can hide a bug in *this* crate:

  | Lint | Group | Why here |
  | --- | --- | --- |
  | `lossy_float_literal` | restriction | Thresholds and scores are f64; `0.70` vs `0.7` must stay exact |
  | `string_slice` | restriction | `clip`, filter prefixes, and JSON extraction index `&str` |
  | `rest_pat_in_fully_bound_structs` | restriction | A new field on `Args` / `FileView` must not be ignored |
  | `range_minus_one` / `range_plus_one` | pedantic | Line ranges are inclusive 1-based |
  | `match_same_arms` | pedantic | Identical arms hide a missed glob/class case |
  | `match_wild_err_arm` | pedantic | `Err(_)` must not swallow a useful error |
  | `option_option` | pedantic | Nested options are usually a missing state |
  | `invalid_upcast_comparisons` | pedantic | Integer width mistakes |
  | `implicit_saturating_sub` | pedantic | Line arithmetic should be explicit |
  | `inconsistent_struct_constructor` | pedantic | Field order hides a swapped value |
  | `same_functions_in_if_condition` | pedantic | Duplicated predicates are often copy-paste bugs |
  | `float_cmp_const` | pedantic | Compare to a named threshold, not a bare literal |
  | `large_stack_arrays` | pedantic | Accidental huge stack buffers |
  | `mem_forget` / `empty_drop` / `get_unwrap` | restriction | Ownership / panic bugs |
  | `undocumented_unsafe_blocks` | restriction | Belt and suspenders with `unsafe` forbid |
  | `infinite_loop` | restriction | A `loop` without `break`/`return` is almost never intended |
  | `deref_by_slicing` / `as_ptr_cast_mut` / `fn_to_numeric_cast_any` | restriction | Pointer/slice accidents |
  | `unused_rounding` / `debug_assert_with_mut_call` / `suspicious_operation_groupings` / `trivial_regex` | nursery | Cheap, high-signal; no findings on the current tree |

Do **not** enable: whole `pedantic` / `restriction` / `nursery` / `cargo`; `unwrap_used` /
`expect_used` / `indexing_slicing` / `panic` (ADR 0003 keeps test unwraps); the `cast_*`
family (token estimates and scores are f64 by design); `map_err_ignore` (CLI/API remaps
are intentional); `wildcard_enum_match_arm` (`io::ErrorKind` and `serde_json::Value` grow);
`shadow_unrelated` and `clone_on_ref_ptr` (test noise); `panic_in_result_fn`
(only fired on test `Result` helpers that assert, which ADR 0003 keeps allowed).

When a newly enabled lint fires, fix the code. A local `#[allow]` needs a one-line reason
on that site. Do not weaken the Cargo table to green a PR.

## Alternatives

- Blanket `pedantic = deny`: rejected; Clippy itself says to cherry-pick, and a probe of
  this crate confirms most hits are style.
- Command-line `-D clippy::suspicious` only in CI: rejected; local `cargo clippy` would
  disagree with the gate.
- `CARGO_BUILD_WARNINGS=deny` instead of `-D warnings`: possible later; it does not replace
  stating levels in Cargo, and ADR 0003 already standardized `-D warnings`.

## Consequences

`cargo clippy` without extra flags is now fail-closed on correctness and suspicious, and on
the cherry-picked list. Style findings still need `-D warnings`, which every required
command passes. Enabling a noisy group later is a new ADR, not a drive-by Cargo edit.
No user-visible binary change; no release.

## Tests and verification

- `Cargo.toml` `[lints.clippy]` is the inventory.
- `scripts/check.sh` and `scripts/pre-commit.sh` use the same Clippy invocation.
- `scripts/test_quality.py` asserts the deny list, the shared command, and the absence of
  blanket pedantic/restriction.
- `string_slice` findings were fixed by indexing through `.get` / an ASCII ellipsis helper
  (`src/files.rs`, `src/filters.rs`, `src/openai.rs`, `src/cli/render.rs`). `match_same_arms`
  collapsed two identical `fnmatch` class arms.

References: Clippy lint groups and usage (stable docs, 2026); Cargo `[lints]` reference;
ADR 0003.
