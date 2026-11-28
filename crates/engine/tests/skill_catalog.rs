#![cfg(unix)]

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};
use zuse_engine::{EngineCore, HarnessRegistry};
use zuse_protocol::{HarnessId, skill_catalog};
use zuse_rpc::methods::{LIST_SKILLS, MUTATE, PROJECT_SKILL_CATALOG};

fn git(cwd: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[ignore = "downloads a pinned public package from GitHub"]
async fn installs_portable_skills_in_the_selected_checkout_and_preserves_existing_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let worktree = temp.path().join("worktree");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(
        &root,
        &[
            "-c",
            "user.name=Skill Test",
            "-c",
            "user.email=skills@example.test",
            "commit",
            "--allow-empty",
            "-qm",
            "Initial commit",
        ],
    );
    git(
        &root,
        &["worktree", "add", "--detach", worktree.to_str().unwrap()],
    );

    let core = EngineCore::assemble(
        &temp.path().join("data"),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Zuse,
        None,
    )
    .unwrap();
    core.workspace
        .create_space(
            "project",
            core.doc_host.device_id(),
            root.to_str().unwrap(),
            None,
            true,
        )
        .unwrap();
    let client = zuse_rpc::memory_client(core.rpc_service());
    client
        .call(
            MUTATE,
            json!({
                "op": "createChat",
                "chatId": "thread",
                "spaceId": "project",
                "deviceId": core.device_id,
                "cwd": worktree,
            }),
        )
        .await
        .unwrap();
    let call = |action: &'static str, checkout: Option<Value>| {
        let client = &client;
        async move {
            client
                .call(
                    PROJECT_SKILL_CATALOG,
                    json!({
                        "chatId": "thread",
                        "action": action,
                        "skill": "cloud-logging-query-generation",
                        "expectedCheckoutId": checkout
                    }),
                )
                .await
        }
    };

    let initial = call("list", None).await.unwrap();
    assert_eq!(initial["installations"], json!({}));
    assert!(
        call("install", Some(json!("different-checkout")))
            .await
            .is_err()
    );
    let checkout = Some(initial["checkoutId"].clone());
    let installed = call("install", checkout.clone()).await.unwrap();
    assert_eq!(
        installed["installations"]["cloud-logging-query-generation"],
        "installed"
    );
    let skill = skill_catalog::entry("cloud-logging-query-generation").unwrap();
    let installed_dir = worktree.join(skill.install_path());
    let installed_file = installed_dir.join("SKILL.md");
    assert!(!root.join(skill.install_path()).exists());
    assert!(skill.package.files.len() > 1);
    for file in &skill.package.files {
        let bytes = std::fs::read(installed_dir.join(&file.path)).unwrap();
        use sha2::{Digest, Sha256};
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), file.sha256);
    }
    // /skills uses the same provider list, scoped to this checkout.
    let manual_skill = worktree.join(".agents/skills/manual-workflow");
    std::fs::create_dir(&manual_skill).unwrap();
    std::fs::write(
        manual_skill.join("SKILL.md"),
        "---\nname: manual-workflow\ndescription: Added outside the catalog.\n---\n",
    )
    .unwrap();
    let skill_list = client
        .call(
            LIST_SKILLS,
            json!({"chatId":"thread", "harness":"zuse", "projectOnly":true}),
        )
        .await
        .unwrap();
    assert!(
        skill_list
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["name"] == skill.package.name)
    );

    assert!(
        skill_list
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["name"] == "manual-workflow")
    );
    assert!(skill_list.as_array().unwrap().iter().all(|row| {
        std::path::Path::new(row["path"].as_str().unwrap())
            .starts_with(worktree.canonicalize().unwrap())
    }));
    call("install", checkout.clone()).await.unwrap();
    let reference = installed_file.with_file_name("notes.md");
    std::fs::write(&reference, "My notes").unwrap();
    assert!(call("remove", checkout.clone()).await.is_err());
    assert!(installed_file.exists());
    assert!(reference.exists());
    std::fs::remove_file(reference).unwrap();
    assert_eq!(
        call("remove", checkout.clone()).await.unwrap()["installations"],
        json!({})
    );
    assert!(!installed_file.exists());

    let external_skill = temp.path().join("existing-skill");
    std::fs::create_dir(&external_skill).unwrap();
    std::os::unix::fs::symlink(&external_skill, installed_file.parent().unwrap()).unwrap();
    let listed = call("list", None).await.unwrap();
    assert_eq!(
        listed["installations"]["cloud-logging-query-generation"],
        "existing"
    );
    assert!(call("install", checkout).await.is_err());
    assert!(!external_skill.join("SKILL.md").exists());
    core.shutdown().await;
}
