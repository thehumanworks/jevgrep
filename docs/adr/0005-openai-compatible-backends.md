# ADR 0005: OpenAI-compatible backends

- Status: Accepted
- Date: 2026-09-19
- Baseline: jevgrep 0.2.0 plus ADRs 0001–0004
- Scope: one `DecisionBackend` for any OpenAI-compatible chat-completions API, the provider
  presets `openrouter` and `openai`, their key resolution and flags, and the provider-neutral
  answer format shared with the ChatGPT backend. File discovery, question wording, result
  selection and machine output stay shared.

## Context

ADR 0004 made the decision backend pluggable and added a ChatGPT subscription path. The next
request was OpenRouter with `inclusionai/ling-3.0-flash-fin:free` as the default model, keyed by
`OPENROUTER_API_KEY` or `--api-key`, with no dependency on fnox (a user may still wrap the call:
`fnox run -- jg ...`). It was then generalised to any OpenAI-compatible model or provider.

"OpenAI-compatible" is a family, not a standard. Hosted aggregators, single-vendor APIs and local
servers agree on `POST <root>/chat/completions` with `model` and `messages`, on
`choices[0].message.content`, and on `usage.prompt_tokens` / `completion_tokens`. They differ in
where they live, how they are keyed (local servers are not), which request fields they accept
(a strict API answers 400 to one it does not know), whether they can enforce a response schema,
whether the model accepts `temperature`, how they report errors and rate limits, and whether they
state a cost.

Live probes on 2026-09-19 (httpx 0.28.1 corpus, OpenRouter):

- `response_format: {type: json_schema}` is not ignored by a model that cannot enforce it. The
  default model's provider (Novita) answers HTTP 400, `model features structured outputs not
  support`.
- With `reasoning: {enabled: false}`, 3 of 8 chunk requests to the default model came back
  unusable: `noul` answers wrapped as `{"answer": 0}`, nested `answers` objects, or the model
  deliberating in the reply (several draft objects, then `</think>`, then the answer). Sections
  with no bearing on the query were rated a direct hit. At temperature 0 each malformed reply
  repeated on every retry.
- With reasoning left on, 8 of 8 replies were well formed and the verdicts sound, at 3-6 s and
  roughly 800-1700 reasoning tokens a request instead of 1-2 s.
- A two-file search was 22 requests with ChatGPT-style spread and 5 without. The 5-request run
  was faster overall (18 s against 32 s), drew no resamples, and found more of the right lines.
- The request `--json-schema` builds was accepted by four free models that list
  `structured_outputs` (providers Nvidia, Nex AGI, Liquid, AtlasCloud). Three returned all 95 of
  95 answers as strict JSON; a 2.6B model ran out of output budget. A search with
  `nex-agi/nex-n2.5-mini:free` took 3 requests and no retries.
- The `openai` backend pointed at `https://openrouter.ai/api/v1` with `--api-key-env` searched
  correctly while sending nothing but `model`, `messages` and `temperature`.
- The default model is not repeatable run to run even at temperature 0.

Not exercised live, for want of an account or a server: api.openai.com, a keyless local server,
and a model that refuses `temperature`. Those are covered against a local fake only.

## Decision

### Shared answer format

`src/answers.rs` holds what any text model needs to answer like Jev, moved unchanged out of
`src/chatgpt.rs`: positional wire ids, the strict response schema with its strict-mode size
budget (`budgeted_schema`), and `decode_answers`, which checks a reply against the questions asked
and rebuilds Jev's `noul` / `score` shape. `check_questions` runs the schema's question checks for
a request that sends no schema. URL vetting lives in `client.rs`: `validate_bearer_url` (https, or
http on loopback) when a key is sent, `validate_keyless_url` when none is. ChatGPT requests and
behaviour are unchanged.

### One client, providers as data

`src/openai_compat.rs` has one `ChatClient`. It speaks only the common core, and what differs
between services is data in two places.

A `Provider` preset is what is fixed about a service: display `name`, API root `base_url`,
default `model` (or none), key variable `key_var`, `extras(json_schema)` for request fields only
that service understands, and `generic`. Two presets exist:

