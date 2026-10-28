#![expect(clippy::unwrap_used, reason = "test failures report invalid fixtures")]

use super::*;

fn workspace() -> (tempfile::TempDir, AuthoringTools) {
    let root = tempfile::tempdir().unwrap();
    aether_skills::creator::write_resources(&root.path().join("creator")).unwrap();
    fs::create_dir(root.path().join("skill")).unwrap();
    fs::create_dir(root.path().join("evaluations")).unwrap();
    let tools = AuthoringTools::new(root.path().to_path_buf());
    (root, tools)
}

fn call(name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: "test".into(),
        name: name.into(),
        args,
    }
}

#[tokio::test]
async fn authoring_files_preview_and_scripts() {
    let (root, tools) = workspace();
    let cancel = CancellationToken::new();
    tools.write("skill/SKILL.md", "---\nname: sample-skill\ndescription: A sample skill for authoring checks\n---\nUse the reference.\n").unwrap();
    tools
        .write("skill/references/guide.md", "reference")
        .unwrap();
    tools.write("skill/LICENSE", "MIT").unwrap();
    tools
        .write("evaluations/results.json", "{\"score\":1}")
        .unwrap();
    tools
        .write("skill/references/guide.md", "updated reference")
        .unwrap();
    assert_eq!(
        tools.read("skill/references/guide.md", 0).unwrap().text,
        "updated reference"
    );
    let preview = tools.validate().unwrap().payload.unwrap();
    assert_eq!(preview["preview"]["name"], "sample-skill");
    assert!(
        preview["preview"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .all(|file| !file["path"].as_str().unwrap().contains("results"))
    );
    assert!(
        tools
            .list(None)
            .unwrap()
            .text
            .contains("creator/agents/grader.md")
    );
    for raw in [
        "../escape",
        "skill/../escape",
        "/etc/passwd",
        "C:/escape",
        "skill\\escape",
        "skill/NUL",
        "skill/assets/file.",
        "skill/assets/a:b",
        "other/file",
    ] {
        assert!(tools.path(raw, true).is_err(), "accepted {raw}");
    }
    assert!(tools.write("creator/SKILL.md", "replace").is_err());
    assert!(tools.write("skill/evaluations/test.json", "{}").is_err());
    assert!(
        tools
            .write("skill/references/Guide.md", "case collision")
            .is_err()
    );
    assert!(
        tools
            .write("skill/SKILL.md", &"x".repeat(MAX_SKILL + 1))
            .is_err()
    );
    tools
        .write("evaluations/big.txt", &"中".repeat(20_000))
        .unwrap();
    let read = tools.read("evaluations/big.txt", 0).unwrap();
    assert!(read.text.len() <= MAX_OUTPUT);
    assert!(read.payload.unwrap()["truncated"].as_bool().unwrap());

    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret.txt"), "must not read").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path(), root.path().join("evaluations/link")).unwrap();
        assert!(tools.read("evaluations/link/secret.txt", 0).is_err());
        assert!(tools.write("evaluations/link/write.txt", "no").is_err());
        assert!(tools.list(Some("evaluations")).is_err());
        fs::remove_file(root.path().join("evaluations/link")).unwrap();
    }
    #[cfg(windows)]
    if std::os::windows::fs::symlink_dir(outside.path(), root.path().join("evaluations/link"))
        .is_ok()
    {
        assert!(tools.read("evaluations/link/secret.txt", 0).is_err());
        assert!(tools.write("evaluations/link/write.txt", "no").is_err());
        assert!(tools.list(Some("evaluations")).is_err());
        fs::remove_dir(root.path().join("evaluations/link")).unwrap();
    }

    for (path, body) in [
        (
            "evaluations/output.js",
            "console.log('node stdout'); console.error('node stderr'); console.log('sk-123456789012345678901234567890'); process.exitCode=3;",
        ),
        (
            "evaluations/output.py",
            "import sys\nprint('python stdout')\nprint('python stderr', file=sys.stderr)\nsys.exit(3)\n",
        ),
    ] {
        tools.write(path, body).unwrap();
        let result = tools
            .execute(
                &call("authoring_run", json!({"path":path,"timeoutSeconds":5})),
                &cancel,
            )
            .await;
        match result {
            Ok(output) => {
                let payload = output.payload.unwrap();
                assert_eq!(payload["exitCode"], 3);
                assert!(payload["stdout"].as_str().unwrap().contains("stdout"));
                assert!(payload["stderr"].as_str().unwrap().contains("stderr"));
                assert!(!output.text.contains("sk-123456789012345678901234567890"));
            }
            Err(error) => assert_eq!(error.code, "interpreter_not_found", "{error:?}"),
        }
    }
    let specs = tools.specs();
    assert_eq!(
        specs
            .iter()
            .find(|entry| entry.spec.name == "authoring_run")
            .unwrap()
            .spec
            .approval,
        Approval::Ask
    );
}

#[tokio::test]
async fn cancel_ends_script_descendants() {
    let (root, tools) = workspace();
    tools.write("evaluations/tree.js", r#"const {spawn}=require('child_process');
const fs=require('fs');
spawn(process.execPath,['-e',"const fs=require('fs');setInterval(()=>fs.appendFileSync('evaluations/heartbeat','x'),20)"],{stdio:'inherit'});
fs.writeFileSync('evaluations/ready','ready');
setInterval(()=>{},1000);
"#).unwrap();
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        tools
            .execute(
                &call(
                    "authoring_run",
                    json!({"path":"evaluations/tree.js","timeoutSeconds":10}),
                ),
                &task_cancel,
            )
            .await
    });
    let started = tokio::time::Instant::now();
    while !root.path().join("evaluations/ready").exists() && !task.is_finished() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "script did not start"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(7), task)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_err(), "cancelled process unexpectedly completed");
    let Err(error) = result else { return };
    if error.code == "interpreter_not_found" {
        return;
    }
    assert_eq!(error.code, "cancelled");
    assert!(error.unknown);
    let bytes = fs::read(root.path().join("evaluations/heartbeat")).unwrap_or_default();
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(
        fs::read(root.path().join("evaluations/heartbeat")).unwrap_or_default(),
        bytes,
        "descendant survived cancellation"
    );
}
