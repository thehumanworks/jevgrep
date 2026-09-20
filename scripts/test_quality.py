#!/usr/bin/env python3
"""Non-network regression tests for QA gates and release-package helpers."""

import hashlib
import importlib.util
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]


def load_helper(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


pins = load_helper("check-toolchain")
package = load_helper("verify-package")


class PinTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.write(
            "rust-toolchain.toml",
            '[toolchain]\nchannel="1.98.1"\ncomponents=["rustfmt","clippy"]\n',
        )
        self.write(
            "Cargo.toml",
            '[package]\nversion="0.2.0"\nrust-version="1.98.1"\nedition="2021"\n',
        )
        self.write("mise.toml", '[tools]\nrust="1.98.1"\n')
        self.write("Cargo.lock", '[[package]]\nname="jevgrep"\nversion="0.2.0"\n')

    def write(self, name, content):
        (self.root / name).write_text(content)

    def test_exact_matching_pins(self):
        self.assertEqual(pins.check_pins(self.root), "1.98.1")

    def test_floating_pin_rejected(self):
        self.write("rust-toolchain.toml", '[toolchain]\nchannel="stable"\n')
        with self.assertRaisesRegex(ValueError, "exact"):
            pins.check_pins(self.root)

    def test_each_secondary_pin_must_match(self):
        for name in ("Cargo.toml", "mise.toml"):
            with self.subTest(name=name):
                path = self.root / name
                original = path.read_text()
                path.write_text(original.replace("1.98.1", "1.98.0"))
                with self.assertRaisesRegex(ValueError, "expected"):
                    pins.check_pins(self.root)
                path.write_text(original)

    def test_components_required(self):
        self.write(
            "rust-toolchain.toml",
            '[toolchain]\nchannel="1.98.1"\ncomponents=["rustfmt"]\n',
        )
        with self.assertRaisesRegex(ValueError, "clippy"):
            pins.check_pins(self.root)

    def test_lock_version_matches_package(self):
        self.write("Cargo.lock", '[[package]]\nname="jevgrep"\nversion="0.1.0"\n')
        with self.assertRaisesRegex(ValueError, "Cargo.lock"):
            pins.check_pins(self.root)

    def test_edition_unchanged(self):
        path = self.root / "Cargo.toml"
        path.write_text(path.read_text().replace("2021", "2024"))
        with self.assertRaisesRegex(ValueError, "edition"):
            pins.check_pins(self.root)


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.name = "jevgrep-v0.2.0-x86_64-unknown-linux-musl"
        self.archive = self.root / f"{self.name}.tar.gz"

    def create_archive(self, extra=None, executable=True, symlink=False):
        with tarfile.open(self.archive, "w:gz") as output:
            folder = tarfile.TarInfo(self.name)
            folder.type = tarfile.DIRTYPE
            output.addfile(folder)
            for leaf in ("jg", "README.md"):
                member = tarfile.TarInfo(f"{self.name}/{leaf}")
                member.mode = 0o755 if executable else 0o644
                if symlink and leaf == "jg":
                    member.type = tarfile.SYMTYPE
                    member.linkname = "/bin/sh"
                    output.addfile(member)
                else:
                    member.size = 7
                    output.addfile(member, io.BytesIO(b"fixture"))
            if extra:
                output.addfile(tarfile.TarInfo(extra))
        digest = hashlib.sha256(self.archive.read_bytes()).hexdigest()
        Path(str(self.archive) + ".sha256").write_text(
            f"{digest}  {self.archive.name}\n"
        )

    def test_valid_checksum_and_layout(self):
        self.create_archive()
        package.verify_checksum(self.archive)
        binary = package.extract_package(self.archive, self.root, self.name)
        self.assertEqual(binary.read_bytes(), b"fixture")
        self.assertTrue(os.access(binary, os.X_OK))
        self.assertEqual((binary.parent / "README.md").read_bytes(), b"fixture")

    def test_package_script_preserves_public_contract_and_target_cache(self):
        scripts = self.root / "scripts"
        scripts.mkdir()
        for name in ("package.sh", "check-toolchain.py"):
            shutil.copyfile(ROOT / "scripts" / name, scripts / name)
        (self.root / "rust-toolchain.toml").write_text(
            '[toolchain]\nchannel="1.98.1"\ncomponents=["rustfmt","clippy"]\n'
        )
        (self.root / "Cargo.toml").write_text(
            '[package]\nversion="0.2.0"\nrust-version="1.98.1"\nedition="2021"\n'
        )
        (self.root / "mise.toml").write_text('[tools]\nrust="1.98.1"\n')
        (self.root / "Cargo.lock").write_text(
            '[[package]]\nname="jevgrep"\nversion="0.2.0"\n'
        )
        (self.root / "README.md").write_text("packaged readme")
        target = "x86_64-unknown-linux-musl"
        cache = self.root / "external-target-cache"
        source = cache / target / "release/jg"
        source.parent.mkdir(parents=True)
        source.write_text("binary fixture")
        result = subprocess.run(
            ["/bin/bash", str(scripts / "package.sh"), target],
            env={
                "PATH": f"{Path(sys.executable).parent}:{os.defpath}",
                "CARGO_TARGET_DIR": str(cache),
            },
            check=False,
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), self.name)
        archive = self.root / "target/dist" / self.archive.name
        package.verify_checksum(archive)
        extracted = package.extract_package(archive, self.root, self.name)
        self.assertEqual(extracted.read_text(), "binary fixture")
        self.assertEqual(
            (extracted.parent / "README.md").read_text(), "packaged readme"
        )

    def test_corrupt_archive_rejected(self):
        self.create_archive()
        with self.archive.open("ab") as output:
            output.write(b"corrupt")
        with self.assertRaisesRegex(ValueError, "SHA256"):
            package.verify_checksum(self.archive)

    def test_wrong_checksum_filename_rejected(self):
        self.create_archive()
        sidecar = Path(str(self.archive) + ".sha256")
        sidecar.write_text(
            sidecar.read_text().replace(self.archive.name, "other.tar.gz")
        )
        with self.assertRaisesRegex(ValueError, "wrong archive name"):
            package.verify_checksum(self.archive)

    def test_extra_traversal_or_duplicate_member_rejected(self):
        for extra in ("../escaped", f"{self.name}/extra", f"{self.name}/jg"):
            with self.subTest(extra=extra):
                self.create_archive(extra=extra)
                with self.assertRaisesRegex(ValueError, "exactly"):
                    package.extract_package(self.archive, self.root, self.name)
        self.assertFalse((self.root.parent / "escaped").exists())

    def test_links_rejected(self):
        self.create_archive(symlink=True)
        with self.assertRaisesRegex(ValueError, "regular files"):
            package.extract_package(self.archive, self.root, self.name)

    def test_executable_mode_required(self):
        self.create_archive(executable=False)
        with self.assertRaisesRegex(ValueError, "not executable"):
            package.extract_package(self.archive, self.root, self.name)

    def test_static_metadata(self):
        for headers, dynamic, succeeds in (
            ("LOAD", "No dynamic section", True),
            ("LOAD", "FLAGS_1 PIE", True),
            ("LOAD INTERP", "No dynamic section", False),
            ("LOAD", "(NEEDED) libc.so", False),
            ("", "", False),
        ):
            with self.subTest(headers=headers, dynamic=dynamic):
                results = [mock.Mock(stdout=headers), mock.Mock(stdout=dynamic)]
                with mock.patch.object(package.subprocess, "run", side_effect=results):
                    if succeeds:
                        package.check_static(Path("jg"))
                    else:
                        with self.assertRaises(ValueError):
                            package.check_static(Path("jg"))

    def test_downloaded_binary_is_used_for_help_version_and_fake_api(self):
        self.create_archive()
        (self.root / "Cargo.toml").write_text('[package]\nversion="0.2.0"\n')
        outputs = [
            mock.Mock(stdout="Usage: jg QUERY\n", stderr=""),
            mock.Mock(stdout="jg 0.2.0\n", stderr=""),
            mock.Mock(),
        ]
        with (
            mock.patch.object(package, "ROOT", self.root),
            mock.patch.object(package, "check_static") as static,
            mock.patch.object(package.subprocess, "run", side_effect=outputs) as run,
            mock.patch("builtins.print"),
        ):
            package.verify(self.archive, "x86_64-unknown-linux-musl")
        static.assert_called_once()
        extracted_binary = str(static.call_args.args[0])
        self.assertNotEqual(extracted_binary, str(ROOT / "target/release/jg"))
        self.assertEqual(run.call_args_list[0].args[0], [extracted_binary, "--help"])
        self.assertEqual(run.call_args_list[1].args[0], [extracted_binary, "--version"])
        self.assertEqual(
            run.call_args_list[2].args[0],
            [
                sys.executable,
                str(self.root / "scripts/terminal-smoke.py"),
                "--binary",
                extracted_binary,
            ],
        )
        for call in run.call_args_list:
            self.assertEqual(call.kwargs["env"]["JG_NO_FNOX"], "1")
            self.assertTrue(call.kwargs["check"])
            self.assertLessEqual(call.kwargs["timeout"], 180)

    def test_wrong_packaged_version_stops_fake_api_smoke(self):
        self.create_archive()
        (self.root / "Cargo.toml").write_text('[package]\nversion="0.2.0"\n')
        outputs = [
            mock.Mock(stdout="Usage: jg QUERY\n", stderr=""),
            mock.Mock(stdout="jg 0.1.0\n", stderr=""),
        ]
        with (
            mock.patch.object(package, "ROOT", self.root),
            mock.patch.object(package, "check_static"),
            mock.patch.object(package.subprocess, "run", side_effect=outputs) as run,
            self.assertRaisesRegex(ValueError, "version does not match"),
        ):
            package.verify(self.archive, "x86_64-unknown-linux-musl")
        self.assertEqual(run.call_count, 2)

    def test_smoke_environment_does_not_inherit_credentials(self):
        with mock.patch.dict(
            os.environ,
            {
                "TYPESAFE_API_KEY": "secret",
                "CHATGPT_ACCOUNT_ID": "account-secret",
                "CHATGPT_ACCESS_TOKEN": "token-secret",
                "CODEX_HOME": "/private/codex-cache",
                "JG_BACKEND": "chatgpt",
                "GH_TOKEN": "secret",
                "JG_MODEL": "user-model",
                "GIT_CONFIG_COUNT": "5",
            },
        ):
            env = package.smoke_environment(self.root / "home")
        for name in (
            "TYPESAFE_API_KEY", "CHATGPT_ACCOUNT_ID", "CHATGPT_ACCESS_TOKEN",
            "CODEX_HOME", "JG_BACKEND", "GH_TOKEN", "JG_MODEL", "GIT_CONFIG_COUNT",
        ):
            self.assertNotIn(name, env)
        self.assertEqual(env["JG_NO_FNOX"], "1")
        self.assertEqual(env["GIT_CONFIG_GLOBAL"], os.devnull)
        self.assertTrue(Path(env["XDG_CONFIG_HOME"]).is_dir())