| Preset | Root | Key variable | Default model | Extras |
| --- | --- | --- | --- | --- |
| `OPENROUTER` | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` | `inclusionai/ling-3.0-flash-fin:free` | `reasoning: {exclude: true}`, `usage: {include: true}`; with a schema, `provider: {require_parameters: true}` |
| `OPENAI` (generic) | `https://api.openai.com/v1` | `OPENAI_API_KEY` | none | none |

A generic preset stands for whatever serves the configured base URL. Away from its own root it is
named by that host in diagnostics and stats, and may go without a key. It has no default model:
model names differ everywhere and go stale, so `--backend openai` requires `--model` / `$JG_MODEL`.
Adding a named service is one `Provider` and one `BackendKind` arm; the client, the CLI execution
path and the tests' fake server do not change.

`ChatConfig` is what the user chose: `base_url`, `model`, `json_schema`, `extra_body`, plus
timeout, retries and pool size. A base URL may be an API root, as services document for OpenAI
SDKs, or the full endpoint; `endpoint()` appends `/chat/completions` when it is missing.

### Request

`model`, a system message with the task and the reply format written out, a user message with
`{state, questions}`, and `temperature: 0` so that a search is repeatable where the model allows.
Then, in order: `response_format` (strict `json_schema`) only when `json_schema` is set; the
preset's extras; the user's `extra_body`, which overrides anything before it and removes a field
given as `null`. `messages` and `stream` are jg's own and are refused in `extra_body`. Nothing is
streamed.

No schema is sent by default, because a model that cannot enforce one may reject the request and
jg cannot know which models can. `--json-schema` is the opt-in. `--extra-body` is the escape hatch
for every knob jg does not model: `reasoning_effort`, routing, sampling, `max_tokens`, a chat
template switch.

A model with fixed sampling answers 400 to `temperature`. The first such refusal is remembered for
the life of the client and the request is repeated at once without the field, so the cost is one
request (per concurrent first-wave request), not a failed search. A refusal of a request that
carried no temperature is an ordinary error, so this cannot loop.

### Reply

`message.content` is a string, or a list of typed parts of which the `text` parts are the reply.
The reply object is the last JSON object carrying `answers` after any `</think>`, so a code fence,
a preface or open deliberation does not defeat an otherwise valid reply. It then passes the same
checks as a ChatGPT reply: every id present, none extra, right types, numbers in range. Nothing is
coerced; a `noul` wrapped in an object is still wrong.

Failures fall in three classes:

- **Unusable reply** (the request worked, the reply failed the checks): asked for again at
  temperature 0.7, at most twice, without backoff. Then an error. Never a zero.
- **Transient** (connection errors, 408/409/425/429/5xx, including errors reported inside a 200):
  up to 8 retries with exponential backoff and jitter. A 429 narrows the shared concurrency gate
  and waits for `Retry-After`, or for an `X-RateLimit-Reset` given as an epoch-millisecond instant,
  capped at 60 s so a per-minute window can be outlasted. Other forms of that header are ignored.
- **Final**: 401 is an `Auth` error that names the service, its key variable and both key flags,
  or, when no key was sent, says the server wants one; 413, a context-length message or code, or
  `finish_reason: length` is a `TokenLimit`, so search splits the chunk; a spent allowance
  (`free-models-per-day`, `insufficient_quota`), a content filter, a refusal and other 4xx are
  reported as they are.

Errors are read from the common `error.message` / `code` / `type`, plus an aggregator's
`metadata.raw` and `provider_name`, because "Provider returned error" alone is not actionable. The
text is shown with the API key replaced, control characters flattened, and at most 300 characters.
Refusal text and model output are never shown. Redirects are not followed.

`answers_sequentially()` stays false: spread trades requests for latency, and requests are what
hosted services ration and local servers queue.

`DecisionBackend` gains `reported_cost_usd() -> Option<f64>` (default `None`). `ChatClient` sums
`usage.cost` where a service reports it (OpenRouter does) and is `None` where none does. jg keeps
no price list.

### Keys

