#!/usr/bin/env python3
"""Exercise the cloud auth entrypoint, real device HTTP routes and CLI isolation."""

import contextlib
from concurrent.futures import ThreadPoolExecutor
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch
import zipfile

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
SKILL = ROOT / "plugins/codex-global/skills/lexmount-browser"
spec = importlib.util.spec_from_file_location("device_auth", SKILL / "scripts/device_auth.py")
auth = importlib.util.module_from_spec(spec)
spec.loader.exec_module(auth)
DEVICE_SECRET = "fixture-private-device-code"
API_SECRET = "fixture-private-api-key"


class CloudAuthTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.credentials = Path(self.directory.name) / "profile/credentials.json"
        self.now = 1000
        self.response = (200, {"project_id": "project-a", "api_key": API_SECRET, "api_base_url": auth.API})
        self.verification = "/connect/device"
        self.redirect = None
        self.calls = []
        outer = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                outer.calls.append((self.path, body))
                if self.path == "/api/connect/device/authorize":
                    code, value = 200, {"device_code": DEVICE_SECRET, "user_code": "ABCD-EFGH", "verification_uri": outer.verification, "expires_in": 60, "interval": 1}
                else:
                    if self.path != "/api/connect/device/token" or body != {"device_code": DEVICE_SECRET}:
                        self.send_error(400)
                        return
                    code, value = outer.response
                self.send_response(code)
                self.send_header("Content-Type", "application/json")
                if outer.redirect:
                    self.send_header("Location", outer.redirect)
                self.end_headers()
                self.wfile.write(json.dumps(value).encode())

            def do_GET(self):
                outer.calls.append((self.path, {"unexpected": "redirect"}))
                self.send_error(400)

            def log_message(self, *args):
                pass

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.console = "http://127.0.0.1:%s" % server.server_port
        self.console_patch = patch.object(auth, "CONSOLE", self.console)
        self.console_patch.start()
        self.addCleanup(self.console_patch.stop)
        self.flow = auth.DeviceFlow(self.credentials, clock=lambda: self.now)

    def call(self, operation, *args, **kwargs):
        with self.flow.locked():
            return getattr(self.flow, operation)(*args, **kwargs)

    def start(self):
        result = self.call("start")
        self.now += 1
        return result

    def test_success_resumes_in_a_new_process_state_and_saves_private_cli_credentials(self):
        public = self.start()
        self.assertEqual(public["user_code"], "ABCD-EFGH")
        self.flow = auth.DeviceFlow(self.credentials, clock=lambda: self.now)
        result = self.call("poll")
        self.assertEqual(result["status"], "authenticated")
        self.assertNotIn(API_SECRET, json.dumps(result))
        self.assertNotIn(DEVICE_SECRET, json.dumps(public))
        self.assertFalse(self.flow.state.exists())
        self.assertEqual(stat.S_IMODE(self.credentials.stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(self.flow.state_dir.stat().st_mode), 0o700)
        saved = auth.private_read(self.credentials)
        self.assertEqual(saved["api_key"], API_SECRET)
        self.assertEqual(saved["api_base_url"], "https://api.lexmount.com")
        self.assertEqual(self.call("status")["status"], "credentials_present")

    def test_pending_start_and_early_poll_do_not_issue_extra_requests(self):
        first = self.call("start")
        self.assertEqual(self.call("start"), first)
        self.assertEqual(self.call("poll")["status"], "authorization_pending")
        self.assertEqual(len(self.calls), 1)
        self.response = 400, {"error": "authorization_pending"}
        self.now += 1
        self.assertEqual(self.call("poll")["retry_after_seconds"], 1)
        self.assertEqual(len(self.calls), 2)

    def test_slow_down_and_transient_failure_preserve_the_same_code(self):
        self.start()
        self.response = 400, {"error": "slow_down"}
        self.assertEqual(self.call("poll")["retry_after_seconds"], 6)
        self.now += 6
        self.response = 503, {"detail": API_SECRET}
        result = self.call("poll")
        self.assertEqual(result["retry_after_seconds"], 12)
        self.assertNotIn(API_SECRET, json.dumps(result))
        self.assertEqual(len([p for p, _ in self.calls if p.endswith("authorize")]), 1)

    def test_denial_and_expiry_preserve_existing_credentials(self):
        for error in ("access_denied", "expired_token"):
            with self.subTest(error=error):
                auth.private_write(self.credentials, {"unchanged": "previous-private-key"})
                previous = self.credentials.read_bytes()
                self.start()
                self.response = 400, {"error": error}
                self.assertIn(self.call("poll")["status"], ("denied", "expired"))
                self.assertEqual(self.credentials.read_bytes(), previous)
                self.assertFalse(self.flow.state.exists())

    def test_expired_local_state_is_not_polled(self):
        self.start()
        self.now += 60
        self.assertEqual(self.call("poll")["status"], "expired")
        self.assertEqual(len(self.calls), 1)
        self.assertFalse(self.credentials.exists())

    def test_project_mismatch_and_foreign_api_never_overwrite_a_previous_identity(self):
        for response in ({"project_id": "wrong", "api_key": API_SECRET, "api_base_url": auth.API},
                         {"project_id": "expected", "api_key": API_SECRET, "api_base_url": "https://api.lexmount.cn"}):
            with self.subTest(response=response["project_id"]):
                auth.private_write(self.credentials, {"unchanged": "old-identity"})
                previous = self.credentials.read_bytes()
                self.call("start", project_id="expected")
                self.now += 1
                self.response = 200, response
                with self.assertRaises(auth.FlowError):
                    self.call("poll")
                self.assertEqual(self.credentials.read_bytes(), previous)
                self.assertFalse(self.flow.state.exists())

    def test_foreign_verification_page_is_rejected(self):
        self.verification = "https://foreign.example/approve"
        with self.assertRaises(auth.FlowError):
            self.call("start")
        self.assertFalse(self.flow.state.exists())

    def test_token_redirect_is_not_followed_with_the_private_device_code(self):
        self.start()
        self.response = 302, {"error": "redirect"}
        self.redirect = self.console + "/unexpected"
        self.assertEqual(self.call("poll")["status"], "failed")
        self.assertFalse(any(path == "/unexpected" for path, _ in self.calls))
        self.assertFalse(self.credentials.exists())

    def test_late_success_after_deadline_cannot_install_credentials(self):
        self.start()
        original = self.flow.request
        def late(*args):
            result = original(*args)
            self.now += 61
            return result
        self.flow.request = late
        self.assertEqual(self.call("poll")["status"], "expired")
        self.assertFalse(self.credentials.exists())

    def test_parallel_starts_create_only_one_authorization(self):
        def start(_):
            flow = auth.DeviceFlow(self.credentials, clock=lambda: self.now)
            with flow.locked():
                return flow.start()
        with ThreadPoolExecutor(max_workers=2) as pool:
            results = list(pool.map(start, range(2)))
        self.assertEqual(results[0], results[1])
        self.assertEqual(len(self.calls), 1)

    def test_separate_credential_files_do_not_share_pending_state(self):
        other = auth.DeviceFlow(self.credentials.with_name("other.json"))
        self.assertNotEqual(self.flow.state, other.state)
        self.start()
        with other.locked():
            self.assertEqual(other.status()["status"], "not_authenticated")

    def test_symlinks_and_public_state_are_rejected(self):
        self.start()
        self.flow.state.chmod(0o644)
        with self.assertRaises(auth.FlowError):
            self.call("status")
        self.flow.state.unlink()
        target = Path(self.directory.name) / "keep.json"
        target.write_text("keep")
        self.credentials.symlink_to(target)
        self.start()
        with self.assertRaises(auth.FlowError):
            self.call("poll")
        self.assertEqual(target.read_text(), "keep")

    def test_cli_entrypoint_redacts_secrets_and_cancellation_does_not_remove_credentials(self):
        env = {"LEXMOUNT_BROWSER_CREDENTIALS_FILE": str(self.credentials)}
        with patch.dict(os.environ, env), patch.dict(os.environ, {"LEXMOUNT_API_KEY": "", "LEXMOUNT_PROJECT_ID": "", "LEXMOUNT_BASE_URL": auth.API}):
            for operation in ("start", "status", "cancel"):
                output = io.StringIO()
                with contextlib.redirect_stdout(output):
                    self.assertEqual(auth.main([operation]), 0)
                result = json.loads(output.getvalue())
                self.assertTrue(result["ok"])
                self.assertNotIn(DEVICE_SECRET, output.getvalue())
                self.assertNotIn(API_SECRET, output.getvalue())
            self.assertFalse(self.flow.state.exists())

    @unittest.skipUnless(os.environ.get("LEXMOUNT_TEST_BINARY"), "set LEXMOUNT_TEST_BINARY to verify released CLI compatibility")
    def test_released_native_cli_reads_the_device_credential(self):
        self.start()
        self.call("poll")
        env = dict(os.environ, LEXMOUNT_BROWSER_CREDENTIALS_FILE=str(self.credentials))
        for name in ("LEXMOUNT_API_KEY", "LEXMOUNT_PROJECT_ID", "LEXMOUNT_BASE_URL"):
            env.pop(name, None)
        result = subprocess.run([os.environ["LEXMOUNT_TEST_BINARY"], "auth", "status"], env=env, capture_output=True, text=True, check=True)
        data = json.loads(result.stdout)
        self.assertTrue(data["data"]["valid"])
        self.assertEqual(data["data"]["api_base_url"], auth.API)
        self.assertNotIn(API_SECRET, result.stdout + result.stderr)


class PluginTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.skill = Path(self.directory.name) / "relocated skill with spaces"
        shutil.copytree(SKILL, self.skill, ignore=shutil.ignore_patterns("bin", "__pycache__"))
        (self.skill / "bin").mkdir()
        binary = self.skill / "bin/browser-cli"
        binary.write_text('#!/bin/sh\nif [ "$1" = --version ]; then echo "browser-cli 1.5.0"; else printf \'{"ok":true,"api":"%s","credentials":"%s"}\\n\' "$LEXMOUNT_BASE_URL" "$LEXMOUNT_BROWSER_CREDENTIALS_FILE"; fi\n')
        binary.chmod(0o755)
        self.env = dict(os.environ, LEXMOUNT_BROWSER_CREDENTIALS_FILE=str(Path(self.directory.name) / "credentials.json"))
        for key in ("LEXMOUNT_API_KEY", "LEXMOUNT_PROJECT_ID", "LEXMOUNT_BASE_URL"):
            self.env.pop(key, None)
        auth.private_write(Path(self.env["LEXMOUNT_BROWSER_CREDENTIALS_FILE"]), {"kind": "api_key", "project_id": "p", "api_key": API_SECRET, "api_base_url": auth.API, "connect_base_url": auth.CONSOLE})

    def run_browser(self, *args):
        return subprocess.run(["sh", str(self.skill / "scripts/browser.sh"), *args], env=self.env, cwd="/", capture_output=True, text=True, timeout=10)

    def test_relocated_entrypoint_uses_fixed_global_api_and_private_profile(self):
        result = self.run_browser("session", "list")
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertEqual(json.loads(result.stdout)["api"], "https://api.lexmount.com")
        self.assertEqual(json.loads(result.stdout)["credentials"], self.env["LEXMOUNT_BROWSER_CREDENTIALS_FILE"])
        self.assertNotIn(API_SECRET, result.stdout + result.stderr)

    def test_environment_and_loopback_overrides_are_rejected(self):
        for args in (("auth", "login", "--browser"), ("auth", "login", "--no-open"), ("auth", "login", "--connect-base-url=https://browser.lexmount.cn")):
            self.assertNotEqual(self.run_browser(*args).returncode, 0)
        self.env["LEXMOUNT_BASE_URL"] = "https://api.lexmount.cn"
        self.assertNotEqual(self.run_browser("session", "list").returncode, 0)
        self.env["LEXMOUNT_BASE_URL"] = "https://api.lexmount.com"
        self.env["LEXMOUNT_API_KEY"] = API_SECRET
        self.assertNotEqual(self.run_browser("session", "list").returncode, 0)

    def test_pending_identity_switch_blocks_browser_operations(self):
        flow = auth.DeviceFlow(Path(self.env["LEXMOUNT_BROWSER_CREDENTIALS_FILE"]))
        with flow.locked():
            auth.private_write(flow.state, {"console": auth.CONSOLE, "device_code": DEVICE_SECRET, "user_code": "ABCD-EFGH", "verification_uri": auth.CONSOLE + "/connect/device", "expires_at": 9999999999, "next_poll_at": 9999999999})
        self.assertNotEqual(self.run_browser("session", "list").returncode, 0)
        self.assertEqual(self.run_browser("auth", "logout").returncode, 0)
        self.assertFalse(flow.state.exists())
        self.assertFalse(flow.credentials.exists())

    def test_help_does_not_require_credentials_or_cancel_pending_auth(self):
        self.assertEqual(self.run_browser("session", "--help").returncode, 0)
        before = Path(self.env["LEXMOUNT_BROWSER_CREDENTIALS_FILE"]).read_bytes()
        self.assertEqual(self.run_browser("auth", "logout", "--help").returncode, 0)
        self.assertEqual(Path(self.env["LEXMOUNT_BROWSER_CREDENTIALS_FILE"]).read_bytes(), before)

    def test_package_is_reproducible_and_contains_no_runtime_credentials_or_mcp(self):
        command = ["sh", str(ROOT / "scripts/package-global-plugin.sh")]
        subprocess.run(command, check=True, capture_output=True)
        path = ROOT / "dist/lexmount-browser-global-4.0.0.zip"
        first = hashlib.sha256(path.read_bytes()).hexdigest()
        subprocess.run(command, check=True, capture_output=True)
        self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), first)
        with zipfile.ZipFile(path) as archive:
            names = archive.namelist()
            self.assertIn("plugin.json", names)
            self.assertFalse(any("mcp" in name or "/bin/" in name or "credentials.json" in name or "pending.json" in name for name in names))
            manifest = json.loads(archive.read("plugin.json"))
            self.assertEqual(manifest["version"], "4.0.0")


if __name__ == "__main__":
    unittest.main(verbosity=2)
