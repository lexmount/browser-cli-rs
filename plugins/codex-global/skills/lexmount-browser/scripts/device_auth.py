#!/usr/bin/env python3
"""Short-lived Global device authorization commands for cloud agent workspaces."""

import argparse
import contextlib
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import re
import stat
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

CONSOLE = "https://browser.lexmount.com"
API = "https://api.lexmount.com"
MAX_RESPONSE = 65536


class FlowError(Exception):
    pass


def credential_path():
    override = os.environ.get("LEXMOUNT_BROWSER_CREDENTIALS_FILE")
    path = Path(override) if override else Path.home() / ".config/lexmount/browser-cli/sites/global-cloud/credentials.json"
    if not path.is_absolute():
        raise FlowError("Use an absolute credential-file path.")
    return path


def private_read(path):
    flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
    with os.fdopen(os.open(path, flags), "r") as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
            raise FlowError("Private authorization files must be owned by this user with mode 0600.")
        payload = json.loads(stream.read(MAX_RESPONSE + 1))
        if not isinstance(payload, dict):
            raise FlowError("Invalid private authorization file.")
        return payload


def private_write(path, value):
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    if path.is_symlink():
        raise FlowError("Refusing to overwrite an authorization symlink.")
    descriptor, temporary = tempfile.mkstemp(prefix=".device-", dir=path.parent)
    try:
        with os.fdopen(descriptor, "w") as stream:
            json.dump(value, stream)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        with contextlib.suppress(FileNotFoundError):
            os.unlink(temporary)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, file, code, message, headers, new_url):
        return None