class EntrypointTests(unittest.TestCase):
    def run_check(self, fail_clippy=False, optional_overrides=True):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        (root / "scripts").mkdir()
        shutil.copyfile(ROOT / "scripts/check.sh", root / "scripts/check.sh")
        binary = root / "bin"
        binary.mkdir()
        log = root / "calls.jsonl"
        user_home = root / "user-home"
        user_home.mkdir()
        (user_home / "sentinel").write_text("private user file")
        # Stubs record actual child environments and read paths. No compiler or
        # API is invoked, and all generated files stay inside this fixture.
        stub = f"""#!{sys.executable}
import json, os, pathlib, sys
name = pathlib.Path(sys.argv[0]).name
args = sys.argv[1:]
with open({str(log)!r}, "a") as output:
    output.write(json.dumps({{"name": name, "args": args, "env": dict(os.environ),
        "home_sentinel": (pathlib.Path.home() / "sentinel").exists()}}) + "\\n")
if name == "python3" and "--print-rust" in args:
    print("1.98.1")
elif name == "actionlint" and "--version" in args:
    print("1.7.12")
elif name == "shellcheck" and "--version" in args:
    print("version: 0.11.0")
elif name == "cargo" and "clippy" in args and {fail_clippy!r}:
    sys.exit(17)
elif name == "cargo" and "build" in args:
    target = pathlib.Path(os.environ.get("CARGO_TARGET_DIR", {str(root / "target")!r})) / "release" / "jg"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("#!/bin/sh\\nexit 0\\n")
    target.chmod(0o755)
"""
        for tool in ("python3", "cargo", "rustc", "actionlint", "shellcheck", "git"):
            path = binary / tool
            path.write_text(stub)
            path.chmod(0o755)
        env = dict(
            os.environ,
            PATH=f"{binary}:{os.defpath}",
            HOME=str(user_home),
            TYPESAFE_API_KEY="real-secret",
            CHATGPT_ACCOUNT_ID="account-secret",
            CHATGPT_ACCESS_TOKEN="token-secret",
            CODEX_HOME=str(user_home / ".codex"),
            JG_BACKEND="chatgpt",
            GH_TOKEN="real-secret",
            JG_MODEL="private",
            RUSTUP_TOOLCHAIN="nightly",
            GIT_CONFIG_COUNT="99",
            CARGO_HOME=str(root / "cargo-cache"),
            RUSTUP_HOME=str(root / "rustup"),
            CARGO_TARGET_DIR=str(root / "build-cache"),
        )
        if not optional_overrides:
            for variable in ("CARGO_TARGET_DIR", "RUSTC_WRAPPER", "SCCACHE_DIR", "CC", "AR"):
                env.pop(variable, None)
        result = subprocess.run(
            ["/bin/bash", str(root / "scripts/check.sh")],
            env=env,
            check=False,
            capture_output=True,
            text=True,
            timeout=30,
        )
        calls = [json.loads(line) for line in log.read_text().splitlines()]
        return result, calls, env

    def test_isolation_and_explicit_pin_for_every_cargo_command(self):
        result, calls, original = self.run_check()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for call in calls:
            env = call["env"]
            self.assertFalse(call["home_sentinel"])
            self.assertNotEqual(env["HOME"], original["HOME"])
            for secret in (
                "TYPESAFE_API_KEY",
                "CHATGPT_ACCOUNT_ID",
                "CHATGPT_ACCESS_TOKEN",
                "CODEX_HOME",
                "JG_BACKEND",
                "GH_TOKEN",
                "JG_MODEL",
                "RUSTUP_TOOLCHAIN",
                "GIT_CONFIG_COUNT",
            ):
                self.assertNotIn(secret, env)
            self.assertEqual(env["JG_NO_FNOX"], "1")
            self.assertEqual(env["GIT_CONFIG_GLOBAL"], "/dev/null")
            for cache in ("CARGO_HOME", "RUSTUP_HOME", "CARGO_TARGET_DIR"):
                self.assertEqual(env[cache], original[cache])
            if call["name"] in ("cargo", "rustc"):
                self.assertEqual(call["args"][0], "+1.98.1")
        cargo = [call["args"] for call in calls if call["name"] == "cargo"]
        self.assertIn(["+1.98.1", "test", "--locked", "--doc", "--all-features"], cargo)
        self.assertIn(
            ["+1.98.1", "doc", "--locked", "--no-deps", "--all-features"], cargo
        )
        self.assertFalse(any("--ignored" in args for args in cargo))
        smoke = [call for call in calls if "scripts/terminal-smoke.py" in call["args"]]
        self.assertEqual(len(smoke), 1)
        self.assertIn("--pty", smoke[0]["args"])
        self.assertFalse(
            Path(calls[0]["env"]["HOME"]).exists(), "temporary HOME must be cleaned"
        )

    def test_isolation_without_optional_overrides_on_system_bash(self):
        # The native macOS lane exercises this with Bash 3.2, where expanding an
        # empty array under nounset aborts before any required checks are run.
        result, calls, _ = self.run_check(optional_overrides=False)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(any(call["name"] == "cargo" and "build" in call["args"] for call in calls))
        for call in calls:
            self.assertEqual(call["env"]["JG_NO_FNOX"], "1")
            self.assertNotIn("CARGO_TARGET_DIR", call["env"])

    def test_first_failed_gate_stops_later_checks(self):
        result, calls, _ = self.run_check(fail_clippy=True)
        self.assertEqual(result.returncode, 17, result.stdout + result.stderr)
        cargo = [call["args"] for call in calls if call["name"] == "cargo"]
        self.assertTrue(any("clippy" in args for args in cargo))
        self.assertFalse(any("test" in args or "build" in args for args in cargo))


