#!/usr/bin/env python3
"""Check real connection routes in the packaged app on one local emulator.

Starts an isolated engine with LAN access and a real relay, puts a killable
proxy in front of the LAN port, and runs RemoteRoutesTests on the emulator.
Requires JAVA_HOME, ANDROID_HOME, ANDROID_NDK_HOME and a LAN or tailnet address.
"""
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import threading
import time

root = Path(__file__).resolve().parents[3]
sdk = Path(os.environ["ANDROID_HOME"])
adb = str(sdk / "platform-tools/adb")
target = Path(json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=root))["target_directory"]) / "debug"


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def wait_for(port: int, process: subprocess.Popen, name: str) -> None:
    deadline = time.monotonic() + 60
    while True:
        if process.poll() is not None:
            raise RuntimeError(f"{name} exited before readiness")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.5):
                return
        except OSError:
            if time.monotonic() >= deadline:
                raise TimeoutError(f"{name} did not start")
            time.sleep(0.2)


class KillableProxy:
    """Forwards to the LAN port until the control port receives its secret."""

    def __init__(self, destination: tuple[str, int]):
        self.destination = destination
        self.secret = secrets.token_hex(16)
        self.sockets: list[socket.socket] = []
        self.listener = socket.create_server(("0.0.0.0", 0))
        self.control = socket.create_server(("0.0.0.0", 0))
        self.port = self.listener.getsockname()[1]
        self.control_port = self.control.getsockname()[1]
        threading.Thread(target=self.accept, daemon=True).start()
        threading.Thread(target=self.kill_on_control, daemon=True).start()

    def accept(self) -> None:
        while True:
            try:
                client, _ = self.listener.accept()
            except OSError:
                return
            server = socket.create_connection(self.destination)
            for pipe in [client, server]:
                pipe.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            self.sockets += [client, server]
            threading.Thread(target=self.pump, args=(client, server), daemon=True).start()
            threading.Thread(target=self.pump, args=(server, client), daemon=True).start()

    @staticmethod
    def pump(source: socket.socket, sink: socket.socket) -> None:
        try:
            while data := source.recv(65536):
                sink.sendall(data)
        except OSError:
            pass

    def kill_on_control(self) -> None:
        # Local tools probe new listeners, so only the secret counts.
        while True:
            caller, _ = self.control.accept()
            caller.settimeout(5)
            try:
                received = caller.recv(64).decode(errors="ignore")
            except OSError:
                received = ""
            caller.close()
            if received == self.secret:
                break
        self.listener.close()
        for each in self.sockets:
            try:
                each.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            each.close()


devices = [line.split()[0] for line in subprocess.check_output([adb, "devices"], text=True).splitlines()[1:] if line.strip().endswith("\tdevice")]
if len(devices) != 1 or not devices[0].startswith("emulator-"):
    raise RuntimeError("Route verification requires exactly one connected emulator")
device = [adb, "-s", devices[0]]
subprocess.run(["cargo", "build", "-p", "zuse-cli", "-p", "zuse-relay"], cwd=root, check=True)
gradle = [str(root / "apps/android/gradlew"), "-p", str(root / "apps/android"), "--offline", "-q"]
subprocess.run([*gradle, ":app:assembleDebug", ":app:assembleDebugAndroidTest"], check=True)
for apk in ["app/build/outputs/apk/debug/app-debug.apk", "app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk"]:
    subprocess.run([*device, "install", "-r", str(root / "apps/android" / apk)], check=True, stdout=subprocess.DEVNULL)

