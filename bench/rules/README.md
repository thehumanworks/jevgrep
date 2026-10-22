# Semantic rule-check pilot

These are 18 hand-labelled synthetic excerpts: TypeScript, Python and Rust, each
with a violation and a nearby negative control for naming, responsibility and
failure behavior. Each source is evaluated against all three criteria. Labels
are kept outside the model input. Excerpts need not compile; external types
represent the named interfaces. This small pilot does not establish accuracy on
production repositories, unseen rule wording or cross-file behavior.

Run only with approval to send these committed fixtures to TypeSafe and incur
usage. The harness requires `TYPESAFE_API_KEY` explicitly in its environment; it
never launches fnox or reads cached ChatGPT auth. The exact destination is
`https://api.typesafe.ai/v1/systemone`, model `jev-latest`. The key is sent as the
Authorization bearer credential; payloads contain only RULES.md criteria, the
single fixture's source and its path, and fixed checker prompts. No labels,
private project contents or runtime/session directories are read or sent.

```sh
cargo build --release
python3 bench/rules/evaluate.py --binary target/release/jg --live --output /tmp/rules-evaluation.json
```

At present this corpus makes 18 logical requests, each with a budget of 32
criterion/range decisions; each Jev request has zero automatic retries. The
whole corpus has 60 planned decisions. Check the per-file offline plans before
running after any fixture changes. There is no monetary-price guarantee.

The output retains every report and computes range-localized precision/recall:
a true positive must match both criterion label and the expected anchor line.
Additional/duplicate findings count as false positives. Precision and recall are
conditional on completed cases when any case is incomplete. Incomplete runs are
counted separately and invalidate the complete-evaluation claim; they are never
counted as clean negatives. Scores and thresholds are model-dependent. Keep the
model, prompts, fixtures, command, revision and date with any published results.
Do not tune on this set and then describe its scores as held-out accuracy.

`tests/rules.rs` uses fake backends and loopback servers to test parsing,
location/evidence fidelity, budgets and failure handling. Those tests do **not**
measure model accuracy. `scripts/test_rule_evaluation.py` tests metric arithmetic,
not inference. Normal CI never runs this live harness.

## Evidence for this iteration

Live evaluation has **not run**: no TypeSafe/OpenAI key or explicit ChatGPT pair
was present in the task environment. No semantic precision or recall is claimed.
The harness and labels are provided so a later authorized run is reproducible.
