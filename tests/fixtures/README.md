# CLI contract fixtures

`grouped.txt`, `json.jsonl`, `flat.txt`, `files.txt`, `files.jsonl`, and `multi.txt`
were captured from the **unmodified 72d526d release binary**, built with Rust
1.98.1, before replacing the parser/renderers. The tiny fixture is `t.py`:

```python
a = 1
needle = 2
b = 3
```

The fake response has relevance 1, confidence 0.9, region probability 0.9,
line 2 probability 0.95, and other line probabilities 0.02. Calls use query
`find needle`, `-q`, then respectively `-C1`, `--json`, `--no-heading`, `-l`,
`-l --json`, and `-e another -l`. Tests retain exact bytes plus semantic JSON
assertions. Do not regenerate these files to accommodate a regression.

`help-before.txt`, `error-before.txt`, and `version.txt` are historical parser
characterization fixtures from the same binary. `help.txt` and `error.txt` capture
the intentional clap presentation change at a fixed width of 100, without color.
Their tests check stream placement, status, environment isolation, required content,
and width independently as well. Version is tested against Cargo metadata so a
future version bump does not require changing a frozen historical fixture.
