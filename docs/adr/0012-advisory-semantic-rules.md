# 0012: Bounded advisory semantic rules

## Decision

Add an experimental `--rules RULES.md --source FILE` path to the existing binary.
Criteria are human-written labelled requirements. Reuse search's logical blocks
and decision backend `noul` questions, with local range selection and exact
snapshot evidence. No AST/index or separate lint runtime is introduced.

The first slice asks one violation proposition per criterion and block. Findings
carry the scored proposition as their reason, explicitly labelled as such; the
existing backends cannot all generate free-form causal explanations. This avoids
inventing an explanation that the provider never gave. Source paths and ranges
are chosen locally, canonical paths resolve symlink parents, and the entire
answer map is validated before accepting a chunk's findings.

Scope is explicit regular UTF-8 files, suitable for a caller's changed-file list.
Entire files provide bounded surrounding context; there is no changed-line filter
or claim of repository-wide/cross-file coverage. Rules, source bytes, decision
count, logical requests and state/question payloads have hard bounds. Serial
requests disable ordinary transient retries. OpenAI adaptation/resampling remains
bounded and its six-attempt worst case is disclosed separately from logical calls.
Skipping or failure is an explicit coverage gap, never a clean result. The
`--plan` path resolves no credentials and performs no network work.

Checks are advisory. Both complete states (`findings` / `no_findings`) exit 0;
incomplete execution or invalid input exits 2. There are no automatic fixes.
Search's existing execution, prompts and exit codes are unchanged.

## Evidence and limitations

Deterministic tests exercise the real CLI against a fake Jev as well as the
backend contract directly. They establish plumbing, not semantics. The separate
opt-in evaluation harness contains 18 labelled synthetic cases across TypeScript,
Python and Rust, including nine negative controls. Neutral fixture filenames
avoid leaking the expected answer through state.path. Live evaluation has not run
in this iteration because no explicit provider key was available; no model
accuracy is claimed and the fixed threshold is experimental.

Follow-up work should first run the pilot with approved credentials, inspect
false positives/negatives and add held-out rules/repositories. A measured context
comparison should precede AST/index dependencies or any enforcement policy.