with tempfile.TemporaryDirectory(prefix="zuse-route-check-") as data:
    root_dir = Path(data)
    port, lan_port, relay_port = free_port(), free_port(), free_port()
    (root_dir / "assets").mkdir()
    (root_dir / "assets/index.html").write_text("<!doctype html>")
    token_file = root_dir / "relay-token"
    token_file.write_text(secrets.token_hex(32))
    token_file.chmod(0o600)
    env = {**os.environ, "ZUSE_BROWSER_TOKEN": secrets.token_hex(32)}
    relay_origin = f"http://127.0.0.1:{relay_port}"
    logs = open(root_dir / "log", "w")
    relay = subprocess.Popen([str(target / "zuse-relay"), "--listen", f"127.0.0.1:{relay_port}", "--assets", str(root_dir / "assets"), "--host-token-file", str(token_file)], stdout=logs, stderr=logs)
    server = subprocess.Popen(
        [str(target / "zuse-server"), "--listen", f"127.0.0.1:{port}", "--ipc-port", "0", "--data-dir", str(root_dir / "engine"), "--assets", str(root_dir / "assets"), "--lan", "--lan-port", str(lan_port), "--relay-origin", relay_origin, "--relay-host-token-file", str(token_file)],
        cwd=root, env=env, stdout=subprocess.PIPE, stderr=logs, text=True)
    filename = "host-test-" + secrets.token_hex(8) + ".json"
    try:
        wait_for(relay_port, relay, "relay")
        wait_for(lan_port, server, "engine LAN listener")
        wait_for(port, server, "engine")
        routes: dict[str, str] = {}
        def read_routes() -> None:
            for line in server.stdout:
                print("host:", line.split("#token=")[0].rstrip())
                for label in ("Local network", "Tailscale"):
                    if line.startswith(label + ": "):
                        routes[label] = line.split(": ", 1)[1].strip()
        threading.Thread(target=read_routes, daemon=True).start()
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline and len(routes) < 2:
            time.sleep(0.2)
        print("advertised:", routes)
        if "Local network" not in routes or "Tailscale" not in routes:
            raise RuntimeError("Route verification needs both a LAN and a tailnet address on this host")

        # The LAN listener must offer nothing but encrypted paired RPC.
        for path in ["/", "/api/rpc", "/api/session/check", "/health"]:
            with socket.create_connection(("127.0.0.1", lan_port), timeout=3) as probe:
                probe.sendall(f"GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n".encode())
                status = probe.recv(64).decode().split("\r\n")[0]
            print(f"lan {path}: {status}")
            if " 404 " not in status:
                raise RuntimeError(f"LAN listener served {path}: {status}")

        proxy = KillableProxy(("127.0.0.1", lan_port))
        dead = free_port()
        for forwarded in [port, relay_port]:
            subprocess.run([*device, "reverse", f"tcp:{forwarded}", f"tcp:{forwarded}"], check=True, stdout=subprocess.DEVNULL)
        config = {
            "address": f"ws://127.0.0.1:{port}/api/rpc", "token": env["ZUSE_BROWSER_TOKEN"],
            "lan": routes["Local network"], "tailnet": routes["Tailscale"], "relay": relay_origin,
            "proxy": f"http://10.0.2.2:{proxy.port}", "proxyControlHost": "10.0.2.2", "proxyControlPort": str(proxy.control_port), "proxySecret": proxy.secret,
            "deadOrigin": f"http://10.0.2.2:{dead}",
        }
        app = "sh.zuse.android"
        subprocess.run([*device, "shell", "run-as", app, "sh", "-c", f"'mkdir -p files && cat > files/{filename}'"], input=json.dumps(config), text=True, check=True)
        subprocess.run([*device, "logcat", "-c"], check=True)
        result = subprocess.run([*device, "shell", "am", "instrument", "-w", "-e", "hostConfig", filename, "-e", "class", os.environ.get("ZUSE_ROUTE_TEST", "sh.zuse.android.RemoteRoutesTests"), f"{app}.test/androidx.test.runner.AndroidJUnitRunner"], capture_output=True, text=True, timeout=420)
        print(result.stdout)
        measured = subprocess.run([*device, "logcat", "-d", "-s", "ZuseRoutes:I"], capture_output=True, text=True).stdout
        print("\n".join(line for line in measured.splitlines() if "ZuseRoutes" in line))
        if "OK (" not in result.stdout or "FAILURES" in result.stdout:
            raise RuntimeError("Route verification failed")
    finally:
        subprocess.run([*device, "shell", "cmd", "connectivity", "airplane-mode", "disable"])
        subprocess.run([*device, "shell", "run-as", app if "app" in dir() else "sh.zuse.android", "rm", "-f", f"files/{filename}"])
        subprocess.run([*device, "reverse", "--remove-all"])
        server.terminate()
        relay.terminate()
        for process in [server, relay]:
            try:
                process.wait(10)
            except subprocess.TimeoutExpired:
                process.kill()
        logs.close()
