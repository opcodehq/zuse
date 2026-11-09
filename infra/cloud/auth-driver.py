#!/usr/bin/env python3
"""Private stdin/stdout authority protocol. Never run with stdout connected to a log."""
import base64
from datetime import datetime
import json
import os
from pathlib import Path
import re
import selectors
import signal
import subprocess
import sys
import tempfile
import time
import urllib.request


class AuthFailure(Exception):
    pass


def emit(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)


def verification_url(agent, value):
    from urllib.parse import urlsplit
    try:
        url = urlsplit(value)
        port = url.port
    except ValueError:
        raise AuthFailure("invalid_verification_url") from None
    hosts = {"codex": {"auth.openai.com", "chatgpt.com"},
             "grok": {"auth.x.ai", "accounts.x.ai", "grok.com"}}
    if url.scheme != "https" or url.hostname not in hosts.get(agent, set()) or url.username or url.password or url.fragment or port not in (None, 443):
        raise AuthFailure("invalid_verification_url")
    return value


def parse_device_output(agent, text):
    text = re.sub(r"\x1b\][^\x07]*(?:\x07|\x1b\\)", "", text)
    text = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", text).replace("\r", "")
    urls = re.findall(r"https://[^\s<>\"']+", text)
    codes = re.findall(r"(?:one-time|device|verification|user)\s+code(?:\s*\([^\n)]*\))?(?:\s*[:=]\s*|\s*\n\s*)([A-Z0-9][A-Z0-9-]{3,31})\b", text, re.I)
    if not codes:
        codes = re.findall(r"(?:^|\n)\s*(?:CODE\s*:\s*)?((?=[A-Z0-9-]{4,32}\s*(?:\n|$))(?=[A-Z0-9-]*[0-9-])[A-Z0-9][A-Z0-9-]{3,31})\s*(?=\n|$)", text, re.M)
    if not urls or not codes:
        return None
    for url in urls:
        try:
            verified = verification_url(agent, url.rstrip(".,);"))
            return {"event": "device-code", "verificationUrl": verified, "verificationCode": codes[0].upper()}
        except AuthFailure:
            continue
    raise AuthFailure("invalid_verification_url")


def isolated_environment(home):
    # Inherited model, cloud, Git and proxy credentials must never verify another key.
    env = {"PATH": os.environ.get("PATH", "/usr/local/bin:/usr/bin:/bin"), "LANG": "C.UTF-8", "HOME": str(home),
           "CODEX_HOME": str(home / ".codex"), "GROK_HOME": str(home / ".grok"),
           "CLAUDE_CONFIG_DIR": str(home / ".claude"), "XDG_CONFIG_HOME": str(home / ".config"),
           "DISABLE_AUTOUPDATER": "1", "NO_COLOR": "1", "BROWSER": "/bin/false"}
    return env


def binary(agent):
    return os.environ.get("ZUSE_AUTH_" + agent.upper() + "_BINARY", agent)


def run(command, home, env, timeout=30, input_text=None):
    try:
        result = subprocess.run(command, cwd=home, env=env, input=input_text, stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, text=True, timeout=timeout, check=False)
    except FileNotFoundError:
        raise AuthFailure("auth_tool_unavailable") from None
    except subprocess.TimeoutExpired:
        raise AuthFailure("auth_verification_timeout") from None
    if result.returncode != 0 or len(result.stdout) > 1024 * 1024:
        raise AuthFailure("auth_verification_failed")
    return result.stdout


