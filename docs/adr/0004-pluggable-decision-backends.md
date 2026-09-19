# ADR 0004: Pluggable decision backends

- Status: Accepted
- Date: 2026-09-19
- Baseline: jevgrep 0.2.0 plus ADRs 0001–0003
- Scope: reusable `DecisionBackend`, ChatGPT subscription transport, CLI selection and stats.
  File discovery, question wording, result selection and machine output stay shared.

## Context

`jg` asked TypeSafe's Jev model through a concrete `JevClient`. Search, triage and the CLI
were typed to that client and always printed Jev's per-token dollar price. A ChatGPT
subscription path has to evaluate the same named questions against the same JSON state and
return the same `noul` / `score` map, without making grep depend on Jev and without treating
ChatGPT usage as Jev billing.

The ChatGPT path posts to `https://chatgpt.com/backend-api/codex/responses` with a ChatGPT
subscription. It is not Codex inference. Live probes accepted `gpt-5.6-luna` with a strict
`text.format` schema returning `{answers:{...}}`. `service_tier: priority` is accepted;
`service_tier: fast` is rejected. A completed event may report `output: []` even when text
arrived on `response.output_text.delta` / `done` (or `response.output_item.done`). Requested
priority may be served as `default`.

## Decision

### Shared contract

Introduce `crate::backend::DecisionBackend: Send + Sync` with:

- `ask(&Value, &Map<String, Value>) -> Result<Map<String, Value>, DecisionError>`
- `usage() -> &Usage`
- `service_tier() -> Option<String>` (default `None`)

`DecisionError` is `JevError`. Search and path triage take `&dyn DecisionBackend`. They do not
discover files or render results. `JevClient` and `ChatGptClient` both implement the trait.
`Usage::add` and `Usage::add_retry` are crate-visible so either client can record tokens and
retries. An unsplittable `TokenLimit` on a single-line chunk is returned as an error instead of
being dropped.

### CLI

`--backend jev|chatgpt` and `$JG_BACKEND` select the provider. The default remains `jev`.
`--chatgpt-login` requires a ChatGPT backend (flag or environment) and a query: it is
login-then-search. Inference never launches Codex.

`--jobs` defaults to 32 and `--chunk-lines` to 150 on both backends (see *Latency* for how ChatGPT
uses them).
`--model` / `$JG_MODEL` remain Jev-configurable. ChatGPT is `gpt-5.6-luna` only: any other
resolved model is a usage error. An explicit `--model gpt-5.6-luna` overrides a conflicting
`$JG_MODEL`. `--base-url` / `$JG_BASE_URL` override the selected backend's default endpoint.
URL scheme/host checks are ChatGPT-only (`https`, or `http` on `localhost` / `127.0.0.1` /
`::1`); Jev keeps the ADR 0001 "no URL validation" rule. The ChatGPT HTTP client sets
`max_redirects(0)` so a redirect cannot replay the bearer token.

Help documents the ChatGPT path, credential order, and that ChatGPT scores are estimates.

### ChatGPT transport

`ChatGptConfig` carries URL, timeout, retries and pool size. The model and requested
`service_tier: priority` are fixed on the client. The wire request uses a strict JSON schema
whose document is `{answers:{...}}`. The assembler collects SSE text from
`response.output_text.delta` / `done` or `response.output_item.done` and only then validates
the accumulated JSON. `response.completed` is not sufficient by itself when `output` is empty.
`ChatGptClient::service_tier()` reports recognized served tiers, `mixed` when completed
requests differ, or `unreported` when metadata is missing or unknown. The CLI prints this
value; it does not claim guaranteed priority.

Schema generation checks the property and total string budgets before sending.
Oversized schemas return `TokenLimit` so search can split its chunks. Streamed transient
server errors use bounded retries, explicit refusal events fail, and non-streaming JSON
must report a completed status before its answers are accepted.

### Latency

