# jevgrep (`jg`)

Natural-language grep. Ask a question about a codebase in plain language and get
back the files and line numbers that answer it.

`jg` is a single Rust binary. The default backend is
[Jev](https://typesafe.ai/blog/introducing-system-one-models-and-jev), TypeSafe's
decision model: calibrated probabilities, every question in a request evaluated
in parallel. You can instead use a ChatGPT subscription (`--backend chatgpt`) or
any OpenAI-compatible service (`--backend openai`).

```console
$ jg "which HTTP method is used after a 303 redirect"
httpx/_client.py  relevance=1.00
    494-508  0.96  def _redirect_method(self, request: Request, response: Response) -> str:
        502  0.91          if response.status_code == codes.SEE_OTHER and method != "HEAD":
        503  0.92              method = "GET"

jg: 23 files, 72 requests, 301,208 tokens (~$0.0127), 2.2s
```

Jev stats include a token dollar estimate; the other backends report tokens (and a cost only
when the service states one).

## Install

Every [release](https://github.com/thehumanworks/jevgrep/releases) carries a prebuilt binary for
macOS on Apple Silicon (`aarch64-apple-darwin`) and Linux on x64 and aarch64
(`x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`). The Linux builds are static, so they
run on any distribution regardless of its glibc. The Linux GNU host build is about 3.3 MB; its
only dynamic dependencies are libc and libgcc, and TLS roots are compiled in.

With [mise](https://mise.jdx.dev), pointed straight at this repo:

```bash
mise exec github:thehumanworks/jevgrep -- jg --help   # one-off run, nothing installed
mise use -g github:thehumanworks/jevgrep              # or put `jg` on PATH for good
```

Or download a tarball from the releases page and drop the binary on your PATH
(replace `v0.3.0` with the version on the
[releases](https://github.com/thehumanworks/jevgrep/releases) page):

```bash
tar xzf jevgrep-v0.3.0-x86_64-unknown-linux-musl.tar.gz
install -m755 jevgrep-v0.3.0-x86_64-unknown-linux-musl/jg ~/.local/bin/jg
```

From source:

```bash
cargo install --path .      # builds the release binary and puts `jg` in ~/.cargo/bin
# or build it and copy it wherever you like:
cargo build --release && install -m755 target/release/jg ~/.local/bin/jg
```

A source build links against the system libc, so the binary can be copied to any
machine with the same OS, architecture, and a libc at least as new. Windows is
not built or tested. Static Linux builds come from adding the musl target and a musl
C compiler, then `cargo build --release --target x86_64-unknown-linux-musl`.

`jg` is not useful until a backend is configured. See **Set up a backend**.

## Set up a backend

Pick one. The default is Jev.

### Jev (default)

Create a TypeSafe API key
([quick start](https://docs.typesafe.ai/introduction/quickstart)) and export it:

```bash
export TYPESAFE_API_KEY=...   # required for --backend jev (the default)
jg "where is the timeout set"
```

The repo and the release binaries do not include a key. `jg` reads
`TYPESAFE_API_KEY` from the environment. Keep the key out of git and out of
process listings (`ps` can see a flag; prefer the variable).

### ChatGPT (`--backend chatgpt`)

Uses your ChatGPT subscription, posting to
`https://chatgpt.com/backend-api/codex/responses` with fixed model `gpt-5.6-luna`.
That path is not Codex inference. Scores use the same output schema as Jev, but
they are generative estimates, not calibrated probabilities.

The ChatGPT backend needs a subscription pair, resolved in this order and never
mixed across sources:

1. Complete `CHATGPT_ACCOUNT_ID` and `CHATGPT_ACCESS_TOKEN` together.
2. Optional `$XDG_CONFIG_HOME/auth.toml` (must be an absolute XDG path) or `~/.config/auth.toml`.
   TOML may use those uppercase keys, lowercase `account_id` / `access_token`, or the same
   lowercase pair under `[chatgpt]` or `[tokens]`.
3. `$CODEX_HOME/auth.json` or `~/.codex/auth.json` (`tokens.account_id` / `tokens.access_token`).

A partial or invalid selected pair is an error. Normal searches never start an
interactive login. `--chatgpt-login` is explicit: device login, then the search.
A stale cache fails at the server with a `--chatgpt-login` hint.

### OpenAI-compatible (`--backend openai`)

Same shape as OpenAI's SDKs. `--model` or `$JG_MODEL` is required (no default).
api.openai.com is the default; [OpenRouter](https://openrouter.ai), Groq, a corporate
gateway, vLLM, llama.cpp, Ollama or LM Studio are each a base URL, a key (or none)
and a model. `jg` prefers the Responses API with structured outputs and falls back
where a service lacks them. These scores are model estimates too.

```bash
export OPENAI_API_KEY=...                            # or --api-key; omit for local servers
export OPENAI_BASE_URL=https://openrouter.ai/api/v1  # default https://api.openai.com/v1
jg --backend openai --model VENDOR/MODEL "<query>"

jg --backend openai --base-url http://localhost:11434/v1 --model MODEL -j 2 "<query>"
```

Prefer `OPENAI_API_KEY` over `--api-key` (`ps` can see the flag). The key is sent
to whatever base URL you set. api.openai.com requires one; other hosts may not.
A key is only sent over HTTPS, or HTTP on localhost.

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
| `-f, --filter TEXT` | plain-language include/exclude rules, see **Filters in plain language** |
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
binaries, lockfiles, and files over 512 KB. Invalid arguments exit 2 before discovery, key
resolution, or API calls. Key and ChatGPT credential resolution run after a successful parse.

`jg --help` is the full flag and environment list. Color, progress, and
flag/environment precedence are documented there.

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
4. **Select** what to show: a row is printed only if its own probability clears `-t`; no source
   line appears twice; every shown file carries a location; matches come before near misses.

Requests that exceed Jev's context are halved and retried. Transient errors retry with
exponential backoff and jitter.

`jg` began as a Python prototype (commit `31b4c25`). The Rust port sends the same requests
and prints the same output; a search still takes about 2 seconds, because that time is spent
waiting on the API.

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
Normal QA and CI **never run ignored live tests**. Rust forbids unsafe code. Clippy denies
the `correctness` and `suspicious` groups plus a short, named list of pedantic/restriction
lints that catch real bugs here (UTF-8 slices, forgotten struct fields, off-by-one ranges).
There is no blanket `pedantic` or `restriction` policy. Lint levels live in `Cargo.toml`
so every Clippy invocation uses the same rules; `scripts/check.sh` still passes
`-D warnings`. See [ADR 0006](docs/adr/0006-clippy-correctness.md).

Install the optional git hook so formatting, Clippy, and the test suite run on every
commit. `scripts/pre-commit.sh` is that fast gate: the same Clippy command as CI, without
isolation, workflow lint, rustdoc, the release build, or the PTY harness
([ADR 0008](docs/adr/0008-pre-commit-hook.md)):

```bash
scripts/install-git-hooks.sh
```

Skip one commit with `JG_SKIP_HOOKS=1` or `SKIP=1`. The hook does not install extra tools
and does not run ignored live tests. `scripts/check.sh` remains the full isolated gate.

For a native Linux release-target build, install `musl-tools` and `binutils`, then:

```bash
rustup target add --toolchain 1.98.1 x86_64-unknown-linux-musl
scripts/check.sh target x86_64-unknown-linux-musl
scripts/check.sh verify target/dist/jevgrep-v0.3.0-x86_64-unknown-linux-musl.tar.gz x86_64-unknown-linux-musl
```

The reusable `.github/workflows/checks.yml` runs the same quality gate and builds/tests
all three packaged targets on every PR, main push, manual CI run, and release run.
Native architecture assertions, downloaded-artifact checksums/layout, packaged help/version,
fake-API smoke and Linux static-linkage checks are required. Only a matching version-tag
**push**, after every gate succeeds, can publish; manual runs cannot publish, even on a tag.
External actions use reviewed commit pins.

Additional opt-in commands (live calls send fixture code and incur API usage):

```bash
# Live Jev tests: only with a key you own, and only on public/fixture code
export TYPESAFE_API_KEY=...
cargo test --test live -- --ignored

# Local protocol tests; no live ChatGPT or OpenAI-compatible service
cargo test --test chatgpt_cli
cargo test --test openai_cli

# Live openai backend: public code only
export OPENAI_API_KEY=... OPENAI_BASE_URL=https://openrouter.ai/api/v1
jg --backend openai --model MODEL "<query>" <path>

cargo build --release && python3 bench/bench.py   # opt-in; needs TYPESAFE_API_KEY
JG_DEBUG=1 jg ...                                 # log retry reasons
```

The Rust integration tests run the real binary against local HTTP servers. The PTY harness
uses Python's standard library, bounded waits, process reaping and server cleanup; it does
not snapshot animation timing. `tests/chatgpt_cli.rs` and `tests/openai_cli.rs` use tiny local
listeners with isolated `HOME` / `XDG_*` / `CODEX_HOME` and fake credentials only. Normal QA
and CI never send live ChatGPT or OpenAI-compatible requests. `JG_BASE_URL` points `jg` at
another endpoint, `JG_MODEL` at another model, and `JG_BACKEND` at `jev`, `chatgpt` or
`openai`. The [ADRs](docs/adr/README.md) record compatibility decisions and the full
option/test inventory.

The benchmark needs the httpx 0.28.1 source in `bench/corpus/httpx` (gitignored):
`pip install --no-deps --target bench/corpus httpx==0.28.1`. The A/B experiment scripts for
question wordings and filter templates were written against the Python prototype and live in
git history (`git show 31b4c25 --stat -- bench`).

### Releasing

`version` in `Cargo.toml` is the source of truth. Pushing a `vX.Y.Z` tag builds
and publishes the three host tarballs. Pre-1.0: user-visible changes, including
question wording sent to Jev, are a **minor** bump; fixes and internals are a
**patch**. Maintainers can use `scripts/release.sh <patch|minor>`.