In order: `--api-key KEY` (an empty value is an error, not a fallback); the variable named by
`--api-key-env VAR` (unset is an error); the preset's variable. Nothing else is consulted: no fnox,
no config file, no other service's key. With none found, a generic preset away from its own root
proceeds without an `Authorization` header, because local servers have no key; anywhere else it is
an `Auth` error before any request. `--api-key-env` exists so that another service's key can be
used under the name its own tooling gives it, without putting it on a command line that `ps` shows.

As with OpenAI's SDKs, `OPENAI_API_KEY` is sent to whatever base URL the `openai` backend is given.
A key is only ever sent over https, or http on loopback; a keyless request may use plain http to
any host, which is the caller's choice of network.

### CLI

`--backend openrouter|openai` and `$JG_BACKEND`. `--model` / `$JG_MODEL` take any model id.
`--base-url`, then `$JG_BASE_URL`, then for `openai` only `$OPENAI_BASE_URL`, then the preset's
root. `--api-key`, `--api-key-env` (mutually exclusive), `--json-schema` and `--extra-body JSON`
(or `$JG_EXTRA_BODY`) require an OpenAI-compatible backend; elsewhere each is a usage error, as
`--chatgpt-login` is outside ChatGPT. `--extra-body` must be a JSON object and is never quoted back.
Parsing never reads a key variable, and `Args` redacts the key flag's value in `Debug`.

Stats: `files, requests, input / output tokens (SERVICE MODEL[; reported cost $X.XXXX]), seconds`.
Non-auth failures say `SERVICE API error`.

## Alternatives

A client per provider: the first cut was an OpenRouter-only client, and everything but four
request fields and one default turned out not to be about OpenRouter. Presets as configuration
files: more surface than two statics need, and `--base-url` already reaches any service. A default
model for `openai`: no name stays right, and a wrong one fails every search. Sending the schema
and relying on the service to drop it: measured rejected. Negotiating the schema like
`temperature` (try, fall back on 400): every first-wave request of every run would spend a refused
request against the rate limit, where the temperature refusal only costs users of such models.
Forcing the answer through a tool call: not grammar-constrained on these providers either, and
more tokens. Disabling reasoning for speed: measured unusable above. Accepting near-miss shapes
such as `{"answer": 0}`: it would weaken the one contract every backend shares; resampling recovers
the same replies. Sending `OPENAI_API_KEY` only to api.openai.com: safer, but it breaks the
`OPENAI_API_KEY` + `OPENAI_BASE_URL` convention that compatible services document. Reusing
`resolve_api_key` with its fnox fallback: ruled out by the requirement.

## Consequences

The public `Args` struct gains `api_key`, `api_key_env`, `json_schema` and `extra_body`;
`BackendKind` gains `Openrouter` and `Openai` and `provider()`. `help.txt` changes. Result quality
is the chosen model's, and the free default is small and not repeatable run to run. Free-tier
limits make whole-repository searches impractical; paths, `-g`, `-l` and `--triage` matter more
here, and `-j` should be lowered for a small local server, where queued requests can outlive the
120 s timeout. Code is forwarded to third parties, and free providers may log prompts. Requests
are not streamed, so a service that cuts long non-streaming responses is not supported.

This is a **minor release (0.3.0)** under the pre-1.0 policy, together with ADR 0004. This work
does not bump the version, tag, or publish.

## Tests and verification

- Parser: `src/cli/args.rs` preset defaults, required model, base-URL order, flag scoping,
  `--extra-body` validation, key redaction, no credential reads while parsing.
- Client: `src/openai_compat.rs` unit tests over an injected `Transport`: presets, endpoint and
  label rules, key resolution, the common request, extras, schema, `extra_body`, the temperature
  fallback, reply extraction, resampling, rate limits, error classes, scrubbing, URL vetting.
- CLI integration: [`tests/openai_compat_cli.rs`](../../tests/openai_compat_cli.rs) runs the real
  binary against a local server as OpenRouter and as "some other service": API root and full
  endpoint, keyless, `--api-key-env`, `OPENAI_BASE_URL`, schema, extras, fixed-temperature models,
  and a fake `fnox` on `PATH` that must never run.
- Live, on request only: `fnox run -- jg --backend openrouter "<query>" <public code>`.
