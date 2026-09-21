#!/usr/bin/env python3
"""Bounded fake-API smoke and real terminal wiring checks. No credentials required."""

import argparse
import contextlib
import errno
import http.server
import json
import os
from pathlib import Path
import selectors
import subprocess
import tempfile
import threading
import time


class FakeAPI(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        pass

    def do_POST(self):  # noqa: N802 (HTTPServer protocol)
        assert self.headers.get("Authorization") == "Bearer test-key"
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        self.server.requests += 1
        answers = {}
        for name, question in body["questions"].items():
            if question["type"] == "score":
                answer = {"type": "score", "score": 3.0, "confidence": 0.9, "probabilities": {}, "legend": {}}
            else:
                answer = {"type": "noul", "noul": 0.9 if ".B" in name else (0.95 if name.endswith(".L2") else 0.02)}
            answers[name] = answer
        payload = json.dumps({"model": "jev-test", "answers": answers, "usage": {"input_tokens": 100}}).encode()
        status = 422 if body.get("state", {}).get("file", "").endswith(self.server.failed_filename) else self.server.status
        if self.server.retry_once:
            self.server.retry_once = False
            status = 503
        if status != 200:
            payload = b'{"detail":"fake API error"}'
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        with contextlib.suppress(BrokenPipeError, ConnectionResetError):
            self.wfile.write(payload)


@contextlib.contextmanager
def fake_api():
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), FakeAPI)
    server.daemon_threads = True
    server.requests = 0
    server.status = 200
    server.failed_filename = "<no failed file>"
    server.retry_once = False
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield server, f"http://127.0.0.1:{server.server_port}/v1/systemone"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        assert not thread.is_alive(), "fake server failed to stop"


def environment(root, url):
    """Pass a deliberately small environment; never inherit application settings."""
    home = root / "home"
    home.mkdir()
    return {
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(home / "config"),
        "XDG_CACHE_HOME": str(home / "cache"),
        "XDG_DATA_HOME": str(home / "data"),
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_CONFIG_NOSYSTEM": "1",
        "PATH": os.defpath,
        "JG_NO_FNOX": "1",
        "TYPESAFE_API_KEY": "test-key",
        "JG_BASE_URL": url,
        "LC_ALL": "C",
        "TERM": "xterm-256color",
        "COLUMNS": "100",
    }


def run_terminal(binary, cwd, env, args, stdout_tty=False, stderr_tty=False, broken_pipe=False):
    """Drain both streams concurrently; kill/reap on every error or timeout."""
    import fcntl
    import pty
    import struct
    import termios

    masters, slaves = [], []
    streams = {}
    proc = None
    with selectors.DefaultSelector() as selector:
        try:
            destinations = []
            for name, tty in (("out", stdout_tty), ("err", stderr_tty)):
                if tty:
                    master, slave = pty.openpty()
                    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
                    masters.append(master)
                    slaves.append(slave)
                    streams[name] = master
                    destinations.append(slave)
                elif name == "out" and broken_pipe:
                    read_fd, write_fd = os.pipe()
                    os.close(read_fd)
                    slaves.append(write_fd)
                    destinations.append(write_fd)
                else:
                    destinations.append(subprocess.PIPE)
            proc = subprocess.Popen(
                [str(binary), *args], cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                stdout=destinations[0], stderr=destinations[1],
            )
            for fd in slaves:
                os.close(fd)
            slaves.clear()
            if not stdout_tty and not broken_pipe:
                streams["out"] = proc.stdout.fileno()
            if not stderr_tty:
                streams["err"] = proc.stderr.fileno()
            for name, fd in streams.items():
                os.set_blocking(fd, False)
                selector.register(fd, selectors.EVENT_READ, name)
            output = {"out": bytearray(), "err": bytearray()}
            deadline = time.monotonic() + 20
            while selector.get_map():
                assert time.monotonic() < deadline, f"terminal case timed out: {args}"
                for key, _events in selector.select(timeout=0.1):
                    try:
                        data = os.read(key.fd, 65536)
                    except OSError as error:
                        if error.errno != errno.EIO:
                            raise
                        data = b""  # PTY slave closed
                    if data:
                        output[key.data].extend(data)
                    else:
                        selector.unregister(key.fd)
            code = proc.wait(timeout=max(0.1, deadline - time.monotonic()))
            return code, bytes(output["out"]), bytes(output["err"])
        finally:
            if proc is not None:
                if proc.poll() is None:
                    proc.kill()
                proc.wait(timeout=5)
                for stream in (proc.stdout, proc.stderr):
                    if stream is not None:
                        stream.close()
            for fd in masters + slaves:
                os.close(fd)


