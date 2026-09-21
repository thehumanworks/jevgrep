# ADR 0005: OpenAI-compatible backend

- Status: Accepted
- Date: 2026-09-19
- Baseline: jevgrep 0.2.0 plus ADRs 0001–0004
- Scope: one `DecisionBackend`, `openai`, for any OpenAI-compatible service; its configuration by
  `OPENAI_API_KEY` / `OPENAI_BASE_URL`; the Responses API with structured outputs and the
  fallbacks from it; and the provider-neutral answer format shared with the ChatGPT backend.
  File discovery, question wording, result selection and machine output stay shared.

## Context

ADR 0004 made the decision backend pluggable and added a ChatGPT subscription path. The next
request was an OpenRouter backend keyed by an environment variable or `--api-key`, with no
dependency on fnox (a user may still wrap the call: `fnox run -- jg ...`). Two designs were built
and discarded on the way here: an OpenRouter-only client, then one chat-completions client with
per-service presets (`openrouter`, `openai`). Both were rejected for the same reason: OpenRouter
is OpenAI-compatible, so it is a base URL, not a backend, and the tool should be configured the
way every OpenAI SDK is. The Responses API with structured outputs is the preferred wire.

"OpenAI-compatible" is a family, not a standard. Services differ in whether they have a
`/responses` route at all, whether the model can enforce a response schema, whether it accepts
`temperature`, whether they want a key (local servers do not), how they report errors and rate
limits, and whether they state a cost.

Live probes on 2026-09-19 (httpx 0.28.1 corpus; OpenRouter as the service, reached only through
`OPENAI_API_KEY` and `OPENAI_BASE_URL=https://openrouter.ai/api/v1`):

- `POST /responses` with `instructions`, a string `input`, `temperature: 0`, `store: false` and a
  strict `text.format` schema is accepted. `nex-agi/nex-n2.5-mini:free` returned all 95 of 95
  answers of a chunk as strict JSON, and a search took 3 requests and no retries. Reasoning arrives
  as a separate `reasoning` output item, not inside the message.
- A schema is not ignored by a model that cannot enforce it. For
  `inclusionai/ling-3.0-flash-fin:free` the upstream (Novita) answers HTTP 400, `model features
  structured outputs not support`, on both APIs. Without the schema the same model returned 95 of
  95 valid answers over Responses. Through `jg` that is one refused request per first-wave
  request (3 retries for 3 chunks), then a normal search; `--no-schema` removes them.
- An unknown route is a 404 `Not Found`; an unknown model is a 400.
- Over chat completions with reasoning disabled, 3 of 8 replies from that model were unusable:
  `noul` answers wrapped as `{"answer": 0}`, nested `answers` objects, or deliberation in the
  reply (draft objects, then `</think>`, then the answer). At temperature 0 each malformed reply
  repeated on every retry.
- A two-file search was 22 requests with ChatGPT-style spread and 5 without; the 5-request run was
  faster overall (18 s against 32 s) and found more of the right lines.
- Free models are not repeatable run to run even at temperature 0.

Not exercised live, for want of an account or a server: api.openai.com, a keyless local server, a
service without a `/responses` route, and a model that refuses `temperature`. Those are covered
against a local fake only.

## Decision

### Shared answer format

`src/answers.rs` holds what any text model needs to answer like Jev, moved unchanged out of
`src/chatgpt.rs`: positional wire ids, the strict response schema with its strict-mode size
budget (`budgeted_schema`), and `decode_answers`, which checks a reply against the questions asked
and rebuilds Jev's `noul` / `score` shape. `check_questions` runs the schema's question checks for
a request that sends no schema. URL vetting lives in `client.rs` (`backend.rs` since ADR 0009): `validate_bearer_url` (https,
or http on loopback) when a key is sent, `validate_keyless_url` when none is. ChatGPT requests and
behaviour are unchanged.

### Configuration

`--backend openai` (or `JG_BACKEND=openai`), and then what OpenAI's SDKs take:

- **Key**: `--api-key`, else `OPENAI_API_KEY`. Nothing else is consulted: no fnox, no config file,
  no other service's variable. An empty `--api-key` is an error, not a fallback. With no key,
  api.openai.com is refused before any request; any other base URL proceeds without an
  `Authorization` header, because local servers have no key, and a 401 from it says that the
  server wants one. As with the SDKs, the key is sent to whatever base URL is configured. It is
  only ever sent over https, or http on loopback; a keyless request may use plain http to any
  host. Redirects are not followed.
- **Base URL**: `--base-url`, else `JG_BASE_URL`, else `OPENAI_BASE_URL`, else
  `https://api.openai.com/v1`. It is an API root, which offers both APIs, or a full `/responses`
  or `/chat/completions` URL, which settles on one.
- **Model**: `--model` or `JG_MODEL`, required. There is no default: model names differ between
  services and go stale, and a wrong one fails every search.