def post(path, body):
    request = urllib.request.Request(CONSOLE + path, data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json", "User-Agent": "LexMount-DeviceAuth/4.0.0"}, method="POST")
    opener = urllib.request.build_opener(NoRedirect())
    try:
        response = opener.open(request, timeout=15)
    except urllib.error.HTTPError as response_error:
        response = response_error
    with response:
        data = response.read(MAX_RESPONSE + 1)
        if len(data) > MAX_RESPONSE:
            raise FlowError("Authorization response exceeded the supported size.")
        try:
            payload = json.loads(data)
        except (ValueError, UnicodeError):
            raise FlowError("Authorization service returned an invalid response.") from None
        if not isinstance(payload, dict):
            raise FlowError("Authorization service returned an invalid response.")
        return response.code, payload


class DeviceFlow:
    def __init__(self, credentials, request=post, clock=time.time):
        self.credentials = Path(credentials)
        profile = hashlib.sha256(str(self.credentials.absolute()).encode()).hexdigest()[:16]
        self.state_dir = self.credentials.parent / (".global-cloud-device-" + profile)
        self.state = self.state_dir / "pending.json"
        self.request = request
        self.clock = clock

    @contextlib.contextmanager
    def locked(self):
        self.state_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
        info = self.state_dir.lstat()
        if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
            raise FlowError("Authorization state requires a private directory owned by this user.")
        descriptor = os.open(self.state_dir / "flow.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        with os.fdopen(descriptor, "r+") as stream:
            fcntl.flock(stream, fcntl.LOCK_EX)
            yield

    def forget(self):
        with contextlib.suppress(FileNotFoundError):
            self.state.unlink()

    def load(self):
        try:
            state = private_read(self.state)
        except FileNotFoundError:
            return None
        if state.get("console") != CONSOLE or not isinstance(state.get("expires_at"), (int, float)):
            raise FlowError("Authorization state belongs to a different console or is invalid.")
        return state

    def pending(self, state):
        return {"status": "authorization_pending", "verification_uri": state["verification_uri"],
            "user_code": state["user_code"], "expires_in": max(0, math.ceil(state["expires_at"] - self.clock())),
            "retry_after_seconds": max(0, math.ceil(state["next_poll_at"] - self.clock()))}

    def start(self, client_name="Codex Cloud", project_id=None):
        state = self.load()
        if state and state["expires_at"] > self.clock():
            if state.get("project_id") != project_id:
                raise FlowError("A different project authorization is pending; cancel it before starting another.")
            return self.pending(state)
        self.forget()
        started = self.clock()
        code, payload = self.request("/api/connect/device/authorize", {"client_name": client_name})
        if code != 200:
            raise FlowError("Device authorization could not start (HTTP %s)." % code)
        secret, short_code = payload.get("device_code"), payload.get("user_code")
        verify = urllib.parse.urljoin(CONSOLE + "/", payload.get("verification_uri", "/connect/device"))
        location, selected = urllib.parse.urlsplit(verify), urllib.parse.urlsplit(CONSOLE)
        if not isinstance(secret, str) or not secret or not isinstance(short_code, str) or not re.fullmatch(r"[A-Za-z0-9-]{4,64}", short_code) or secret == short_code:
            raise FlowError("Device authorization returned invalid codes.")
        if (location.scheme, location.netloc) != (selected.scheme, selected.netloc) or secret in urllib.parse.unquote(verify):
            raise FlowError("Device authorization returned an invalid verification URL.")
        lifetime, interval = payload.get("expires_in", 600), payload.get("interval", 5)
        if type(lifetime) is not int or lifetime <= 0 or type(interval) is not int or interval < 0:
            raise FlowError("Device authorization returned invalid timing.")
        state = {"console": CONSOLE, "device_code": secret, "user_code": short_code,
            "verification_uri": verify, "expires_at": started + min(lifetime, 600),
            "interval": max(1, interval), "next_poll_at": self.clock() + max(1, interval), "project_id": project_id}
        private_write(self.state, state)
        return self.pending(state)

    def poll(self):
        state = self.load()
        if not state:
            raise FlowError("No device authorization is pending. Start authorization first.")
        if state["expires_at"] <= self.clock():
            self.forget()
            return {"status": "expired", "retry_safe": False}
        if state["next_poll_at"] > self.clock():
            return self.pending(state)
        try:
            code, payload = self.request("/api/connect/device/token", {"device_code": state["device_code"]})
        except (urllib.error.URLError, TimeoutError):
            code, payload = 503, {}
        if state["expires_at"] <= self.clock():
            self.forget()
            return {"status": "expired", "retry_safe": False}
        if code >= 500 or code == 429:
            state["interval"] = min(30, max(5, state["interval"] * 2))
        elif code != 200:
            error = payload.get("error")
            if error == "slow_down":
                state["interval"] += 5
            elif error != "authorization_pending":
                self.forget()
                return {"status": {"access_denied": "denied", "expired_token": "expired"}.get(error, "failed"), "retry_safe": False}
        else:
            if state["expires_at"] <= self.clock():
                self.forget()
                return {"status": "expired", "retry_safe": False}
            try:
                credentials = self.extract(payload)
                if state.get("project_id") and credentials["project_id"] != state["project_id"]:
                    raise FlowError("Authorized project did not match the requested project.")
                private_write(self.credentials, credentials)
            finally:
                self.forget()
            return {"status": "authenticated", "credentials_saved": True, "credentials_file": str(self.credentials),
                "project_id": credentials["project_id"], "api_base_url": API}
        state["next_poll_at"] = self.clock() + state["interval"]
        private_write(self.state, state)
        return self.pending(state)

    @staticmethod
    def extract(payload):
        for value in [payload] + [payload.get(key) for key in ("credential", "credentials", "token", "env")]:
            if not isinstance(value, dict):
                continue
            project = value.get("project_id", value.get("projectId", value.get("LEXMOUNT_PROJECT_ID", payload.get("project_id"))))
            key = value.get("api_key", value.get("apiKey", value.get("LEXMOUNT_API_KEY", payload.get("api_key"))))
            if not isinstance(project, str) or not project.strip() or not isinstance(key, str) or not key.strip():
                continue
            api = value.get("api_base_url", value.get("apiBaseUrl", payload.get("api_base_url", API)))
            if not isinstance(api, str) or api.rstrip("/") != API:
                raise FlowError("Authorization returned credentials for a different API site.")
            scopes = value.get("scopes", value.get("scope", payload.get("scopes", [])))
            if isinstance(scopes, str):
                scopes = scopes.split()
            if not isinstance(scopes, list) or any(not isinstance(scope, str) for scope in scopes):
                raise FlowError("Authorization returned invalid credential scopes.")
            expiry = value.get("expires_at", value.get("expiresAt"))
            if expiry is not None and not isinstance(expiry, str):
                raise FlowError("Authorization returned an invalid credential expiry.")
            return {"kind": "api_key", "project_id": project, "api_key": key, "api_base_url": API,
                "scopes": scopes, "expires_at": expiry,
                "source": "connect_device_global_cloud", "connect_base_url": CONSOLE}
        raise FlowError("Authorization returned invalid credentials.")

    def status(self):
        state = self.load()
        if state:
            if state["expires_at"] <= self.clock():
                self.forget()
                return {"status": "expired", "retry_safe": False}
            return self.pending(state)
        if self.credentials.exists():
            credentials = private_read(self.credentials)
            if credentials.get("kind") != "api_key" or credentials.get("connect_base_url") != CONSOLE:
                raise FlowError("Credentials belong to another authentication profile.")
            self.extract(credentials)
            return {"status": "credentials_present", "credentials_file": str(self.credentials), "next": "Run browser.sh doctor to verify API access."}
        return {"status": "not_authenticated"}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("start", "poll", "status", "cancel", "logout", "check"))
    parser.add_argument("--client-name", default="Codex Cloud")
    parser.add_argument("--project-id")
    args = parser.parse_args(argv)
    try:
        if os.environ.get("LEXMOUNT_API_KEY") or os.environ.get("LEXMOUNT_PROJECT_ID"):
            raise FlowError("Unset API-key environment credentials before device authorization.")
        if os.environ.get("LEXMOUNT_BASE_URL", API).rstrip("/") != API:
            raise FlowError("LEXMOUNT_BASE_URL conflicts with the Global cloud edition.")
        flow = DeviceFlow(credential_path())
        with flow.locked():
            if args.operation == "start":
                result = flow.start(args.client_name, args.project_id)
            elif args.operation in ("cancel", "logout"):
                flow.forget()
                if args.operation == "logout":
                    with contextlib.suppress(FileNotFoundError):
                        flow.credentials.unlink()
                result = {"status": "logged_out" if args.operation == "logout" else "cancelled"}
            elif args.operation == "check":
                result = flow.status()
                if result["status"] != "credentials_present":
                    raise FlowError("Finish device authorization before browser operations.")
            else:
                result = getattr(flow, args.operation)()
        ok = result["status"] not in ("failed", "denied", "expired")
        print(json.dumps({"ok": ok, "data": result}))
        return 0 if ok else 1
    except FlowError as error:
        print(json.dumps({"ok": False, "error": "device_authorization_failed", "message": str(error)}))
        return 1
    except (OSError, ValueError, TypeError, KeyError):
        # Neither upstream bodies nor private file contents belong in task output.
        print(json.dumps({"ok": False, "error": "device_authorization_failed",
            "message": "Device authorization could not complete. Check Global connectivity and private file permissions; use status or explicitly restart after denial or expiry."}))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
