# ChatGPT backend verification

Date: September 19, 2026 (Europe/London). See [ADR 0004](adr/0004-pluggable-decision-backends.md)
for the implemented provider and authentication contract.

## Live subscription checks

The implementation session used the existing `~/.codex/auth.json` credentials directly against
`https://chatgpt.com/backend-api/codex/responses`. No API key or Codex inference process was used.
Credential values were not printed or saved in this repository.

A minimal request using `gpt-5.6-luna`, `stream: true`, `store: false`,
`service_tier: "priority"`, and a strict `text.format` JSON schema completed successfully.
The schema included numeric minimum and maximum bounds. The completed event reported the
requested model, input/output usage, `service_tier: "default"`, and an empty `output` array.
This requires collecting the separate text events before validating the answers.

A separate request using `service_tier: "fast"` returned HTTP 400 with
`Unsupported service_tier: fast`. The implementation therefore sends `priority` and reports
the actual served tier. Accepted priority requests do not demonstrate priority delivery.

The integrated debug binary then passed a two-query search over two small synthetic Python
files using path triage and two workers. It returned the expected retry region (lines 1–7)
and timeout definition (line 1), with valid Jev-compatible JSON. Exit status was 0; the run
completed three requests with 5,170 input and 1,260 output tokens in 10.97 seconds. The
reported served tier was again `default`. This is a functional smoke check, not a benchmark.

The final host release binary is built at `target/release/jg` (3,276,688 bytes on
`x86_64-unknown-linux-gnu`). It also passed a live `--backend chatgpt --json --files`
search over the synthetic retry file using the cached Codex credentials. It returned the
expected file with relevance 0.99 and exit status 0, completing one request with 881 input
and 174 output tokens in 2.92 seconds. The served tier was `default`.

## Latency benchmarks

Measured live on September 19, 2026 against the subscription endpoint (Pro plan, served tier
`default`), on public code only: the httpx 0.28.1 corpus in `bench/corpus`. "Subset" is five
known-answer pinpoint queries from `bench/bench.py` in one run over the five files that hold the
answers (1,531 lines); quality is files ranked first / region shown / pinpoint line shown.

| Build | One file, 1 query (390 lines) | Subset, 5 queries x 5 files | Whole corpus, 1 query (23 files) |
| --- | --- | --- | --- |
| Before: Jev-shaped answers, default reasoning, 8 lanes | 29.0 s | 237.7 s, 4 requests failed, 5/5 4/5 4/5 | not run |
| Terse answers, `none`, 8 lanes | 7.2 s | 55.4 s, 5/5 5/5 5/5 | |
| + 32 lanes | | 21.8 s, 5/5 5/5 5/5 | 17.8 s, wrong file ranked first |
| + spread | 3.3-5.1 s | 20.6 s, 5/5 5/5 5/5 | |
| Shipped: terse, `low`, 32 lanes, spread | 4.7-7.1 s (9.6 s with spread off) | 27.7 s, 5/5 5/5 5/5 | 21.8 s, `_client.py` 964-999 first at 0.99 |

Per-request probes on one 150-line chunk: the old format produced about 2,300 output tokens in
29 s; keyed decimals 994 tokens in 13.2 s; keyed integer percentages 900 tokens in 11.7 s;
positional ids with integer percentages 505 tokens in 7.2 s. Reasoning accounted for only about
140 of the original tokens but delayed the first answer token from 1.1 s to 3.0 s.

Reasoning effort on the `__init__.py` chunk of the redirect query (an `__all__` list that merely
names `TooManyRedirects`), three runs each: `none` rated it a direct hit at 80-90%; `low` and
`medium` rated it tangential or relevant. `low` used 70-115 reasoning tokens, `medium` 360-440.
`minimal` is rejected by the endpoint for this model.

Chunk size on the one-file search with idle lanes: 150 lines 7.3 s, 75 lines 4.5 s, 40 lines
3.5 s, 25 lines 3.1 s, 12 lines 4.0 s with 2.5 times the input tokens. Concurrency: 8, 24 and 48
simultaneous requests all completed with a median near 6.6 s each; three whole-corpus runs back
to back then drew 29 retries. Occasional single requests take 10 s or more at any setting, and
some streams end without a completion event; both are retried or absorbed by the existing logic.

Local read-and-chunk of 579 public crate source files (2,790 chunks): 230 ms sequential, 77 ms in
batches of 6 on 8 threads, no better in batches of 32.

## Deterministic checks

Credential tests use injected environment values, files, and a login runner. CLI integration
tests use isolated homes, fake credentials, local HTTP/SSE servers, and a fake `codex` executable.
They cannot establish real subscription entitlement or a completed browser/device login.

The repository's complete `scripts/check.sh quality` gate passed with Rust 1.98.1:

- 180 Rust tests passed, including 12 ChatGPT CLI tests running in parallel; eight
  pre-existing opt-in live tests remained ignored.
- 24 Python helper tests passed, including isolation of ChatGPT environment credentials
  and `CODEX_HOME` from automated QA.
- Rust formatting, Clippy with warnings denied, actionlint, ShellCheck, documentation
  tests and warnings-free documentation passed.
- The optimized host build and all 11 fake-API/PTY scenarios passed.

The successful gate used `PATH=/home/tomas/.cargo/bin:/usr/local/bin:/usr/bin:/bin`
to resolve Rustup proxies before mise shims. The initial invocation hit mise's trust
check under the gate's isolated home; no global mise trust settings were changed.
This session did not validate macOS or musl release targets, or a real device-login
round trip; the existing Codex cache already authenticated successfully.

The implementation session coordinated Codex, Claude Opus, and Cursor Grok review and development
through the deployed MSN workspace `jev`, channel `luna-grep`. The canonical coordination log
is remote at `https://msn.rodat-human-ada.workers.dev`.

## Protocol references

The [OpenAI structured outputs guide](https://developers.openai.com/api/docs/guides/structured-outputs?api-mode=responses)
documents numeric bounds and schema budgets. The [GPT-5.6 Luna model page](https://developers.openai.com/api/docs/models/gpt-5.6-luna)
lists Responses and structured-output support. The [authentication guide](https://learn.chatgpt.com/docs/auth)
documents Codex's ChatGPT login, file cache and device flow. Subscription endpoint behavior
and the served-tier observations above were verified separately against the live backend.
