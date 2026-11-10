#!/usr/bin/env python3
"""Package the shared encrypted channel for iPhone, Simulator and Mac (requires Xcode)."""
import json
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile


def main():
    if platform.system() != "Darwin":
        raise SystemExit("Apple pairing packaging requires macOS and full Xcode.")
    root = Path(__file__).resolve().parents[3]
    targets = ["aarch64-apple-ios", "aarch64-apple-ios-sim", "x86_64-apple-ios",
               "aarch64-apple-darwin", "x86_64-apple-darwin"]
    subprocess.run(["xcodebuild", "-version"], check=True)
    subprocess.run(["rustup", "target", "add", *targets], check=True)
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=root))
    build = Path(metadata["target_directory"])
    for target in targets:
        subprocess.run(["cargo", "rustc", "-j", "2", "--release", "--target", target,
                        "-p", "zuse-mobile-bridge", "--crate-type", "staticlib"], cwd=root, check=True)
    output = root / "apps/ios/Native/ZusePairing.xcframework"
    output.parent.mkdir(parents=True, exist_ok=True)
    # Create a complete candidate before replacing an existing package.
    with tempfile.TemporaryDirectory(prefix="pairing-", dir=output.parent) as temporary:
        staging = Path(temporary)
        headers = staging / "include"
        shutil.copytree(root / "crates/mobile-bridge/include", headers)
        (headers / "module.modulemap").write_text(
            'module ZusePairing {\n  header "zuse_mobile.h"\n  export *\n}\n')
        libraries = [build / targets[0] / "release/libzuse_mobile_bridge.a"]
        for name, variants in [("simulator", targets[1:3]), ("macos", targets[3:])]:
            library = staging / name / "libzuse_mobile_bridge.a"
            library.parent.mkdir()
            subprocess.run(["xcrun", "lipo", "-create", *[
                str(build / target / "release/libzuse_mobile_bridge.a") for target in variants
            ], "-output", str(library)], check=True)
            libraries.append(library)
        command = ["xcodebuild", "-create-xcframework"]
        for library in libraries:
            command.extend(["-library", str(library), "-headers", str(headers)])
        candidate = staging / output.name
        subprocess.run([*command, "-output", str(candidate)], check=True)
        if output.exists():
            shutil.rmtree(output)
        shutil.move(candidate, output)
    print(f"Built {output}")


if __name__ == "__main__":
    main()