class InstallerTests(unittest.TestCase):
    def test_bad_checksum_stops_before_extraction_or_installation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "bin"
            binary.mkdir()
            curl = binary / "curl"
            curl.write_text(
                '#!/bin/sh\nwhile [ "$1" != "-o" ]; do shift; done\n'
                'printf corrupt > "$2"\n'
            )
            curl.chmod(0o755)
            sentinel = root / "tar-was-run"
            tar = binary / "tar"
            tar.write_text(f"#!/bin/sh\ntouch '{sentinel}'\nexit 99\n")
            tar.chmod(0o755)
            result = subprocess.run(
                [
                    "/bin/bash",
                    str(ROOT / "scripts/install-qa-tools.sh"),
                    str(root / "installed"),
                ],
                env={"PATH": f"{binary}:{os.defpath}"},
                check=False,
                capture_output=True,
                text=True,
                timeout=30,
            )
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn("checksum mismatch", result.stderr)
            self.assertFalse(sentinel.exists())
            self.assertEqual(list((root / "installed").iterdir()), [])


class WorkflowTests(unittest.TestCase):
    def test_external_actions_are_reviewed_immutable_pins(self):
        reviewed = {
            "actions/checkout": "3d3c42e5aac5ba805825da76410c181273ba90b1",
            "actions/setup-python": "5fda3b95a4ea91299a34e894583c3862153e4b97",
            "actions/upload-artifact": "043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
            "actions/download-artifact": "3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c",
        }
        for workflow in (ROOT / ".github/workflows").glob("*.yml"):
            for action, sha in re.findall(
                r"uses: ([\w/-]+)@(\S+)", workflow.read_text()
            ):
                self.assertEqual(
                    sha, reviewed[action], f"unreviewed action: {workflow}: {action}"
                )

    def test_shared_gates_and_publish_permissions(self):
        workflows = ROOT / ".github/workflows"
        shared = (workflows / "checks.yml").read_text()
        self.assertNotIn("concurrency:", shared)
        self.assertIn("needs: [quality, native]", shared)
        self.assertIn("if: always()", shared)
        self.assertIn("scripts/check.sh verify", shared)
        self.assertLess(
            shared.index("actions/download-artifact@"),
            shared.index("scripts/check.sh verify"),
        )
        for target in package.TARGETS:
            self.assertIn(f"target: {target}", shared)
        for name in ("ci.yml", "release.yml"):
            text = (workflows / name).read_text()
            self.assertIn("uses: ./.github/workflows/checks.yml", text)
            self.assertIn("concurrency:", text)
            self.assertNotIn("pull_request_target", text)
        release = (workflows / "release.yml").read_text()
        self.assertIn("needs: checks", release)
        self.assertIn(
            "if: github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')",
            release,
        )
        self.assertEqual(release.count("contents: write"), 1)
        self.assertNotIn("contents: write", shared)
        self.assertNotIn("contents: write", (workflows / "ci.yml").read_text())
        self.assertIn('test "v${version}" = "$GITHUB_REF_NAME"', release)

    def test_installer_and_gate_tool_versions_agree(self):
        installer = (ROOT / "scripts/install-qa-tools.sh").read_text()
        check = (ROOT / "scripts/check.sh").read_text()
        for version in ("1.7.12", "0.11.0"):
            self.assertIn(version, installer)
            self.assertIn(version, check)
        digests = re.findall(r"(?:action|shell)_sha=([0-9a-f]+)", installer)
        self.assertEqual(len(digests), 8)
        self.assertTrue(all(len(digest) == 64 for digest in digests))
        self.assertIn('[[ ${actual%% *} == "$expected" ]]', installer)


