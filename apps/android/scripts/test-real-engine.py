#!/usr/bin/env python3
"""Run native JVM acceptance against an isolated instance of the actual Rust engine."""
import argparse
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import time

root = Path(__file__).resolve().parents[3]
parser = argparse.ArgumentParser()
parser.add_argument("--device", action="store_true", help="Also run the packaged JNI client on one connected Android emulator")
parser.add_argument("--staging", action="store_true", help="Build and test the staging app identity and endpoints")
args = parser.parse_args()
profile_args = ["-PdeploymentProfile=staging"] if args.staging else []
app_id = "sh.zuse.android.staging" if args.staging else "sh.zuse.android"
subprocess.run(["cargo", "build", "-j", "2", "-p", "zuse-cli"], cwd=root, check=True)
metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=root))
binary = Path(metadata["target_directory"]) / "debug" / "zuse-server"
subprocess.run(["cargo", "build", "-j", "2", "--release", "-p", "zuse-mobile-bridge"], cwd=root, check=True)
java = Path(os.environ["JAVA_HOME"])

with tempfile.TemporaryDirectory(prefix="zuse-native-acceptance-") as data:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    native = Path(data) / "jni"
    native.mkdir()
    subprocess.run(["cc", "-shared", "-fPIC", "-O2", "-I" + str(java / "include"), "-I" + str(java / "include/linux"), "-I" + str(root / "crates/mobile-bridge/include"), str(root / "apps/android/native/pairing-jni.c"), str(Path(metadata["target_directory"]) / "release/libzuse_mobile_bridge.a"), "-ldl", "-lpthread", "-lm", "-o", str(native / "libzuse_pairing.so")], check=True)
    env = os.environ.copy()
    env["ZUSE_NATIVE_JNI_DIR"] = str(native)
    env["ZUSE_BROWSER_TOKEN"] = secrets.token_hex(32)
    env["ZUSE_NATIVE_RPC_TOKEN"] = env["ZUSE_BROWSER_TOKEN"]
    env["ZUSE_NATIVE_RPC_ADDRESS"] = f"ws://127.0.0.1:{port}/api/rpc"
    env["ZUSE_MOCK_QUESTION"] = "1"
    env["ZUSE_MOCK_DELAY_MS"] = "25"
    with open(Path(data) / "server.log", "w") as log:
        server = subprocess.Popen([str(binary), "--listen", f"127.0.0.1:{port}", "--ipc-port", "0", "--data-dir", data], cwd=root, env=env, stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 60
            while True:
                if server.poll() is not None:
                    raise RuntimeError("Isolated engine exited before readiness")
                try:
                    with socket.create_connection(("127.0.0.1", port), timeout=0.5):
                        break
                except OSError:
                    if time.monotonic() >= deadline:
                        raise TimeoutError("Isolated engine did not start in 60 seconds")
                    time.sleep(0.2)
            subprocess.run([str(root / "apps/android/gradlew"), "-p", str(root / "apps/android"), ":core:test", "--tests", "sh.zuse.core.RealEngineTests", "--rerun-tasks", "--no-daemon", *profile_args], cwd=root, env=env, check=True)
            if args.device:
                sdk = Path(os.environ["ANDROID_HOME"])
                adb = str(sdk / "platform-tools/adb")
                # Restrict this credential-bearing acceptance run to a local emulator.
                devices = subprocess.check_output([adb, "devices"], text=True).splitlines()[1:]
                serials = [line.split()[0] for line in devices if line.strip().endswith("\tdevice")]
                if len(serials) != 1 or not serials[0].startswith("emulator-"):
                    raise RuntimeError("Device acceptance requires exactly one connected emulator")
                adb_command = [adb, "-s", serials[0]]
                subprocess.run([str(root / "apps/android/gradlew"), "-p", str(root / "apps/android"), ":app:assembleDebug", ":app:assembleDebugAndroidTest", "--no-daemon", *profile_args], cwd=root, env=env, check=True)
                for apk in ["app/build/outputs/apk/debug/app-debug.apk", "app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk"]:
                    subprocess.run([*adb_command, "install", "-r", str(root / "apps/android" / apk)], check=True)
                filename = "host-test-" + secrets.token_hex(8) + ".json"
                subprocess.run([*adb_command, "reverse", f"tcp:{port}", f"tcp:{port}"], check=True)
                try:
                    config = json.dumps({"address": env["ZUSE_NATIVE_RPC_ADDRESS"], "token": env["ZUSE_NATIVE_RPC_TOKEN"]})
                    subprocess.run([*adb_command, "shell", "run-as", app_id, "sh", "-c", f"'mkdir -p files && cat > files/{filename}'"], input=config, text=True, check=True)
                    result = subprocess.run([*adb_command, "shell", "am", "instrument", "-w", "-e", "hostConfig", filename, "-e", "class", "sh.zuse.android.RealHostTests,sh.zuse.android.RealHostUiTests", app_id + ".test/androidx.test.runner.AndroidJUnitRunner"], text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=True)
                    print(result.stdout)
                    if "OK (2 tests)" not in result.stdout or "FAILURES" in result.stdout:
                        raise RuntimeError("Packaged real-host Android acceptance failed")
                finally:
                    subprocess.run([*adb_command, "shell", "run-as", app_id, "rm", "-f", f"files/{filename}"], check=True)
                    subprocess.run([*adb_command, "reverse", "--remove", f"tcp:{port}"], check=True)
        finally:
            server.terminate()
            try:
                server.wait(timeout=15)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