def check(binary, terminal=False):
    with tempfile.TemporaryDirectory(prefix="jg-terminal-") as tmp, fake_api() as (server, url):
        root = Path(tmp)
        cwd = root / "repo"
        cwd.mkdir()
        (cwd / "t.py").write_text("a = 1\nneedle = 2\nb = 3\n", encoding="utf-8")
        env = environment(root, url)
        for flag in ("--help", "--version"):
            result = subprocess.run([str(binary), flag], cwd=cwd, env=env, capture_output=True, timeout=10, check=True)
            assert result.stdout and not result.stderr
        assert server.requests == 0, "help/version accessed the API"
        result = subprocess.run([str(binary), "find needle", "--json", "-q"], cwd=cwd, env=env, capture_output=True, timeout=20, check=True)
        row = json.loads(result.stdout)
        assert row["path"] == "t.py" and row["match"] == "strong", row
        assert row["regions"][0]["start"] == 1 and row["regions"][0]["end"] == 3, row
        assert result.stderr == b"" and b"\x1b" not in result.stdout
        assert server.requests == 1
        if not terminal:
            print("fake API smoke: help, version, JSON search passed")
            return

        # Exact flags/env and stream topology, not elapsed time or animation snapshots.
        cases = [
            ("stdout pipe / stderr TTY", False, True, "xterm-256color", [], True),
            ("stdout TTY / stderr pipe", True, False, "xterm-256color", [], False),
            ("both pipes", False, False, "xterm-256color", [], False),
            ("dumb terminal", True, True, "dumb", [], False),
            ("quiet", True, True, "xterm-256color", ["-q"], False),
        ]
        for label, out_tty, err_tty, term, flags, progress in cases:
            code, out, err = run_terminal(binary, cwd, {**env, "TERM": term}, ["find needle", *flags], out_tty, err_tty)
            assert code == 0 and b"t.py" in out, (label, code, out, err)
            assert (b"chunks" in err) == progress, (label, err)
            assert (b"\x1b" in out) == (out_tty and term != "dumb"), (label, out)
            if progress:
                assert b"\x1b[" in err, (label, err)
                assert err.rfind(b"chunks") < err.rfind(b"jg: 1 files"), (label, err)
            if not err_tty or term == "dumb":
                assert b"\x1b" not in err and b"\r" not in err.replace(b"\r\n", b"\n"), (label, err)
            if flags:
                assert err == b"", (label, err)
            print(f"PTY: {label}: passed")

        # A partial API failure emits a note while progress is still active.
        (cwd / "bad.py").write_text("needle = 2\n", encoding="utf-8")
        server.failed_filename = "bad.py"
        code, out, err = run_terminal(binary, cwd, env, ["find needle"], stderr_tty=True)
        assert code == 0 and b"t.py" in out and b"bad.py" not in out, (code, out, err)
        assert b"1 request(s) failed, results may be incomplete" in err, err
        assert err.rfind(b"chunks") < err.rfind(b"jg: 2 files"), err
        (cwd / "bad.py").unlink()
        server.failed_filename = "<no failed file>"
        print("PTY: notes suspend active progress: passed")

        # Worker-thread retry logs suspend progress too; quiet keeps requested debug logs.
        for quiet in (False, True):
            server.retry_once = True
            code, out, err = run_terminal(binary, cwd, {**env, "JG_DEBUG": "1"},
                                         ["find needle", *(["-q"] if quiet else [])], stderr_tty=True)
            assert code == 0 and b"t.py" in out and b"jg[debug]: attempt 1:" in err, (code, out, err)
            if quiet:
                assert b"chunks" not in err and b"jg: 1 files" not in err, err
            else:
                assert err.rfind(b"chunks") < err.rfind(b"jg: 1 files"), err
        print("PTY: worker retry logs and quiet debug output: passed")

        # Rendering failures must clear progress and preserve broken-pipe success.
        code, _out, err = run_terminal(binary, cwd, env, ["find needle"], stderr_tty=True, broken_pipe=True)
        assert code == 0 and b"chunks" in err, (code, err)
        assert not err.rstrip().endswith(b"chunks"), err
        print("PTY: broken-pipe cleanup: passed")

        # A final API error remains visible even when quiet; finish clears before it.
        server.status = 401
        for quiet in (False, True):
            code, out, err = run_terminal(binary, cwd, env, ["find needle", *(["-q"] if quiet else [])], stderr_tty=True)
            assert code == 2 and not out and b"jg:" in err, (code, out, err)
            if quiet:
                assert b"chunks" not in err, err
            assert err.rfind(b"chunks") < err.rfind(b"jg:"), err
        print("PTY: API-error cleanup and quiet diagnostics: passed")

        server.status = 200
        code, out, err = run_terminal(binary, cwd, env, ["find needle", "-t1", "--no-heading"], stderr_tty=True)
        assert code == 1 and not out, (code, out, err)
        assert err.rfind(b"chunks") < err.rfind(b"jg: 1 files"), err
        print("PTY: no-match cleanup: passed")

        # Forced colors must never escape into machine output even with real TTYs.
        for mode in ("--json", "--no-heading"):
            code, out, _err = run_terminal(binary, cwd, env, ["find needle", mode, "--color", "always"], stdout_tty=True)
            assert code == 0 and b"\x1b" not in out, (code, out)
        print("PTY: machine-output color isolation: passed")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--pty", action="store_true", help="also test bounded POSIX PTY wiring")
    options = parser.parse_args()
    check(options.binary.resolve(strict=True), options.pty)
