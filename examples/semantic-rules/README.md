# Check whether a preview changes its input

A preview should not silently change the caller's invoice. This small example
compares a function that mutates its input with one that changes a local copy.
Both contain the same `status = sent` assignment, so finding that assignment by
text alone does not distinguish the behaviors. [RULES.md](RULES.md) states the
contract in plain language; [invoices.py](invoices.py) is runnable Python with no
third-party dependencies.

Run these commands from the repository root, using Python 3 and the repository's
pinned Rust toolchain. This example needs the source version containing semantic
rules; older release binaries may not have `--rules`.

## See the behavior without a model

```sh
python3 examples/semantic-rules/invoices.py
```

The deterministic Python output is:

```text
preview_invoice changed input: True
preview_invoice_copy changed input: False
```

A human reviewing the contract would flag the first function's mutation and leave
the copy alone. This runtime demonstration establishes their behavior, **not**
whether a model will identify the violation accurately.

## Preview the checking scope without a provider

Build once (Cargo may download locked dependencies), then run the offline plan:

```sh
cargo build --locked
target/debug/jg --backend jev --model jev-latest \
  --rules examples/semantic-rules/RULES.md \
  --source examples/semantic-rules/invoices.py --plan
```

The plan itself needs no key and makes no provider requests. Its single JSON
report has `status: "planned"`, the explicit source path, the `PREVIEW` criterion,
one planned request with three criterion/range decisions, limits,
`coverage.completed: 0`, and an empty
`findings` array. An empty findings array here means **not evaluated**. Check
`coverage.gaps` before running inference; any gap makes the plan exit 2.

## Ask a configured provider (optional; sends source and uses quota)

Only run this after configuring a TypeSafe API key in `TYPESAFE_API_KEY` and
approving the code transmission and usage in your environment:

```sh
JG_NO_FNOX=1 target/debug/jg --backend jev --model jev-latest \
  --base-url https://api.typesafe.ai/v1/systemone \
  --rules examples/semantic-rules/RULES.md \
  --source examples/semantic-rules/invoices.py
```

This sends the example source, path, criterion and checker prompts to TypeSafe;
it does not scan other files. `JG_NO_FNOX=1` disables secret-manager fallback, so
the command requires the environment key. Other configured providers are
supported as described in the [main README](../../README.md#set-up-a-backend).

The following is a **schematic excerpt**, not an observed model result. Angle
bracket values stand for fields supplied by an actual run. A finding, if the
model emits one above the threshold, has this shape:

```json
{
  "schema_version": 1,
  "advisory": true,
  "status": "findings",
  "coverage": {
    "unit": "criterion_range",
    "planned": "<number of decisions>",
    "completed": "<number completed>",
    "gaps": [],
    "whole_repository": false
  },
  "findings": [{
    "criterion": "PREVIEW",
    "requirement": "<criterion text from RULES.md>",
    "path": "examples/semantic-rules/invoices.py",
    "start": "<1-based inclusive line number>",
    "end": "<1-based inclusive line number>",
    "evidence": "<original source lines in that range>",
    "score": "<model score from 0 to 1>",
    "reason": "<the proposition scored by the model>",
    "reason_kind": "scored_proposition",
    "advisory": true
  }]
}
```

Real numeric fields are numbers, not placeholder strings. Reports also include
scope, criteria, limits, usage and limitations. Read the evidence and reason;
the reason is a scored proposition, not a generated causal explanation. Actual
outcomes may be `findings`, `no_findings`, `incomplete`, or `error`. Completed
checks exit 0 with or without findings; incomplete/error results exit 2.
`no_findings` is not proof of compliance. The checker never applies a fix.

The Python demonstration and offline plan were verified. **Live inference was
not run for this example**, and no model accuracy is claimed. The separate
[labelled evaluation pilot](../../bench/rules/README.md) is for measuring model
quality, not required for this quickstart.
