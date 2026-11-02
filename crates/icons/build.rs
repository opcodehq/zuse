use std::{env, fs, path::PathBuf, process::Command};
fn main() {
    println!("cargo:rerun-if-env-changed=HUGEICONS_SOLID_VERSION");
    println!("cargo:rerun-if-changed=mapping.tsv");
    println!("cargo:rerun-if-changed=assets/free");
    println!("cargo:rerun-if-changed=../../scripts/prepare-icons.mjs");
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    // Check key presence each invocation without recording the secret value in
    // Cargo's RerunIfEnvChanged fingerprint. The missing sentinel forces this
    // small selector to rerun; prepared SVGs are reused by the generator.
    println!(
        "cargo:rerun-if-changed={}",
        out.join("style-selection-sentinel").display()
    );
    let solid = env::var("HUGEICONS_LICENSE_KEY").is_ok_and(|key| !key.trim().is_empty());
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
