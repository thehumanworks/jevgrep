"""Probe whether TypeSafe's 429s track request rate or token rate."""
import concurrent.futures as cf, sys, time
from jevgrep.client import JevClient, resolve_api_key

def burst(n_req, threads, state_lines, n_questions):
    state = {"query": "find the needle", "code": "\n".join(f"{i}| value_{i} = compute({i})" for i in range(state_lines))}
    qs = {f"L{i}": {"type": "noul", "instructions": f"Does line {i} directly answer the query?"} for i in range(n_questions)}
    with JevClient(resolve_api_key(), pool_size=threads) as c:
        t = time.time()
        with cf.ThreadPoolExecutor(threads) as ex:
            list(ex.map(lambda _: c.ask(state, qs), range(n_req)))
        dt = time.time() - t
        u = c.usage
        print(f"req={n_req} threads={threads} tok/req={u.input_tokens // u.requests:,} -> {dt:.1f}s, "
              f"{u.requests / dt:.0f} req/s, {u.input_tokens / dt / 1000:.0f}k tok/s, 429-retries={u.retries}")

burst(300, 64, 5, 1)        # many tiny requests: request-rate test
time.sleep(20)
burst(60, 32, 400, 400)     # fewer huge requests: token-rate test
time.sleep(20)
burst(60, 8, 400, 400)      # same load, lower concurrency
