# jevgrep (`jg`)

Natural-language grep. Ask a question about a codebase in plain language and get back the files
and line numbers that answer it.

`jg` is built on a reusable decision backend: it sends a JSON `state` plus named typed questions
and receives a Jev-compatible `answers` map (`noul` / `score`). Search, filters and rendering
are independent of the provider. The default backend is [Jev](https://typesafe.ai/blog/introducing-system-one-models-and-jev),
TypeSafe's decision model. Jev does not generate text. It returns calibrated probabilities and
evaluates every question in a request in parallel. `jg` exploits that: each file chunk becomes
one request carrying one question per line, and all requests fan out over a thread pool.
A 9,000-line codebase is scored line by line in about 2 seconds for about 1 cent.

`--backend chatgpt` (or `JG_BACKEND=chatgpt`) uses your ChatGPT subscription instead, posting
directly to `https://chatgpt.com/backend-api/codex/responses` with fixed model `gpt-5.6-luna`.
That path is not Codex inference. ChatGPT scores use the same grep JSON/text schema as Jev, but
they are generative relevance estimates, not empirically calibrated probabilities.

`--backend openai` sends the same questions to any OpenAI-compatible service, configured the way
OpenAI's own SDKs are: `OPENAI_API_KEY` and `OPENAI_BASE_URL`. api.openai.com is the default, and
[OpenRouter](https://openrouter.ai), Groq, a corporate gateway, vLLM, llama.cpp, Ollama or LM
Studio are each just a base URL, a key (or none) and a model. `jg` prefers the Responses API with
structured outputs and falls back by itself where a service lacks them. These scores are model
estimates too.

`jg` is a single native binary written in Rust, with no runtime to install. The Linux GNU
host release build with all backends is about 3.3 MB (3,334,600 bytes).
Its only dynamic dependencies are libc and libgcc, and TLS roots are compiled in.

```console
$ jg "which HTTP method is used after a 303 redirect"
httpx/_client.py  relevance=1.00
    494-508  0.96  def _redirect_method(self, request: Request, response: Response) -> str:
        502  0.91          if response.status_code == codes.SEE_OTHER and method != "HEAD":
        503  0.92              method = "GET"

jg: 23 files, 72 requests, 301,208 tokens (~$0.0127), 2.2s
```

Jev stats include a token dollar estimate. ChatGPT subscription stats do not: they report
input and output tokens, that priority was requested, and the tier the server actually served.
`openai` stats report input and output tokens, the service's host and the model, and a cost only
where the service itself states one, as OpenRouter does (`$0.0000` on a free model).

## Reading the output

Every row has the same shape: location, probability, text.

| Row | Location | Probability means |
|---|---|---|
| file header | path | `relevance`: the best evidence in the file; files are sorted by it |
| region | `494-508` | these lines contain the code you are looking for |
| line | `502` | this line directly answers the query |

Jev probabilities are calibrated: 0.9 means right about 9 times in 10. ChatGPT and OpenAI-compatible
values in the same columns are model estimates and are not calibrated that way.

Regions are logical blocks (a function, a method, a paragraph), and adjacent matching blocks are
merged. Lines are nested under the region they belong to. Two rules keep the output honest:

- **Every printed row clears `-t` on its own.** Jev scores a line alone and a block as a whole,
  and the two can disagree. A line that clears `-t` inside a block that does not is listed on
  its own, with no region row above it. Nothing rides in on a neighbour's score.
- **No source line is printed twice.** A pinpointed line that is already its region's label is
  folded into the region row, and `-C` context never repeats a row.

A narrow query pinpoints lines. A broad query has no single answering line, so it returns
regions only:

```console
$ jg "how does digest authentication work"
httpx/_auth.py  relevance=0.99
    255-299  0.79  def _build_auth_header(
```

A file is never listed without a location. When a file looks related overall but nothing in it
clears `-t`, it goes below a separator, and only with near misses of at least 0.7 x `-t` (0.35 by
default). Agents can stop reading at the separator. The flat `--no-heading` format omits this
tier entirely; `--json` marks it with `"match": "weak"`.

```console
-- weaker: these files look related overall, but nothing in them reached 0.50; near misses (>= 0.35) shown

src/search.rs  relevance=0.68
    176-238  0.40  pub fn build_request(queries: &[String], chunk: &Chunk, files_only: bool, ...
```

## Filters in plain language

```console
$ jg "where is the timeout set" --filter "Source code files only. No tests or documentation files"
jg: filter: only Source code files; not tests; not documentation files
```

`--filter` takes free text. `--only CATEGORY` and `--not CATEGORY` take one category each and skip
the parsing. All three are repeatable and combine. `jg` prints how it read the filter, and how
many matching rows the filter removed.

**How it works under the hood.** Jev is unreliable on negated or compound rules and very reliable
on atomic, positive ones, which is also what TypeSafe's docs prescribe: ask one narrow question,
phrase it so that yes is the high answer, and compose the answers in code. Measured on 350
labelled files:

| How the rule is put to Jev | Gate accuracy |
|---|---|
| One positive question per category, only/not applied in code | 95% |
| Filter text placed in `state`, referenced by path | 88% |
| Filter text embedded in one question ("does this satisfy: No tests") | 85% |
| Same, split into sentences but still negated | 71% |

Asked whether a file satisfies "No tests", ordinary source files score about 0.48. Asked whether
it falls under the category "tests", they score near 0. So `jg` parses the filter into polar
rules and asks, per category, `Does this file fall under the category: tests?` for each section
and `Do lines A-B fall under the category: tests?` for each region. A category applies if either
level says yes: sections settle file kinds (tests, docs), regions settle constructs (imports,
comments, an inline `#[cfg(test)]` module). Rows are kept when P(passes) reaches 0.5. The filter
text never enters `state`, because state is shared and there it shifted relevance scores by up to
half the scale. Filter questions do not depend on the query, so they are asked once per chunk
however many queries run, and add about 10% to the tokens.

**Phrasing that works.**

- Name kinds of thing: `tests`, `documentation files`, `imports`, `generated code`, `async code`,
  `the command line interface`. Any of "only X", "X only", "no X", "exclude X", "skip X",
  "X but not Y", and lists with commas, "and", "or" are understood.
- Be as specific as you mean. "No documentation" also drops doc comments inside source files;
  "No documentation files" drops only the files. On a doc-comment-heavy Rust crate the first kept
  85% of matching code lines, the second 100%, and both removed every README row.
- One category per idea beats a clever sentence. `--not tests --not benchmarks` is exactly what
  `--filter "no tests or benchmarks"` becomes.
- If a glob can say it (`-g "*.py"`, `-x "docs/*"`), use the glob: exact, free, and it skips the
  files before they are sent.

End to end (12 exclusion runs over two repos): of 60 rows from forbidden
kinds shown without a filter, 0 remained with it. Excluding tests or benchmarks kept 100% of the
allowed code lines; excluding "documentation" kept 73 to 88% on the Rust crate, for the reason
above.

## Install

Every [release](https://github.com/thehumanworks/jevgrep/releases) carries a prebuilt binary for
macOS on Apple Silicon (`aarch64-apple-darwin`) and Linux on x64 and aarch64
(`x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`). The Linux builds are static, so they
run on any distribution regardless of its glibc.

With [mise](https://mise.jdx.dev), pointed straight at this repo:

```bash
mise exec github:thehumanworks/jevgrep -- jg --help   # one-off run, nothing installed
mise use -g github:thehumanworks/jevgrep              # or put `jg` on PATH for good
```

Or download a tarball from the releases page and drop the binary on your PATH:

```bash
tar xzf jevgrep-v0.2.0-x86_64-unknown-linux-musl.tar.gz
install -m755 jevgrep-v0.2.0-x86_64-unknown-linux-musl/jg ~/.local/bin/jg
```

From source:

```bash
cargo install --path .      # builds the release binary and puts `jg` in ~/.cargo/bin
# or build it and copy it wherever you like:
cargo build --release && install -m755 target/release/jg ~/.local/bin/jg
```

A source build links against the system libc, so the binary can be copied to any machine with the
same OS, architecture and a libc at least as new. The static builds come from adding the musl
target (`rustup target add x86_64-unknown-linux-musl`, plus a musl C compiler for the TLS library)
and building with `--target x86_64-unknown-linux-musl`; `.github/workflows/release.yml` does
exactly that on every `v*` tag. Windows is not built or tested.

`jg` needs `TYPESAFE_API_KEY` for the default Jev backend. It reads the environment variable first, then falls back to
`fnox get TYPESAFE_API_KEY`. This repo's `fnox.toml` already provides it, so inside this repo
`jg` just works. To use `jg` in other repos, export the variable or add the secret to your global
fnox config.

The ChatGPT backend does not use that key. It needs a ChatGPT subscription pair, resolved in this
order and never mixed across sources:

1. Complete `CHATGPT_ACCOUNT_ID` and `CHATGPT_ACCESS_TOKEN` together.
2. Optional `$XDG_CONFIG_HOME/auth.toml` (must be an absolute XDG path) or `~/.config/auth.toml`.
   TOML may use those uppercase keys, lowercase `account_id` / `access_token`, or the same
   lowercase pair under `[chatgpt]` or `[tokens]`.
3. `$CODEX_HOME/auth.json` or `~/.codex/auth.json` (`tokens.account_id` / `tokens.access_token`).

A partial or invalid selected pair is an error. Normal searches never start an interactive login.
`--chatgpt-login` is explicit: it runs `codex login --device-auth` with file-backed storage,
sends Codex's stdout to stderr, then reads the new cache (stale env/TOML cannot hide a fresh
login) and continues the search. A stale Codex cache fails at the server with an actionable
`--chatgpt-login` hint. Live protocol notes and QA evidence live in
[docs/chatgpt-verification.md](docs/chatgpt-verification.md).

The `openai` backend is configured like an OpenAI SDK:

```bash
export OPENAI_API_KEY=...                                   # or --api-key KEY
export OPENAI_BASE_URL=https://openrouter.ai/api/v1         # or --base-url; default https://api.openai.com/v1
jg --backend openai --model VENDOR/MODEL "<query>"          # --model or $JG_MODEL is required

jg --backend openai --base-url http://localhost:11434/v1 --model MODEL -j 2 "<query>"   # local, keyless
fnox run -- sh -c 'OPENAI_API_KEY=$OPENROUTER_API_KEY jg --backend openai --model MODEL "<query>"'
```

The key is `--api-key`, else `OPENAI_API_KEY`, and nothing else: `jg` never runs `fnox` or any
other secret manager for it, though you can wrap the call in one as above. Prefer the variable: a
key passed as `--api-key` is visible to other users in `ps`. As with OpenAI's SDKs, the key is sent
to whatever base URL is configured. api.openai.com requires one; any other base URL may go without,
as local servers do. A key is only ever sent over HTTPS, or HTTP on localhost. There is no default
model: names differ between services and go stale.

[Vercel AI Gateway](https://vercel.com/docs/ai-gateway) also lists Jev itself, as
`typesafe-ai/jev`, but [not behind its OpenAI-compatible routes](https://vercel.com/docs/ai-gateway/modalities/evaluation).
With that base URL and model, `jg` asks Jev on the gateway's
[TypeSafe-compatible route](https://vercel.com/docs/ai-gateway/sdks-and-apis/typesafe) with the
same key, so the scores are Jev's own calibrated ones, billed through the gateway:

```bash
export OPENAI_API_KEY=$AI_GATEWAY_API_KEY OPENAI_BASE_URL=https://ai-gateway.vercel.sh/v1
jg --backend openai --model typesafe-ai/jev "<query>"
```

## Usage

```
jg QUERY [PATH ...]
```

| Flag | Meaning |
|---|---|
| `-e QUERY` | extra query answered in the same pass (repeatable); files are read and sent once |
| `-f, --filter TEXT` | plain-language include/exclude rules, see above |
| `--only CAT`, `--not CAT` | one category each, no parsing; repeatable |
| `-l, --files` | rank files only, no region or line scoring; about 3x cheaper |
| `-b, --broad` | flag every line related to the query, not only the lines that answer it |
| `-t 0.5` | minimum probability for a region or line to match; raise for precision, lower for recall |
| `-T 0.6` | file relevance needed for `-l`, or for the weaker tier |
| `-n 15`, `--max-regions 5`, `-m 10` | max files per query, regions per file, pinpointed lines per file |
| `-C N` | context lines around each pinpointed line |
| `-g GLOB`, `-x GLOB` | include or exclude files |
| `--json` | one object per file: `relevance`, `match` (`strong` or `weak`), `regions[]` each with `start`, `end`, `p`, `label`, `label_line`, `lines[]`, plus top-level `lines[]` for lines outside any shown region |
| `--no-heading` | flat rows: `path:START-END:prob:label` for regions, `path:LINE:prob:text` for lines; matches only |
| `--triage` | pre-filter files by path first; automatic above `--max-files` (1500) |
| `-j N` | concurrent requests (must be positive; default 32). On ChatGPT a search with fewer chunks than lanes is cut into smaller requests to fill them |
| `--backend jev\|chatgpt\|openai` | decision backend; default `jev`, or `$JG_BACKEND`. `openai` is any OpenAI-compatible service |
| `--chatgpt-login` | ChatGPT only: run Codex device login, then search; requires a query |
| `--api-key KEY` | `openai` only: API key; default `$OPENAI_API_KEY` |
| `--no-schema` | `openai` only: do not ask for structured outputs. `jg` finds this out by itself at the cost of one refused request per concurrent first request; the flag saves those for a model known to lack them |
| `--extra-body JSON` | `openai` only: request fields to add or override, or `$JG_EXTRA_BODY`; `null` removes a field; `input`, `messages` and `stream` are refused |
| `--model MODEL` | model, or `$JG_MODEL`. Jev: `jev-latest`. `openai`: required, no default. ChatGPT is `gpt-5.6-luna` only; any other `--model` or `$JG_MODEL` is an error unless `--model gpt-5.6-luna` overrides |
| `--base-url URL` | override the selected backend endpoint, or `$JG_BASE_URL`; `openai` then honours `$OPENAI_BASE_URL`. For `openai` it is an API root (`https://host/v1`), or a full `/responses` or `/chat/completions` URL to settle which API is spoken. ChatGPT and keyed `openai` requests accept HTTPS, or HTTP on `localhost` / `127.0.0.1` / `::1`, and do not follow redirects |
| `--color auto\|always\|never` | styling for human output; default `auto` |

Exit status follows grep: `0` matches, `1` none, `2` error. Results go to stdout; progress and the
stats line go to stderr (`-q` silences them). File discovery honors `.gitignore` and skips
binaries, lockfiles, and files over 512 KB.

### Parsing, color, and progress

Help and usage errors are generated by clap. Existing short aliases, clustered flags,
attached values, options after positionals, repeatable collections and last-value-wins
scalar options are retained. `QUERY` is required even with `-e`; queries are trimmed and
empty queries removed. Probability thresholds (`-t` and `-T`) must be finite and in
`[0, 1]`. Jobs and chunk sizes must be positive integers. Other numeric options retain
their zero behavior (for example, `--top 0` shows no files). Invalid arguments exit 2
before discovery, key resolution, or API calls.

`--color` controls application presentation, independently for stdout and stderr:

1. JSON and flat result output never receive application-added styling, including
   files-only output combined with those flags. Payload text and JSON precision/order
   are preserved. Generated help and usage errors are always plain.
2. Explicit `always` or `never` overrides color environment settings for eligible human output.
3. In `auto`, nonempty `NO_COLOR` or `TERM=dumb` disables color. Otherwise, nonempty
   `CLICOLOR_FORCE` other than `0` forces color, then `CLICOLOR=0` disables it; terminal
   detection supplies the default.

Human output uses display-column-aware Unicode truncation, with the existing 110-column
region-label and 200-column line budgets. Color supplements, not replaces, text labels.
Progress is uncolored, on stderr only, and visible only for an eligible stderr terminal
when `TERM` is not `dumb` and `-q` is absent. Redirecting stdout does not hide interactive
stderr progress. Progress is cleared before final results, stats, or errors; notes suspend
it without losing redirected diagnostics. Quiet suppresses notes/progress/stats, not errors. Explicit `JG_DEBUG` retry logs
also suspend progress safely and retain their existing quiet-mode behavior.

Explicit `--backend` overrides `JG_BACKEND`; unset or empty environment values use `jev`.
Explicit `--model`/`--base-url` overrides `JG_MODEL`/`JG_BASE_URL`; unset or empty environment
values use the selected backend's default. API-key/fnox and ChatGPT credential resolution
remain outside parsing. `--chatgpt-login` without a ChatGPT backend is a usage error, and so are
`--api-key`, `--no-schema` and `--extra-body` without `--backend openai`. `JG_MODEL` applies to whichever backend is selected.

## For coding agents

Paste this into `CLAUDE.md` or `AGENTS.md`:

```markdown
## Code search with jg
Use `jg "<question>" [path]` when you know what you are looking for but not what it is called.
Rows are `location  probability  text`. A range like `494-508` is a region to Read; a single
number is a line that directly answers the query. Jev probabilities are calibrated; ChatGPT
(`--backend chatgpt`) and OpenAI-compatible (`--backend openai`) scores
are uncalibrated estimates with the same output schema.
- Every row above the `-- weaker` separator cleared the threshold on its own. Rows below it are near misses; ignore them unless the matches above are not enough.
- Ask several things at once: `jg "q1" -e "q2" -e "q3"`. One pass, shared cost.
- Orient first with `jg -l "<topic>"` to rank files, then Read the top hits.
- Narrow with plain language: `--not tests --not "documentation files"`, `--only "async code"`. Name kinds of thing; jg prints how it read the filter.
- Use grep/rg instead when you already know the identifier or string.
- Phrase queries as the thing sought ("where is the retry budget enforced"), not as keywords.
- No results (exit 1) is meaningful: retry with `--broad` or `-t 0.3` before concluding absence.
```

Several agents can run `jg` at once against one key. The client throttles adaptively: on HTTP 429
every thread pauses briefly and concurrency halves, then grows back on success.

## How it works

1. **Discover** files with ripgrep's `ignore` walker, which honors `.gitignore`, `.ignore` and
   global git excludes without needing git installed.
2. **Split** each file into logical blocks using blank lines, indentation and definition
   keywords, then pack blocks into chunks of up to 150 lines within a token budget that shrinks
   as queries are added. Chunks break between blocks and carry 12 lines of leading context.
3. **Ask the selected backend**, one request per chunk. State is the query plus a line-numbered listing. Questions
   per query: one `score` for the section (ranks files), one `noul` per block ("do lines A-B
   contain the code the query is looking for"), and one `noul` per non-blank line ("does line N
   directly answer the query"). With a filter, add one `noul` per category per section and per
   block, asked once regardless of the number of queries. Jev evaluates those questions in one
   System One request. ChatGPT sends the same state and questions to the subscription Responses
   endpoint as a strict `{answers:{...}}` schema and assembles the map from SSE
   `response.output_text` deltas/done events (`response.completed` may have empty `output`).
   ChatGPT writes its answers token by token, so a request is as slow as its reply is long. The
   reply is therefore terse (a bare integer percentage per question under a positional id; the
   Jev-shaped answer is rebuilt locally), reasoning effort is `low`, and a search too small to
   fill `-j` lanes is split into finer chunks, down to a quarter of `--chunk-lines`.
   An OpenAI-compatible service gets the same state, questions and terse reply format as one
   Responses API request with structured outputs: `instructions`, `input`, `store: false`,
   `temperature: 0`, and a strict JSON schema that admits exactly the answers asked for. Only the
   API's common core is sent, and `--extra-body` passes any service's own knobs through. The
   client then adapts, once per run, to what a service refuses: no `/responses` route, chat
   completions instead; no structured outputs, no schema (a live probe of a model without them
   came back HTTP 400 rather than being served without); no `temperature`, none sent. The reply
   format is also written into the instructions, and every reply passes the same local checks
   with or without a schema; one that fails them is asked for again at a higher temperature, at
   most twice. Chunks are never split finer to fill lanes: hosted services ration requests (free
   models on OpenRouter: 20 a minute, 50 or 1000 a day).
4. **Select** what to show with the rules in `src/results.rs`: a row is printed only if
   its own probability clears `-t`; no source line appears twice; every shown file carries a
   location; matches come before near misses.

Requests that exceed Jev's context are halved and retried. Transient errors retry with
exponential backoff and jitter.

| Module | Role |
|---|---|
| `src/backend.rs` | `DecisionBackend`: `ask(state, questions) -> answers`, plus usage and optional served tier |
| `src/files.rs` | discovery, block splitting, chunking |
| `src/filters.rs` | plain-language filter parsing, polar rules |
| `src/search.rs` | request building, thread fan-out, answer aggregation over any backend |
| `src/results.rs` | display rules: what is shown, in which tier |
| `src/client.rs` | Jev HTTP client, retries, adaptive rate limiting |
| `src/answers.rs` | what a text model needs to answer like Jev: terse wire ids, response schema, reply validation |
| `src/chatgpt.rs` | ChatGPT Responses client: `gpt-5.6-luna`, requested `priority`, SSE assembly |
| `src/openai.rs` | any OpenAI-compatible service: `OPENAI_API_KEY` / `OPENAI_BASE_URL`, Responses API with a strict schema, fallbacks to chat completions / no schema / no temperature, `extra_body`, resampling, reported cost |
| `src/chatgpt_auth.rs` | subscription credential resolution and explicit Codex device login |
| `src/cli/{args,render,progress,mod}.rs` | typed flags, writer-based presentation, progress lifecycle, execution/exit codes |

`jg` began as a Python prototype (commit `31b4c25`). The Rust port sends byte-identical requests
and prints identical output; startup went from 160 ms to 2 ms and the installed footprint from
about 128 MB (CPython plus packages) to 2.8 MB. A search still takes about 2 seconds, because
that time is spent waiting on the API.

## Measured behavior

Benchmarked live (`bench/bench.py`) on the httpx source, scoring what `jg` displays. Queries are
paraphrased to avoid the code's identifiers: nine pinpoint ("which HTTP method is used after a
303 redirect") and six broad ("how does digest authentication work").

| | Result |
|---|---|
| Expected file shown first | 15 of 15 |
| Expected region shown as a confident match | 15 of 15 |
| Exact line shown, pinpoint queries | 9 of 9 |
| Confident files shown per query | 1 to 3 |
| Cost | about 35 input tokens per line per query, $0.042 per million tokens |

Question wordings were chosen by A/B inside shared requests, which works
because Jev evaluates each question independently. For lines, "does line N directly answer the
query" flags 10 to 30 times fewer stray lines than "is line N relevant", which is kept as
`--broad`. Block questions are what make broad queries work: for the digest authentication query
no line scores above 0.3, while the right blocks score 0.66 to 0.78.

API facts: the docs give a request budget of about 32k tokens shared by state and questions; by
probing, requests up to about 47k still passed and beyond that return HTTP 400
`max_tokens_exceeded` (`jg` budgets about 14k per request and halves on that error); 1,200 questions in one request take about
1.5s; bursts of 300k tokens per second pass, but a sustained token budget returns 429 without
`retry-after` headers.

## Caveats

- **Your code is sent to the selected backend.** Jev uses TypeSafe's API. ChatGPT uses your
  ChatGPT subscription over `chatgpt.com`. The `openai` backend sends it to the service at its
  base URL, with `store: false`; an aggregator such as OpenRouter forwards it to whichever provider
  serves the model, and providers of free models may log or train on prompts. Do not point `jg` at code
  you may not share.
- Jev cost scales with lines times queries. A million-line repo is roughly $1 per query, so use
  paths, `-g`, `-l`, or `--triage` to narrow large searches. ChatGPT is billed as a subscription;
  `jg` does not print Jev's per-token dollar price on that path.
- OpenAI-compatible services differ in what they ration. Free models on OpenRouter are rate
  limited (20 requests a minute; 50 a day, or 1000 with credits on the account); `jg` waits out
  the minute and reports a spent day as an error. A search costs about one request per 150 lines,
  so narrow large searches, and lower `-j` for a small local server. Result quality is the
  model's, and free models are not repeatable run to run. Requests are not streamed; a service
  that only streams is not supported.
- ChatGPT requests `service_tier: priority` (the backend rejects `fast`). The served tier may
  still be `default`; the stats line reports what came back.
- A line probability is "this line answers the query", judged within its chunk. Cross-file
  reasoning is left to the caller.

## Development

Rust **1.98.1** is both the pinned compiler and the minimum supported version.
The official stable distribution manifest was rechecked on **September 18, 2026**
(manifest date **September 3, 2026**). `rust-toolchain.toml` is canonical;
`Cargo.toml` and `mise.toml` must agree. Edition 2021 and release optimization settings
are unchanged. Rust 1.82 is no longer supported. Upgrades are explicit reviewed pin
changes, not floating `stable` updates.

Install Python 3.11+ and the exact Rust compiler/components, then the pinned QA tools:

```bash
rustup toolchain install 1.98.1 --profile minimal --component rustfmt --component clippy
scripts/install-qa-tools.sh  # checksum-pinned actionlint 1.7.12 and ShellCheck 0.11.0
scripts/check.sh             # the same fail-fast quality entrypoint used by CI
```

The check script invokes `cargo +<pin>` even under a `RUSTUP_TOOLCHAIN`/mise override.
It checks synchronized pins, helper scripts, every workflow, formatting, Clippy with
warnings denied, all targets/features, doctests, the host release build and bounded
fake-API/PTY tests. It isolates HOME, XDG paths, global Git configuration, application
environment and credentials while intentionally retaining the compiler and Cargo cache.
Normal QA and CI **never run ignored live tests**. Rust forbids unsafe code; Clippy denies
`dbg!`, `todo!` and `unimplemented!` without blanket pedantic/restriction policies.

For a native Linux release-target build, install `musl-tools` and `binutils`, then:

```bash
rustup target add --toolchain 1.98.1 x86_64-unknown-linux-musl
scripts/check.sh target x86_64-unknown-linux-musl
scripts/check.sh verify target/dist/jevgrep-v0.2.0-x86_64-unknown-linux-musl.tar.gz x86_64-unknown-linux-musl
```

The reusable `.github/workflows/checks.yml` runs the same quality gate and builds/tests
all three packaged targets on every PR, main push, manual CI run, and release run.
Native architecture assertions, downloaded-artifact checksums/layout, packaged help/version,
fake-API smoke and Linux static-linkage checks are required. Only a matching version-tag
**push**, after every gate succeeds, can publish; manual runs cannot publish, even on a tag.
External actions use reviewed commit pins. Repository settings are not changed here.
Recommended required branch-protection check: **`checks / required checks`**, which fails
unless `checks / quality` and all three `checks / native (<target>)` jobs succeed.

Additional opt-in commands (live calls send fixture code and incur API usage):

```bash
fnox exec -- cargo +1.98.1 test --test live -- --ignored  # only with explicit live-test authorization
cargo +1.98.1 test --test chatgpt_cli                 # local HTTP/SSE ChatGPT CLI tests; no live ChatGPT
cargo +1.98.1 test --test openai_cli                  # local OpenAI-compatible CLI tests; no live service
# live check through OpenRouter; send public code only
fnox run -- sh -c 'OPENAI_API_KEY=$OPENROUTER_API_KEY OPENAI_BASE_URL=https://openrouter.ai/api/v1 jg --backend openai --model MODEL "<query>" <path>'
cargo +1.98.1 build --release && fnox exec -- python3 bench/bench.py  # separate opt-in benchmark
JG_DEBUG=1 jg ...                                     # log retry reasons
```

The Rust integration tests run the real binary against local HTTP servers. The PTY harness
uses Python's standard library, bounded waits, process reaping and server cleanup; it does
not snapshot animation timing. `tests/chatgpt_cli.rs` uses a tiny local listener (account
headers and SSE) with isolated `HOME` / `XDG_*` / `CODEX_HOME` and fake credentials only.
`tests/openai_cli.rs` does the same for the Responses API and chat completions, with fake keys
and a fake `fnox` on `PATH` that must never be run. Normal QA and CI
never send live ChatGPT or OpenAI-compatible requests. Live verification notes are in
[docs/chatgpt-verification.md](docs/chatgpt-verification.md). `JG_BASE_URL` points `jg` at another
endpoint, `JG_MODEL` at another model, `JG_BACKEND` at `jev`, `chatgpt` or `openai`, and
`JG_NO_FNOX=1` disables fnox lookup. The [ADRs](docs/adr/README.md)
record compatibility decisions and the full option/test inventory.

### Releasing

`version` in `Cargo.toml` is the source of truth, and pushing a `vX.Y.Z` tag is what builds and
publishes the binaries. Cut a release from a clean `main` with:

```bash
scripts/release.sh patch --dry-run   # bump, test, show the plan, change nothing
scripts/release.sh minor             # bump, commit, tag, push, wait for the build, verify
```

The script refuses to run off `main`, with a dirty tree, or on a tag that already exists; after
the build it checks that all three tarballs are attached, that the release is the latest one, and
that `mise exec github:thehumanworks/jevgrep@X.Y.Z -- jg --version` prints the new version. `jg`
is pre-1.0: user-visible changes, including any change to the wording of the questions sent to
Jev, are a **minor** bump; fixes and internals are a **patch**. Releases are cut by a coding
agent, not by hand: [`AGENTS.md`](AGENTS.md) says so and `.claude/skills/release/SKILL.md` spells
out the rules.

The benchmark needs the httpx 0.28.1 source in `bench/corpus/httpx` (gitignored):
`pip install --no-deps --target bench/corpus httpx==0.28.1`. The A/B experiment scripts for
question wordings and filter templates were written against the Python prototype and live in
git history (`git show 31b4c25 --stat -- bench`).