class CorrectnessGateTests(unittest.TestCase):
    CLIPPY = 'cargo +"$pin" clippy --locked --all-targets --all-features -- -D warnings'

    def test_clippy_policy_is_correctness_first(self):
        cargo = (ROOT / "Cargo.toml").read_text()
        self.assertIn('unsafe_code = "forbid"', cargo)
        self.assertIn('correctness = "deny"', cargo)
        self.assertIn('suspicious = "deny"', cargo)
        self.assertIn('string_slice = "deny"', cargo)
        self.assertIn('lossy_float_literal = "deny"', cargo)
        self.assertNotIn('pedantic = "deny"', cargo)
        self.assertNotIn('restriction = "deny"', cargo)
        self.assertNotIn('nursery = "deny"', cargo)

    def test_check_and_precommit_use_the_same_clippy_invocation(self):
        check = (ROOT / "scripts/check.sh").read_text()
        hook = (ROOT / "scripts/pre-commit.sh").read_text()
        self.assertIn(self.CLIPPY, check)
        self.assertIn(self.CLIPPY, hook)
        self.assertIn("scripts/githooks/*", check)
        self.assertIn("cargo +\"$pin\" fmt --all -- --check", hook)
        self.assertIn(
            'cargo +"$pin" test --locked --all-targets --all-features', hook
        )
        self.assertIn("scripts/check.sh", hook)

    def test_precommit_is_installable_and_avoids_secret_tools(self):
        hook = (ROOT / "scripts/pre-commit.sh").read_text()
        installer = (ROOT / "scripts/install-git-hooks.sh").read_text()
        wrapper = (ROOT / "scripts/githooks/pre-commit").read_text()
        self.assertIn("core.hooksPath", installer)
        self.assertIn("scripts/githooks", installer)
        self.assertIn("scripts/pre-commit.sh", wrapper)
        for text in (hook, installer, wrapper):
            for banned in ("fnox", "1Password", "op run", ".claude", "agent-store"):
                self.assertNotIn(banned, text)
        readme = (ROOT / "README.md").read_text()
        self.assertIn("scripts/install-git-hooks.sh", readme)
        self.assertIn("scripts/pre-commit.sh", readme)
        for adr in (
            "0006-clippy-correctness.md",
            "0007-tests-as-specification.md",
            "0008-pre-commit-hook.md",
        ):
            self.assertTrue((ROOT / "docs/adr" / adr).is_file(), adr)


if __name__ == "__main__":
    unittest.main()
