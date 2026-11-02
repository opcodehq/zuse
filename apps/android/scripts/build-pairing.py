#!/usr/bin/env python3
"""Build the shared Rust channel and JNI shim for Android API 28+."""
import argparse
import json
import os
import platform
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("--ndk", required=True)
parser.add_argument("--output", required=True)
args = parser.parse_args()
root = Path(__file__).resolve().parents[3]
ndk = Path(args.ndk).resolve()
host = "darwin-x86_64" if platform.system() == "Darwin" else "linux-x86_64"
bin_dir = ndk / "toolchains/llvm/prebuilt" / host / "bin"
metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=root))
for target, abi, compiler in [("aarch64-linux-android", "arm64-v8a", "aarch64-linux-android28-clang"), ("x86_64-linux-android", "x86_64", "x86_64-linux-android28-clang")]:
    env = os.environ.copy()
    env["CARGO_TARGET_" + target.upper().replace("-", "_") + "_LINKER"] = str(bin_dir / compiler)
    subprocess.run(["cargo", "build", "-j", "2", "--release", "--target", target, "-p", "zuse-mobile-bridge"], cwd=root, env=env, check=True)
    output = Path(args.output).resolve() / abi
    output.mkdir(parents=True, exist_ok=True)
    subprocess.run([str(bin_dir / compiler), "-shared", "-fPIC", "-O2", "-Wl,-z,max-page-size=16384", "-I" + str(root / "crates/mobile-bridge/include"), str(root / "apps/android/native/pairing-jni.c"), str(Path(metadata["target_directory"]) / target / "release/libzuse_mobile_bridge.a"), "-ldl", "-lm", "-o", str(output / "libzuse_pairing.so")], check=True)