Luna writes its reply token by token at roughly 80 tokens a second, so a request costs what its
reply costs. Every choice below was benchmarked live against the subscription endpoint on the
httpx 0.28.1 corpus and kept only because it measurably helped
(see [chatgpt-verification.md](../chatgpt-verification.md#latency-benchmarks)):

- **Terse wire answers.** Questions travel under positional ids (`"0"`, `"1"`, ...). A `noul` is
  answered with a bare integer percentage; a `score` with `{confidence, probabilities[]}`.
  `ChatGptClient::ask` rebuilds the Jev shape locally: `noul` = p/100, `score` = the mean of the
  reported distribution, `legend` = the caller's own criteria. The model can no longer report a
  `score` that contradicts its probabilities, which previously failed whole requests. Callers and
  the `DecisionBackend` contract are unchanged; every question is still answered or the call fails.
- **`reasoning.effort: low`.** The default (`medium`) spent 360-440 reasoning tokens per chunk for
  the same verdicts. `none` is faster still but was rejected: it rated a bare export list a direct
  hit and put the wrong file first in a whole-corpus run.
- **32 lanes.** 48 concurrent requests completed without throttling or slower replies, but three
  whole-corpus runs in a row at 32-64 lanes drew retries, so the default stays at 32.
- **Spread.** `DecisionBackend::answers_sequentially()` is true for ChatGPT. When a search yields
  fewer chunks than three quarters of `--jobs`, search re-chunks finer (both the line and token
  caps scale, floor one quarter) so idle lanes shorten every request. Searches that already fill
  the lanes keep full chunks, which cost fewer input tokens. Jev requests are untouched.
- **Batched parallel reads** (both backends). Files are read and chunked in batches of 6 on up to
  8 threads before any request is sent.

Rejected on measurement: a bare ordered array of answers (fast, but the model zeroed every line),
sparse "hits only" replies (unstable length, and it breaks the every-question-answered rule), and
decimal rather than integer probabilities (about 10% more output tokens).

Default URL: `https://chatgpt.com/backend-api/codex/responses`. Tests and compatible mocks use
`--base-url` on loopback HTTP.

### Authentication

Resolution never starts a login:

1. A complete `CHATGPT_ACCOUNT_ID` + `CHATGPT_ACCESS_TOKEN` pair.
2. `$XDG_CONFIG_HOME/auth.toml` when that XDG path is absolute, else `~/.config/auth.toml`.
3. `$CODEX_HOME/auth.json` or `~/.codex/auth.json`.

TOML accepts uppercase top-level keys, lowercase top-level keys, `[chatgpt]`, or `[tokens]`.
Codex JSON uses `tokens.account_id` / `tokens.access_token`. A partial or malformed selected
pair fails; sources are not mixed. API keys and refresh tokens are not subscription
credentials. `--chatgpt-login` runs `codex login --device-auth` with
`cli_auth_credentials_store="file"`, routes Codex stdout to stderr, and reads the new cache
directly so stale environment or TOML cannot hide a successful login. Expiry is the server's
job; HTTP 401/403 must be actionable (`jg --chatgpt-login --backend chatgpt '<query>'`) and
must not echo tokens, raw parser errors, or request source.

### Output and stats

JSON, text and flat rows keep the existing Jev schema. ChatGPT values in those columns are
uncalibrated estimates. Jev stats stay `tokens (~$X.XXXX)`. ChatGPT stats are:

`files, requests, input / output tokens (ChatGPT subscription; requested priority, served TIER), seconds`

with no dollar figure. Auth errors print the backend message; other failures say `TypeSafe` or
`ChatGPT` according to `--backend`.

## Alternatives

Keeping search wired to `JevClient` would force ChatGPT to impersonate Jev's HTTP JSON
envelope. Embedding Codex as the inference runtime would use a different product than the
subscription Responses endpoint. Printing Jev's `$0.042` / MTok on ChatGPT would misstate
subscription billing. Auto-login on a missing cache would start an interactive Codex flow
during ordinary searches.

## Consequences

The public `Args` struct gains `backend` and `chatgpt_login`. Parse-time jobs become optional
so the backend can supply 8 vs 32. Existing help/error fixtures and environment-read unit
tests must account for `JG_BACKEND` and the new flags. Binary size grows with `toml` and the
ChatGPT client. Users can share one grep pipeline across providers; ChatGPT calibration and
served tier are not Jev's.

This work warrants a future **minor release (0.3.0)** under the pre-1.0 policy. It does not
bump the package version, tag, or publish.

## Tests and verification

- Parser: `src/cli/args.rs` backend defaults, `JG_BACKEND`, fixed ChatGPT model, login
  requires ChatGPT.
- Auth: `src/chatgpt_auth.rs` injected env/files/login runner; no global environment mutation.
- Transport: `src/chatgpt.rs` local unit tests owned by that module.
- CLI integration: [`tests/chatgpt_cli.rs`](../../tests/chatgpt_cli.rs) against a local
  listener that records `Authorization` / `ChatGPT-Account-Id` and serves SSE, including the
  empty-`output` completed pattern. Homes are isolated; credentials are fake; `CODEX_HOME` is
  set. `--chatgpt-login` uses a fake `codex` whose stdout marker must appear only on stderr.
  `cargo +1.98.1 test --test chatgpt_cli` is the targeted gate. Live ChatGPT calls are out of
  scope for this suite.

## Interfaces

- `DecisionBackend::{ask, usage, service_tier}`
- `ChatGptClient::new(&ChatGptCredentials, ChatGptConfig)`, `with_transport`, `ask`,
  `set_debug_reporter`, `service_tier`
- `resolve_credentials`, `device_login`
- CLI: `--backend`, `--chatgpt-login`, backend-aware stats and errors
