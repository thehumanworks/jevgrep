# ADR 0011: `typesafe-jev` speaks the API's types, not JSON values

- Status: Accepted
- Date: 2026-09-21
- Baseline: ADRs 0009 and 0010; `typesafe-jev` 0.1.0 on crates.io
- Scope: the public request and reply types of `crates/typesafe-jev` (0.2.0), and the one place
  `jg` meets them.

## Context

`Client::ask` took a `serde_json::Value` state and a `Map<String, Value>` of questions and
returned a `Map<String, Value>` of answers: `jg`'s internal contract, published as it was. A
caller wrote `json!({"type": "noul", ...})` from memory and read
`answers["q"]["noul"].as_f64().unwrap_or(0.0)`, where a misspelt field is an HTTP 422 at best and
a silent zero at worst. The crate documented two question types; TypeSafe's
[API reference](https://docs.typesafe.ai/api) has three (`noul`, `choice`, `score`), optional
criteria on a `noul`, and structured (`string | object | array`) instructions and descriptions.

## Decision

The crate's types are the reference's, field for field, named as TypeSafe's own SDKs name them
where Rust allows:

- Questions: `Noul` (with `NoulCriteria`, whose wire fields `true` and `false` are `yes` and `no`
  in Rust), `Choice`, `Score`; the `Question` enum, tagged by `type`; and `Questions`, the ordered
  set of one request. Constructors and chained setters build them; the fields are public.
- Answers: `NoulAnswer`, `ChoiceAnswer`, `ScoreAnswer`, the `Answer` enum, and `Response` with
  `model`, `answers` and `usage` (`TokenUsage`; the crate's running counters keep the name
  `Usage`). A score's `legend` and `probabilities` are keyed by level number (`BTreeMap<u32, _>`),
  so level 10 sorts after level 9, which the wire's string keys do not.
- `Content` is `string | object | array` (`JSONContent` in TypeSafe's SDKs). The `state` is any
  `Serialize` value: the reference allows a string, an object or an array, and a caller's own
  struct is the typed form of an object.
- Order is part of the request: questions and a choice's options go out in the order they were
  added, by way of `indexmap`, not by whether some crate enables `serde_json/preserve_order`.
- A reply is read strictly, as TypeSafe's Python SDK reads it: a missing required field or an
  unknown answer `type` is `Error::Api` naming it, because a defaulted probability is a wrong
  result that looks right. Unknown fields are ignored and token counts are optional, as in that
  SDK. `Answer` is read through a private flat struct rather than serde's internally tagged
  enums, whose buffering garbles `f64` under `serde_json/arbitrary_precision` and whose errors do
  not name the answer type.
- Every wire struct and enum is `#[non_exhaustive]` with constructors, the lesson of ADR 0010:
  the API can gain a field or a question type in a minor release of the crate.
- The client does not enforce the documented limits (255 options, 2 to 10 levels). They are the
  server's to change; it answers 422, which is now `Error::InvalidRequest`.

`serde` (with `derive`) and `indexmap` join the dependencies and the public API. Both were already
in `jg`'s build through `serde_json`, so its lockfile gains no package.

### `jg`

`DecisionBackend` stays a JSON contract: the ChatGPT and OpenAI-compatible backends build their
prompts and JSON schemas from the question documents (ADRs 0004 and 0005), and typing them is a
different change. `impl DecisionBackend for typesafe_jev::Client` reads `jg`'s questions into
`Questions` and writes the typed answers back as JSON.
`the_typed_client_sends_exactly_the_json_that_jg_built` holds the request to the bytes the old
client sent, for single-query, multi-query, filtered and path-triage requests: the wording and
order of questions are behaviour, and neither moved, so no benchmark run is needed.

## Alternatives

A generic `ask<Q: Serialize, A: DeserializeOwned>` would have typed nothing: the caller would
still invent the shapes. Keeping a raw `ask_json` beside the typed `ask` would have kept two
contracts alive in a crate whose point is one. `Vec<(String, Question)>` with hand-written map
serialization avoids `indexmap` at the price of reimplementing it, badly. A `Vec<f64>` for a
score's probabilities reads well but invents a guarantee (contiguous levels from 0) that the
reference does not give.

## Consequences

`typesafe-jev` 0.2.0 is a breaking release; its changelog has the migration. For `jg`, what is
sent is unchanged. What is accepted is narrower: a reply that lacks a required field now fails
the request with the field's name instead of scoring zero. The reference marks every one of those
fields required. The Vercel gateway's TypeSafe-compatible route (ADR 0005) documents that it
"implements the TypeSafe request and response shapes", with `model`, `answers` and `usage` in its
example reply next to a `provider_metadata` object, which is ignored like any unknown field. That
is its documentation, not a measurement: live calls are made only when the maintainer asks, and
one `jg --backend openai --model typesafe-ai/jev` run confirms it before the next `jg` release.
The fakes in `jg`'s tests and `scripts/terminal-smoke.py`, which had left fields out, now
send them. This is a change to what `jg` accepts on the wire, so it ships with a `jg` release.