def read_auth(home, agent):
    filename = home / (".codex/auth.json" if agent == "codex" else ".grok/auth.json")
    if filename.is_symlink() or not filename.is_file() or filename.stat().st_size > 131072:
        raise AuthFailure("auth_credentials_missing")
    value = json.loads(filename.read_text())
    if agent == "codex":
        tokens = value.get("tokens", {})
        access, refresh, identity = tokens.get("access_token"), tokens.get("refresh_token"), tokens.get("account_id")
        try:
            claims = json.loads(base64.urlsafe_b64decode(access.split(".")[1] + "==="))
            expiry = int(claims["exp"]) * 1000
        except (KeyError, TypeError, ValueError, IndexError):
            raise AuthFailure("invalid_auth_expiry") from None
        label = claims.get("email")
    else:
        entries = [(key, item) for key, item in value.items() if key.startswith("https://auth.x.ai::") and isinstance(item, dict)]
        if len(entries) != 1:
            raise AuthFailure("auth_account_ambiguous")
        _, tokens = entries[0]
        access, refresh = tokens.get("key"), tokens.get("refresh_token")
        identity = tokens.get("user_id") or tokens.get("email")
        try:
            expiry = int(datetime.fromisoformat(tokens["expires_at"].replace("Z", "+00:00")).timestamp() * 1000)
        except (KeyError, TypeError, ValueError):
            raise AuthFailure("invalid_auth_expiry") from None
        label = tokens.get("email")
    if not all(isinstance(s, str) and 0 < len(s) <= 32768 for s in (access, refresh, identity)) or expiry <= time.time() * 1000 + 60000:
        raise AuthFailure("auth_credentials_invalid")
    return {"formatVersion": 1, "agent": agent, "method": "device-code", "accessToken": access,
            "refreshToken": refresh, "providerAccountId": identity, "accountLabel": label,
            "expiresAt": expiry, "nativeAuth": value}


def device_login(agent, home, env, timeout):
    # Codex requires an explicitly configured CODEX_HOME to exist before it
    # loads any command, including device login. Never borrow a user's profile.
    for name, leaf in (("CODEX_HOME", ".codex"), ("GROK_HOME", ".grok"),
                       ("CLAUDE_CONFIG_DIR", ".claude"), ("XDG_CONFIG_HOME", ".config")):
        profile = home / leaf
        if env.get(name) != str(profile) or profile.is_symlink():
            raise AuthFailure("auth_profile_unavailable")
        try:
            profile.mkdir(mode=0o700, exist_ok=True)
            if not profile.is_dir():
                raise AuthFailure("auth_profile_unavailable")
            profile.chmod(0o700)
        except OSError:
            raise AuthFailure("auth_profile_unavailable") from None
    selector = None
    try:
        command = [binary(agent)] + (["--no-auto-update"] if agent == "grok" else [])
        child = subprocess.Popen(command + ["login", "--device-auth"], cwd=home, env=env,
                                 stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    except FileNotFoundError:
        raise AuthFailure("auth_tool_unavailable") from None
    try:
        selector = selectors.DefaultSelector()
        selector.register(child.stdout, selectors.EVENT_READ)
        output, previous = "", None
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for key, _ in selector.select(timeout=0.25):
                chunk = os.read(key.fileobj.fileno(), 4096)
                if chunk:
                    output = (output + chunk.decode(errors="replace"))[-8192:]
                    event = parse_device_output(agent, output)
                    if event and event != previous:
                        emit(event)
                        previous = event
                else:
                    selector.unregister(key.fileobj)
            if child.poll() is not None:
                if child.returncode != 0:
                    raise AuthFailure("auth_login_failed")
                return read_auth(home, agent)
        raise AuthFailure("auth_login_expired")
    finally:
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=2)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        if selector is not None:
            selector.close()
        child.stdout.close()


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        raise AuthFailure("auth_verification_redirect")


def verify_key(agent, method, secret, home, env):
    if agent == "claude" and method == "setup-token":
        secret = re.sub(r"\s", "", secret)
        env["CLAUDE_CODE_OAUTH_TOKEN"] = secret
        raw = run([binary(agent), "-p", "Reply only OK.", "--max-turns", "1", "--tools", "", "--output-format", "json",
                   "--strict-mcp-config", "--mcp-config", '{"mcpServers":{}}', "--setting-sources", ""], home, env)
        result = json.loads(raw)
        if result.get("type") != "result" or result.get("subtype") != "success" or result.get("is_error") is not False or not result.get("result", "").strip():
            raise AuthFailure("auth_verification_failed")
    elif method == "api-key":
        endpoint = {"claude": "https://api.anthropic.com/v1/models", "codex": "https://api.openai.com/v1/models", "grok": "https://api.x.ai/v1/models"}[agent]
        headers = {"Accept": "application/json", "User-Agent": "zuse-credential-authority/1"}
        if agent == "claude":
            headers.update({"x-api-key": secret, "anthropic-version": "2023-06-01"})
        else:
            headers["Authorization"] = "Bearer " + secret
        try:
            with urllib.request.build_opener(NoRedirect).open(urllib.request.Request(endpoint, headers=headers), timeout=20) as response:
                if response.status != 200:
                    raise AuthFailure("auth_verification_failed")
        except Exception:
            raise AuthFailure("auth_verification_failed") from None
    else:
        raise AuthFailure("auth_method_unsupported")
    return {"formatVersion": 1, "agent": agent, "method": method, "secret": secret, "expiresAt": None, "accountLabel": None, "providerAccountId": None}


