# jevgrep (`jg`)

Natural-language grep. Ask a question about a codebase in plain language and get back the files
and line numbers that answer it, each with a calibrated probability.

`jg` is built on [Jev](https://typesafe.ai/blog/introducing-system-one-models-and-jev), TypeSafe's
decision model. Jev does not generate text. It takes a state plus typed questions and returns
calibrated probabilities, evaluating every question in a request in parallel. `jg` exploits that:
each file chunk becomes one request carrying one question per line, and all requests fan out over
a thread pool. A 9,000-line codebase is scored line by line in about 2 seconds for about 1 cent.

```console
$ jg "which HTTP method is used after a 303 redirect"
httpx/_client.py  relevance=1.00
    494-508  0.96  def _redirect_method(self, request: Request, response: Response) -> str:
        502  0.91          if response.status_code == codes.SEE_OTHER and method != "HEAD":
        503  0.92              method = "GET"

jg: 23 files, 72 requests, 301,208 tokens (~$0.0127), 2.2s
```

## Reading the output

Every row has the same shape: location, probability, text.

| Row | Location | Probability means |
|---|---|---|
| file header | path | `relevance`: the best evidence in the file; files are sorted by it |
| region | `494-508` | these lines contain the code you are looking for |
| line | `502` | this line directly answers the query |

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

src/jevgrep/search.py  relevance=0.68
    122-134  0.40  def build_request(queries: list[str], chunk: Chunk, files_only: bool, ...
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
labelled files (`bench/filters.py`):

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

End to end (`bench/filter_e2e.py`, 12 exclusion runs over two repos): of 60 rows from forbidden
kinds shown without a filter, 0 remained with it. Excluding tests or benchmarks kept 100% of the
allowed code lines; excluding "documentation" kept 73 to 88% on the Rust crate, for the reason
above.

## Install

```bash
uv tool install -e .        # puts `jg` on your PATH
# or, from this directory without installing:
uv run jg "your question"
```

`jg` needs `TYPESAFE_API_KEY`. It reads the environment variable first, then falls back to
`fnox get TYPESAFE_API_KEY`. This repo's `fnox.toml` already provides it, so inside this repo
`jg` just works. To use `jg` in other repos, export the variable or add the secret to your global
fnox config.

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
| `--json` | one object per file: `relevance`, `match` (`strong` or `weak`), `regions[]` each with `start`, `end`, `p`, `label`, `lines[]`, plus top-level `lines[]` for lines outside any shown region |
| `--no-heading` | flat rows: `path:START-END:prob:label` for regions, `path:LINE:prob:text` for lines; matches only |
| `--triage` | pre-filter files by path first; automatic above `--max-files` (1500) |
| `-j 32` | concurrent requests |

Exit status follows grep: `0` matches, `1` none, `2` error. Results go to stdout; progress and the
stats line go to stderr (`-q` silences them). File discovery honors `.gitignore` and skips
binaries, lockfiles, and files over 512 KB.

## For coding agents

Paste this into `CLAUDE.md` or `AGENTS.md`:

```markdown
## Code search with jg
Use `jg "<question>" [path]` when you know what you are looking for but not what it is called.
Rows are `location  probability  text`. A range like `494-508` is a region to Read; a single
number is a line that directly answers the query. Probabilities are calibrated.
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

1. **Discover** files with `git ls-files` (or a filtered walk outside git).
2. **Split** each file into logical blocks using blank lines, indentation and definition
   keywords, then pack blocks into chunks of up to 150 lines within a token budget that shrinks
   as queries are added. Chunks break between blocks and carry 12 lines of leading context.
3. **Ask Jev**, one request per chunk. State is the query plus a line-numbered listing. Questions
   per query: one `score` for the section (ranks files), one `noul` per block ("do lines A-B
   contain the code the query is looking for"), and one `noul` per non-blank line ("does line N
   directly answer the query"). With a filter, add one `noul` per category per section and per
   block, asked once regardless of the number of queries.
4. **Select** what to show with the rules in `src/jevgrep/results.py`: a row is printed only if
   its own probability clears `-t`; no source line appears twice; every shown file carries a
   location; matches come before near misses.

Requests that exceed Jev's context are halved and retried. Transient errors retry with
exponential backoff and jitter.

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

Question wordings were chosen by A/B inside shared requests (`bench/wording.py`), which works
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

- **Your code is sent to TypeSafe's API.** Do not point `jg` at code you may not share.
- Cost scales with lines times queries. A million-line repo is roughly $1 per query, so use
  paths, `-g`, `-l`, or `--triage` to narrow large searches.
- A line probability is "this line answers the query", judged within its chunk. Cross-file
  reasoning is left to the caller.

## Development

```bash
uv run pytest -m "not live"          # unit tests, fake transport
fnox exec -- uv run pytest           # everything, including live API tests
fnox exec -- uv run python bench/bench.py            # known-answer benchmark, scores displayed output
fnox exec -- uv run python bench/wording.py          # A/B question wordings
fnox exec -- uv run python bench/inspect_query.py "query" [path]   # raw probabilities behind a result
fnox exec -- uv run python bench/audit_output.py     # checks displayed rows: none below its bar, none twice
fnox exec -- uv run python bench/filters.py          # A/B ways of putting a filter rule to Jev
fnox exec -- uv run python bench/filter_templates.py # A/B category question templates, gate sweep
fnox exec -- uv run python bench/filter_e2e.py       # filter leaks and retention on displayed rows
JG_DEBUG=1 jg ...                    # log retry reasons
```
