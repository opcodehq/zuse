use std::{env, fs, path::PathBuf, process::Command};
fn main() {
    println!("cargo:rerun-if-env-changed=HUGEICONS_SOLID_VERSION");
    println!("cargo:rerun-if-env-changed=ZUSE_ICON_STYLE");
    println!("cargo:rerun-if-changed=mapping.tsv");
    println!("cargo:rerun-if-changed=assets/free");
    println!("cargo:rerun-if-changed=../../scripts/prepare-icons.mjs");
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    // The dev launcher supplies a public selector so unchanged builds stay
    // fresh without recording the license key in Cargo's fingerprints.
    // Direct Cargo invocations retain automatic credential-presence checks.
    let solid = match env::var("ZUSE_ICON_STYLE").as_deref() {
        Ok("free") => false,
        Ok("solid") => true,
        Ok(_) => panic!("ZUSE_ICON_STYLE must be free or solid"),
        Err(_) => {
            println!(
                "cargo:rerun-if-changed={}",
                out.join("style-selection-sentinel").display()
            );
            env::var("HUGEICONS_LICENSE_KEY").is_ok_and(|key| !key.trim().is_empty())
        }
    };
    if solid {
        assert!(
            env::var("HUGEICONS_LICENSE_KEY").is_ok_and(|key| !key.trim().is_empty()),
            "Solid icons require HUGEICONS_LICENSE_KEY"
        );
    }
    let assets = if solid {
        let assets = out.join("solid");
        let status = Command::new("node")
            .arg(root.join("../../scripts/prepare-icons.mjs"))
            .arg("--reuse")
            .arg("--output")
            .arg(&assets)
            .status()
            .expect("Node.js 22 is required for licensed icon builds");
        assert!(
            status.success(),
            "Could not prepare licensed solid icons; check HUGEICONS_LICENSE_KEY"
        );
        assets
    } else {
        root.join("assets/free")
    };
    let map = fs::read_to_string(root.join("mapping.tsv")).unwrap();
    let mut generated = format!(
        "pub const STYLE: &str = {:?};\npub fn load(path: &str) -> Option<&'static [u8]> {{ match path {{\n",
        if solid {
            "solid-rounded"
        } else {
            "free-rounded"
        }
    );
    for line in map.lines().filter(|line| !line.is_empty()) {
        let (name, _) = line.split_once('\t').expect("icon mapping entry");
        let path = assets.join(format!("{name}.svg"));
        assert!(path.is_file(), "Missing prepared icon: {name}");
        generated.push_str(&format!(
            "{:?} => Some(include_bytes!({:?}).as_slice()),\n",
            format!("icons/{name}.svg"),
            path
        ));
    }
    generated.push_str("_ => None, } }\npub const NAMES: &[&str] = &[\n");
    for line in map.lines().filter(|line| !line.is_empty()) {
        let (name, _) = line.split_once('\t').unwrap();
        generated.push_str(&format!("{:?},\n", format!("icons/{name}.svg")));
    }
    generated.push_str("];\n");
    let destination = out.join("icons.rs");
    if fs::read_to_string(&destination).ok().as_deref() != Some(&generated) {
        fs::write(destination, generated).unwrap();
    }
}