def refresh_native(agent, credential, home, env):
    filename = home / (".codex/auth.json" if agent == "codex" else ".grok/auth.json")
    filename.parent.mkdir(mode=0o700)
    filename.write_text(json.dumps(credential["nativeAuth"]))
    filename.chmod(0o600)
    command = [binary(agent), "app-server", "--listen", "stdio://"] if agent == "codex" else [binary(agent), "--no-auto-update", "--trust", "agent", "--no-leader", "stdio"]
    env["GROK_AUTH_EARLY_INVALIDATION_SECS"] = "315360000"
    child = subprocess.Popen(command, cwd=home, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    try:
        request = {"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "zuse-cloud-auth", "version": "1"}}} if agent == "codex" else {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": 1, "clientCapabilities": {}}}
        child.stdin.write(json.dumps(request) + "\n");child.stdin.flush()
        for _ in range(64):
            line = child.stdout.readline(131073)
            if not line or len(line) > 131072:
                raise AuthFailure("auth_refresh_failed")
            response = json.loads(line)
            if response.get("id") == 1:
                if "error" in response:
                    raise AuthFailure("auth_refresh_failed")
                if agent == "codex":
                    child.stdin.write(json.dumps({"method": "initialized"}) + "\n")
                    request = {"id": 2, "method": "account/read", "params": {"refreshToken": True}}
                else:
                    ids = [v.get("id") for v in response.get("result", {}).get("authMethods", [])]
                    method = "cached_token" if "cached_token" in ids else "grok.com" if "grok.com" in ids else None
                    if method is None:
                        raise AuthFailure("auth_refresh_failed")
                    request = {"jsonrpc": "2.0", "id": 2, "method": "authenticate", "params": {"methodId": method, "_meta": {"headless": True}}}
                child.stdin.write(json.dumps(request) + "\n");child.stdin.flush()
            elif response.get("id") == 2:
                if "error" in response:
                    raise AuthFailure("auth_refresh_failed")
                return read_auth(home, agent)
        raise AuthFailure("auth_refresh_failed")
    except (AuthFailure, ValueError):
        # The CLI may have persisted a rotation before a later account/read
        # failure. Preserve it rather than retrying the consumed refresh token.
        rotated = read_auth(home, agent)
        if rotated["refreshToken"] != credential.get("refreshToken"):
            return rotated
        raise
    finally:
        if child.poll() is None:
            child.terminate()
            try: child.wait(timeout=2)
            except subprocess.TimeoutExpired: child.kill();child.wait()
        child.stdin.close()
        child.stdout.close()


def main():
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(AuthFailure("auth_cancelled")))
    raw = sys.stdin.buffer.read(262145)
    if len(raw) > 262144:
        raise AuthFailure("invalid_auth_request")
    request = json.loads(raw)
    agent, method = request["agent"], request["method"]
    if agent not in ("claude", "codex", "grok") or method not in ("api-key", "setup-token", "device-code") or (method == "setup-token" and agent != "claude") or (method == "device-code" and agent == "claude"):
        raise AuthFailure("auth_method_unsupported")
    timeout = min(max(int(request.get("timeoutSeconds", 60)), 1), 900)
    signal.signal(signal.SIGALRM, lambda *_: (_ for _ in ()).throw(AuthFailure("auth_operation_timeout")))
    signal.alarm(timeout)
    with tempfile.TemporaryDirectory(prefix="zuse-authority-") as directory:
        home = Path(directory)
        home.chmod(0o700)
        env = isolated_environment(home)
        if request.get("action") == "refresh":
            result = refresh_native(agent, request["credential"], home, env)
        elif method == "device-code":
            result = device_login(agent, home, env, timeout)
        else:
            result = verify_key(agent, method, request["secret"], home, env)
    emit({"event": "verified", "credential": result})


if __name__ == "__main__":
    try:
        main()
    except AuthFailure as error:
        emit({"event": "error", "code": str(error)})
        sys.exit(1)
    except Exception:
        emit({"event": "error", "code": "auth_operation_failed"})
        sys.exit(1)
