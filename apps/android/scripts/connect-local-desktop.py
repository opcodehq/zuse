#!/usr/bin/env python3
"""Pair staging Android with the running Rust desktop and retain it for manual use.

Start scripts/run-staging-desktop.sh first. Browser assets must be built. No hosted
API or owner bearer is used; only the desktop's encrypted pairing invitation.
"""
import argparse
import json
import os
from pathlib import Path
import secrets
import subprocess
from urllib.parse import urlsplit

root = Path(__file__).resolve().parents[3]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--agent", action="store_true", help="Also send a harmless prompt through the desktop's installed Codex provider")
parser.add_argument("--files", action="store_true", help="Exercise the real desktop file browser")
parser.add_argument("--reuse-helper", action="store_true", help="Use an already-built pairing helper")
args = parser.parse_args()
native_deps = root / ".context/linux-deps/root/usr"
if native_deps.is_dir():
    previous_pkg = os.environ.get("PKG_CONFIG_PATH", "")
    previous_include = os.environ.get("C_INCLUDE_PATH", "")
    os.environ["PKG_CONFIG_PATH"] = str(native_deps / "lib/x86_64-linux-gnu/pkgconfig") + (":" + previous_pkg if previous_pkg else "")
    os.environ["C_INCLUDE_PATH"] = str(native_deps / "include") + ":" + str(native_deps / "include/x86_64-linux-gnu") + (":" + previous_include if previous_include else "")
app_id = "sh.zuse.android.staging"
sdk = Path(os.environ["ANDROID_HOME"])
adb = str(sdk / "platform-tools/adb")
devices = subprocess.check_output([adb, "devices"], text=True).splitlines()[1:]
serials = [line.split()[0] for line in devices if line.strip().endswith("\tdevice")]
if len(serials) != 1 or not serials[0].startswith("emulator-"):
    raise RuntimeError("Connect exactly one local emulator")
command = [adb, "-s", serials[0]]
if args.reuse_helper:
    metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=root))
    subprocess.run([str(Path(metadata["target_directory"]) / "debug/examples/android_pairing")], cwd=root, check=True)
else:
    subprocess.run(["cargo", "run", "-j", "2", "-p", "zuse", "--example", "android_pairing"], cwd=root, check=True)
invitation = root / ".context/android-tools/staging-desktop-invitation.json"
config = json.loads(invitation.read_text())
origin = urlsplit(config["origin"])
if origin.hostname != "127.0.0.1" or origin.port is None:
    raise RuntimeError("Expected the desktop's local loopback listener")
subprocess.run([str(root / "apps/android/gradlew"), "-p", str(root / "apps/android"), "-PdeploymentProfile=staging", ":app:assembleDebug", ":app:assembleDebugAndroidTest", "--no-daemon"], cwd=root, check=True)
for apk in ["app/build/outputs/apk/debug/app-debug.apk", "app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk"]:
    subprocess.run([*command, "install", "-r", str(root / "apps/android" / apk)], check=True)
subprocess.run([*command, "reverse", f"tcp:{origin.port}", f"tcp:{origin.port}"], check=True)
# Reserve the same localhost address for a separately launched local Rust API.
subprocess.run([*command, "reverse", "tcp:28700", "tcp:28700"], check=True)
filename = "desktop-test-" + secrets.token_hex(8) + ".json"
try:
    subprocess.run([*command, "shell", "run-as", app_id, "sh", "-c", f"'mkdir -p files && cat > files/{filename}'"], input=json.dumps(config), text=True, check=True)
    result = subprocess.run([*command, "shell", "am", "instrument", "-w", "-e", "desktopConfig", filename, "-e", "realAgent", str(args.agent).lower(), "-e", "browseFiles", str(args.files).lower(), "-e", "class", "sh.zuse.android.DesktopConnectionTests", app_id + ".test/androidx.test.runner.AndroidJUnitRunner"], text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=True)
    print(result.stdout)
    if "OK (1 test)" not in result.stdout or "FAILURES" in result.stdout:
        raise RuntimeError("Desktop-to-Android acceptance failed")
finally:
    subprocess.run([*command, "shell", "run-as", app_id, "rm", "-f", f"files/{filename}"], check=True)
# Reuse the helper we just built. A second `cargo run` can block behind unrelated
# builds and unnecessarily invalidate native build-script environment caches.
metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=root))
helper = Path(metadata["target_directory"]) / "debug/examples/android_pairing"
subprocess.run([str(helper), "--verify"], cwd=root, check=True)
subprocess.run([*command, "shell", "am", "start", "-n", app_id + "/sh.zuse.android.MainActivity"], check=True)
print("Staging Android paired to the local Rust desktop. Emulator route retained for manual use.")
