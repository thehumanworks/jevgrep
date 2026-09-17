"""Thread-safe client for TypeSafe's System One API (the Jev model).

Jev does not generate text. You send a `state` plus a map of typed questions and
get back calibrated probabilities. All questions in one request are evaluated in
parallel against the same state, so a request with hundreds of questions costs
about the same latency as a request with one.
"""

from __future__ import annotations

import os
import random
import shutil
import subprocess
import sys
import threading
import time
from dataclasses import dataclass, field

import httpx

DEFAULT_BASE_URL = "https://api.typesafe.ai/v1/systemone"
DEFAULT_MODEL = "jev-latest"
USD_PER_INPUT_MTOK = 0.042  # output tokens are free
RETRYABLE = {408, 409, 425, 429, 500, 502, 503, 504, 529}


class JevError(Exception):
    """Unrecoverable API error."""


class JevAuthError(JevError):
    """Missing or rejected API key."""


class TokenLimitError(JevError):
    """The request exceeded the model's context. Callers should split and retry."""


@dataclass
class Usage:
    requests: int = 0
    retries: int = 0
    input_tokens: int = 0
    output_tokens: int = 0
    _lock: threading.Lock = field(default_factory=threading.Lock, repr=False)

    def add(self, usage: dict) -> None:
        with self._lock:
            self.requests += 1
            self.input_tokens += int(usage.get("input_tokens", 0))
            self.output_tokens += int(usage.get("output_tokens", 0))

    def add_retry(self) -> None:
        with self._lock:
            self.retries += 1

    @property
    def cost_usd(self) -> float:
        return self.input_tokens / 1_000_000 * USD_PER_INPUT_MTOK


class AdaptiveLimiter:
    """Shared concurrency gate with AIMD backoff.

    TypeSafe's rate limit is a sustained token budget with no quota headers, and several jg
    processes (one per agent) may share a key. So on a 429 every thread pauses briefly and the
    allowed concurrency halves; each success grows it back by roughly one slot per round.
    """

    def __init__(self, max_concurrency: int, pause: float = 1.0) -> None:
        self.pause = pause
        self.max = max(1, max_concurrency)
        self.limit = float(self.max)
        self.in_flight = 0
        self.paused_until = 0.0
        self._cv = threading.Condition()

    def acquire(self) -> None:
        with self._cv:
            while True:
                wait = self.paused_until - time.monotonic()
                if wait <= 0 and self.in_flight < max(1, int(self.limit)):
                    self.in_flight += 1
                    return
                self._cv.wait(timeout=max(wait, 0.05) if wait > 0 else None)

    def release(self, throttled: bool) -> None:
        with self._cv:
            self.in_flight -= 1
            now = time.monotonic()
            if throttled:
                if now >= self.paused_until:  # one cut per throttling episode, not per 429
                    self.limit = max(1.0, self.limit / 2)
                    self.paused_until = now + self.pause * (1.0 + random.random())
            else:
                self.limit = min(float(self.max), self.limit + 1.0 / max(1.0, self.limit))
            self._cv.notify_all()


def resolve_api_key() -> str:
    """TYPESAFE_API_KEY from the environment, falling back to `fnox get`."""
    key = os.environ.get("TYPESAFE_API_KEY", "").strip()
    if key:
        return key
    if shutil.which("fnox"):
        try:
            out = subprocess.run(
                ["fnox", "get", "TYPESAFE_API_KEY"],
                capture_output=True, text=True, timeout=30,
            )
            if out.returncode == 0 and out.stdout.strip():
                return out.stdout.strip()
        except (OSError, subprocess.TimeoutExpired):
            pass
    raise JevAuthError(
        "TYPESAFE_API_KEY is not set. Export it, run via `fnox exec -- jg ...`, "
        "or add it to a fnox config visible from this directory."
    )


class JevClient:
    """One shared connection pool; safe to call `ask` from many threads."""

    def __init__(
        self,
        api_key: str,
        *,
        base_url: str = DEFAULT_BASE_URL,
        model: str = DEFAULT_MODEL,
        timeout: float = 60.0,
        max_retries: int = 8,
        pool_size: int = 64,
    ) -> None:
        self.model = model
        self.base_url = base_url
        self.max_retries = max_retries
        self.usage = Usage()
        self.limiter = AdaptiveLimiter(pool_size)
        self._http = httpx.Client(
            timeout=httpx.Timeout(timeout, connect=15.0),
            limits=httpx.Limits(max_connections=pool_size, max_keepalive_connections=pool_size),
            headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"},
        )

    @staticmethod
    def _debug(msg: str) -> None:
        if os.environ.get("JG_DEBUG"):
            print(f"jg[debug]: {msg}", file=sys.stderr)

    def close(self) -> None:
        self._http.close()

    def __enter__(self) -> "JevClient":
        return self

    def __exit__(self, *exc) -> None:
        self.close()

    def ask(self, state, questions: dict) -> dict:
        """Returns the `answers` map. Retries transient failures with backoff."""
        body = {"model": self.model, "state": state, "questions": questions}
        last = "unknown error"
        for attempt in range(self.max_retries + 1):
            if attempt:
                self.usage.add_retry()
                time.sleep(min(30.0, 0.5 * 2 ** (attempt - 1)) * (0.5 + random.random()))
            self.limiter.acquire()
            throttled = False
            try:
                resp = self._http.post(self.base_url, json=body)
                throttled = resp.status_code in (429, 529)
            except httpx.TransportError as e:
                last = f"{type(e).__name__}: {e}"
                self._debug(f"attempt {attempt + 1}: {last}")
                continue
            finally:
                self.limiter.release(throttled)
            if resp.status_code == 200:
                data = resp.json()
                self.usage.add(data.get("usage") or {})
                return data["answers"]
            text = resp.text[:400]
            if resp.status_code in (401, 403):
                raise JevAuthError(f"TypeSafe rejected the API key (HTTP {resp.status_code}): {text}")
            if "max_tokens_exceeded" in text or resp.status_code == 413:
                raise TokenLimitError(text)
            if resp.status_code in RETRYABLE:
                last = f"HTTP {resp.status_code}: {text}"
                self._debug(f"attempt {attempt + 1}: {last} headers={dict(resp.headers)}")
                retry_after = resp.headers.get("retry-after")
                if retry_after:
                    try:
                        time.sleep(min(30.0, float(retry_after)))
                    except ValueError:
                        pass
                continue
            raise JevError(f"HTTP {resp.status_code}: {text}")
        raise JevError(f"gave up after {self.max_retries} retries: {last}")