There is no per-service code and no service list. A service is named in diagnostics and stats by
the host of its base URL (`OpenAI` for api.openai.com).

### Request: Responses first, adapting once

The preferred request is `POST <root>/responses` with `model`, `instructions` (the task and the
reply format), `input` (`{state, questions}` as a string), `store: false` (the state is the user's
source code and nothing asks the service to keep it), `temperature: 0` so that a search is
repeatable where the model allows, and `text.format`: a strict `json_schema` that admits exactly
the answers asked for. Only the API's common core is sent, since a strict service refuses fields
it does not know. Nothing is streamed.

The client then adapts to what a service says it cannot do. Each refusal is recognised only when
the request carried the thing refused, is remembered for the life of the client, and is answered
by repeating the request at once in the new shape, so none can loop:

| Refusal | Recognised as | Then |
| --- | --- | --- |
| no Responses route | 404, 405 or 501 on `/responses`, when the base URL is a root | chat completions: `messages`, and `response_format` for the schema |
| no structured outputs | 400 or 422 naming the schema (`structured output`, `json_schema`, `response_format`, `text.format`, `schema`) | no schema |
| no `temperature` | 400 or 422 naming it, as models with fixed sampling give | none sent |

Every first-wave request of a run pays the refusal, since they are concurrent; a serialised probe
would instead double the latency of every search. `--no-schema` starts without a schema for a
model known to lack structured outputs, and a full endpoint URL starts on that API. A missing
model can also be a 404; chat completions then says the same and that is the error shown.

`--extra-body JSON` (or `JG_EXTRA_BODY`) is merged over the request last: it overrides anything,
and removes a field given as `null`. It is the escape hatch for every knob jg does not model:
`reasoning`, routing (`provider: {require_parameters: true}` on OpenRouter), sampling, output
limits. `input`, `messages` and `stream` are jg's own and are refused.

### Jev on Vercel AI Gateway

AI Gateway is an OpenAI-compatible base URL like any other, with one exception measured live on
2026-09-19. It lists TypeSafe's evaluation model as `typesafe-ai/jev`, under the same key as its
language models, and answers 400 for it on `/responses` and `/chat/completions`: "is an evaluation
model, not a language model. Use the evaluation generation API instead." Its documentation agrees:
evaluation "is not supported through the OpenAI-compatible ... endpoints". It offers two routes
instead. `/v1/evaluate` has its own vocabulary (`boolean` with a `probability`, no `confidence`)
and refuses a `noul` question. `/typesafe/v1/systemone` is TypeSafe's API verbatim: it took jg's
`noul` and `score` questions and returned `noul`, `score`, `confidence` and `legend` as
api.typesafe.ai does.

So when the base URL's host is `ai-gateway.vercel.sh` and the model starts with `typesafe-ai/`,
`openai::evaluation_url` names `https://ai-gateway.vercel.sh/typesafe/v1/systemone`, and the CLI
builds a `JevClient` on it with the key this backend resolved (`--api-key`, else
`OPENAI_API_KEY`; one is required). No prompt, schema or adaptation is involved, `--no-schema` and
`--extra-body` have nothing to apply to, and the answers are Jev's own calibrated ones. This is
not a per-service backend or preset: the user still configures a base URL, a key and a model, and
jg knows only that this model on this host is asked elsewhere. The URL is a constant, so the key
never follows a base URL's scheme or path.

The same probes found the route flaky: a 503 `service_unavailable_error` ("try again shortly")
within 0.3 s and without `Retry-After`, for about an eighth of one-question requests and half of
jg's, the same at 1, 4, 12 and 32 concurrent requests. With the Jev client's backoff growing to
30 s, one unlucky request became the search's tail (16 files at `-j 4`: 104.8 s). On this route
the backoff is capped at 2 s and retries raised from 8 to 16 (`Config::max_backoff`,
`max_retries`): the same search took 15.0 s, and the chance of a request running out of retries
fell from about 1 in 500 to 1 in 100,000.

### Reply

Responses: `status` must be `completed`; `incomplete` with `max_output_tokens` is a `TokenLimit`;
the reply is the `output_text` parts of `message` items, and a `refusal` part is final. Chat
completions: `choices[0].message.content`, as a string or typed parts; `finish_reason: length` is
a `TokenLimit`. Usage is read under either API's names.

The reply object is the last JSON object carrying `answers` after any `</think>`. Under a strict
schema that is the whole output; without one, a code fence, a preface or open deliberation does
not defeat an otherwise valid reply. It then passes the same checks as a ChatGPT reply: every id
present, none extra, right types, numbers in range. Nothing is coerced; a `noul` wrapped in an
object is still wrong.

Failures fall in three classes:

- **Unusable reply** (the request worked, the reply failed the checks): asked for again at
  temperature 0.7, at most twice, without backoff. Then an error. Never a zero.
