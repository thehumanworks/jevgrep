"""Natural-language include/exclude filters, turned into the question shape Jev answers best.

Benchmarks (bench/filters.py) show Jev is unreliable on negated or compound rules. Asked whether a
file satisfies "No tests", ordinary source files score near 0.5. Asked "Is this a test file?", the
same files score near 0 and test files near 1. This matches TypeSafe's guidance: ask atomic
questions, phrase them so that yes is the high-probability answer, and compose answers in code.

So a filter such as "Source code files only. No documentation" is parsed into polar rules:
    only:  [["source code files"]]      every clause must match (alternatives inside a clause: any)
    not:   ["documentation"]            none may match
Each term becomes one positive category question. Polarity is applied here, in code.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field

FILTER_GATE = 0.5   # a row is kept when P(passes the rules) reaches this

_EXCLUDE = re.compile(
    r"^(?:no|not|non|without|except(?:\s+for)?|exclud(?:e|es|ed|ing)|ignor(?:e|es|ed|ing)|skip(?:s|ped|ping)?|"
    r"omit(?:s|ted|ting)?|hide|drop|never|minus|leave\s+out|filter\s+out|"
    r"(?:do\s+not|don'?t)\s+(?:include|show|match|return|want|search))\b[\s:-]*(?:any\s+|all\s+|the\s+)?",
    re.I,
)
_INCLUDE = re.compile(
    r"^(?:only|just|solely|include\s+only|show\s+only|match\s+only|search\s+only|limit(?:ed)?\s+to|"
    r"restrict(?:ed)?\s+to|must\s+be|keep\s+only|include)\b[\s:-]*(?:the\s+)?",
    re.I,
)
_ONLY_SUFFIX = re.compile(r"[\s,]+only$", re.I)
_LIST = re.compile(r"\s*(?:,|;|/|\band\b|\bor\b|\bnor\b|&)\s*", re.I)


@dataclass
class Rules:
    only: list[list[str]] = field(default_factory=list)   # AND of clauses, OR within a clause
    exclude: list[str] = field(default_factory=list)      # none may match

    def __bool__(self) -> bool:
        return bool(self.only or self.exclude)

    def terms(self) -> list[str]:
        """Distinct terms, in a stable order: one question each."""
        seen: dict[str, None] = {}
        for t in [t for clause in self.only for t in clause] + self.exclude:
            seen.setdefault(t)
        return list(seen)

    def passes(self, p: dict[str, float]) -> float:
        """P(row passes), from per-term probabilities. Missing terms count as unknown (0.5)."""
        parts = [max(p.get(t, 0.5) for t in clause) for clause in self.only]
        parts += [1.0 - p.get(t, 0.5) for t in self.exclude]
        return min(parts, default=1.0)

    def describe(self) -> str:
        bits = [f"only {' or '.join(c)}" for c in self.only] + [f"not {t}" for t in self.exclude]
        return "; ".join(bits)

    def merge(self, other: "Rules") -> "Rules":
        return Rules(self.only + other.only, list(dict.fromkeys(self.exclude + other.exclude)))


def _clean(term: str) -> str:
    return term.strip(" \t\"'`.,:;!()[]").strip()


def parse_filter(text: str) -> Rules:
    """Parses free text like "Source code only. No docs, tests or examples" into polar rules."""
    rules = Rules()
    for clause in re.split(r"[.\n;]+|\s+but\s+", text):
        clause = _clean(clause)
        if not clause:
            continue
        clause_only = bool(_ONLY_SUFFIX.search(clause))
        if clause_only:
            clause = _ONLY_SUFFIX.sub("", clause)
        polarity = "only"          # a bare clause such as "async code" is an inclusion
        alternatives: list[str] = []
        for part in _LIST.split(clause):
            part = _clean(part)
            if not part:
                continue
            if m := _EXCLUDE.match(part):
                polarity, part = "not", part[m.end():]
            elif m := _INCLUDE.match(part):
                polarity, part = "only", part[m.end():]
            if _ONLY_SUFFIX.search(part):
                polarity, part = "only", _ONLY_SUFFIX.sub("", part)
            part = _clean(part)
            if not part:
                continue
            if polarity == "not":
                rules.exclude.append(part)
            else:
                alternatives.append(part)
        if alternatives:
            rules.only.append(alternatives)
    rules.exclude = list(dict.fromkeys(rules.exclude))
    return rules
