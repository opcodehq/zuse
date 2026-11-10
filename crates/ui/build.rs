use std::{env, path::PathBuf, process::Command};

fn main() {
    // Legacy Objective-C macros expand this feature check in the calling crate.
    println!("cargo:rustc-check-cfg=cfg(feature, values(\"cargo-clippy\"))");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=src/dictation/permission.m");
        println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");
        let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
        let object = out.join("voice-permission.o");
        assert!(
            Command::new("clang")
                // Build scripts run on the host. clang otherwise emits a host
                // object even when Cargo is cross-compiling the Rust crate.
                .args(["-target", &env::var("TARGET").expect("Cargo sets TARGET")])
                .args(["-fobjc-arc", "-c", "src/dictation/permission.m", "-o"])
                .arg(&object)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("ar")
                .arg("crus")
                .arg(out.join("libvoice-permission.a"))
                .arg(object)
                .status()
                .unwrap()
                .success()
        );
        println!("cargo:rustc-link-search=native={}", out.display());
        println!("cargo:rustc-link-lib=static=voice-permission");
        println!("cargo:rustc-link-lib=framework=AVFoundation");
    }

    println!("cargo:rerun-if-changed=src/browser/linux/helper.c");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("zuse-webkit");
    let flags = Command::new("pkg-config")
        .args(["--cflags", "--libs", "webkit2gtk-4.1", "json-glib-1.0"])
        .output()
        .expect("pkg-config is required to build the Linux browser helper");
    assert!(
        flags.status.success(),
        "The Linux browser requires WebKitGTK 4.1 and JSON-GLib development files \
         discoverable by pkg-config (webkit2gtk-4.1 and json-glib-1.0). \
         See internal-docs/development.md for distribution-specific installation commands.\n{}",
        String::from_utf8_lossy(&flags.stderr)
    );
    let status = Command::new(env::var("CC").unwrap_or_else(|_| "cc".into()))
        .args([
            "-std=c11",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Wno-unused-parameter",
            "src/browser/linux/helper.c",
            "-o",
        ])
        .arg(&output)
        .args(String::from_utf8(flags.stdout).unwrap().split_whitespace())
        .status()
        .expect("C compiler is required to build the Linux browser helper");
    assert!(status.success(), "Linux browser helper compilation failed");
}