- **Transient** (connection errors, 408/409/425/429/5xx, a `failed` run, errors reported inside a
  200): up to 8 retries with exponential backoff and jitter. A 429 narrows the shared concurrency
  gate and waits for `Retry-After`, or for an `X-RateLimit-Reset` given as an epoch-millisecond
  instant, capped at 60 s so a per-minute window can be outlasted.
- **Final**: 401 is an `Auth` error naming the service, `OPENAI_API_KEY` and `--api-key`; 413 or a
  context-length message or code is a `TokenLimit`, so search splits the chunk; a spent allowance
  (`free-models-per-day`, `insufficient_quota`), a content filter, a refusal and other 4xx are
  reported as they are.

Errors are read from the common `error.message` / `code` / `type` / `param`, plus an aggregator's
`metadata.raw` and `provider_name`, because "Provider returned error" alone is not actionable. The
text is shown with the API key replaced, control characters flattened, and at most 300 characters.
Refusal text and model output are never shown.

`answers_sequentially()` stays false: spread trades requests for latency, and requests are what
hosted services ration and local servers queue. `DecisionBackend` gains
`reported_cost_usd() -> Option<f64>` (default `None`): the client sums `usage.cost` where a
service states it and shows nothing where none does. jg keeps no price list.

### CLI

`--backend jev|chatgpt|openai`. `--api-key`, `--no-schema` and `--extra-body` require
`--backend openai`; elsewhere each is a usage error, as `--chatgpt-login` is outside ChatGPT.
`OPENAI_BASE_URL` and `JG_EXTRA_BODY` are read for this backend only. `--extra-body` must be a JSON
object and is never quoted back. Parsing never reads the key variable, and `Args` redacts the key
flag's value in `Debug`.

Stats: `files, requests, input / output tokens (SERVICE MODEL[; reported cost $X.XXXX]), seconds`.
Non-auth failures say `SERVICE API error`.

## Alternatives

A backend or preset per service: built twice, and everything turned out to be a base URL, a key
and a model. A flag or variable naming another key variable: `OPENAI_API_KEY` is the convention,
and a wrapper can map any secret onto it. Chat completions only: the Responses API is where
structured outputs and separated reasoning live, and what was asked for. Responses only: most
compatible services still lack the route. No schema by default: measured to make weak models
wrap, nest and leak; with a schema the reply is valid by construction where the model supports
it. A serialised capability probe: doubles latency for everyone to save a few refused requests for
some. A default model: no name stays right. Forcing the answer through a tool call: not
grammar-constrained on most services either, and more tokens. Accepting near-miss shapes such as
`{"answer": 0}`: it would weaken the one contract every backend shares. Sending the key only to
api.openai.com: safer, but it breaks the `OPENAI_API_KEY` + `OPENAI_BASE_URL` convention this
backend exists to honour. Reusing `resolve_api_key` with its fnox fallback: ruled out by the
requirement.

## Consequences

The public `Args` struct gains `api_key`, `no_schema` and `extra_body`; `BackendKind` gains
`Openai`. `help.txt` changes. Result quality is the chosen model's, and free models are not
repeatable run to run. Free-tier limits make whole-repository searches impractical; paths, `-g`,
`-l` and `--triage` matter more here, and `-j` should be lowered for a small local server, where
queued requests can outlive the 120 s timeout. Code is sent to the configured service, which may
forward it; free providers may log prompts. Requests are not streamed, so a service that cuts long
non-streaming responses is not supported.

This is a **minor release (0.3.0)** under the pre-1.0 policy, together with ADR 0004. This work
does not bump the version, tag, or publish.

## Tests and verification

- Parser: `src/cli/args.rs` required model, base-URL order (`OPENAI_BASE_URL` for this backend
  only), flag scoping, `--extra-body` validation, key redaction, no credential reads while
  parsing, `openrouter` no longer a backend.
- Client: `src/openai.rs` unit tests over an injected `Wire`: routes and labels, key resolution,
  the Responses request and its schema, the chat-completions shape, the three adaptations and
  their limits, `extra_body`, reply extraction, resampling, rate limits, error classes, scrubbing,
  URL vetting.
- Gateway: `evaluation_url` is unit-tested for host and model matching (whole host, no userinfo,
  other models and services untouched). The route itself is only checked live, as above.
- CLI integration: [`tests/openai_cli.rs`](../../tests/openai_cli.rs) runs the real binary against
  a local server speaking both APIs: environment-only configuration, fallbacks end to end, pinned
  endpoints, keyless use, and a fake `fnox` on `PATH` that must never run.
- Live, on request only, with public code:
  `OPENAI_API_KEY=... OPENAI_BASE_URL=https://openrouter.ai/api/v1 jg --backend openai --model MODEL "<query>" <path>`.
