//! Filesystem discovery and packaging boundary tests.
#![expect(
    clippy::unwrap_used,
    reason = "test failures report invalid fixtures",
)]

use super::*;

#[test]
fn selected_root_discovers_skills_and_packages_only_allowed_files() {
    let root = tempfile::tempdir().unwrap();
    #[cfg(not(windows))]
    let root = root.path().canonicalize().unwrap();
    #[cfg(not(windows))]
    let root = root.as_path();
    #[cfg(windows)]
    let root = root.path();
    let skill = root.join("example");
    fs::create_dir_all(skill.join("references")).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: example\ndescription: Test skill\n---\nRead the reference.",
    )
    .unwrap();
    fs::write(skill.join("references/context.md"), "Context").unwrap();
    fs::write(skill.join("README.md"), "Not installed").unwrap();
    let found = discover_directory(root).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(
        discover_directory(&skill).unwrap(),
        vec![SystemSkill {
            path: skill.to_string_lossy().into_owned(),
            name: "example".into(),
            origin: "Selected folder".into()
        }]
    );
    let prepared = prepare_directory(&skill).unwrap();
    assert_eq!(prepared.ignored, vec!["README.md"]);
    let store_root = tempfile::tempdir().unwrap();
    let store = crate::Store::open(store_root.path(), &[]).unwrap();
    let preview = store.preview(&prepared.source).unwrap();
    assert_eq!(preview.files.len(), 1);
    assert_eq!(preview.files[0].path, "references/context.md");
    assert!(store.list().unwrap().is_empty());
    let installed = store
        .install(&prepared.source, &preview.digest, false)
        .unwrap();
    assert_eq!(
        store
            .file(
                &installed[0].id,
                installed[0].revision,
                "references/context.md"
            )
            .unwrap(),
        b"Context"
    );
    assert_eq!(
        fs::read_to_string(skill.join("README.md")).unwrap(),
        "Not installed"
    );
    #[cfg(unix)]
    {
        let link = root.join("linked");
        std::os::unix::fs::symlink(&skill, &link).unwrap();
        assert!(discover_directory(&link).is_err());
        assert!(prepare_directory(&link).is_err());
    }
    fs::write(skill.join("references/SKILL.md"), "nested").unwrap();
    assert!(prepare_directory(&skill).is_err());
}

#[test]
fn scan_and_package_limits_reject_without_following_links() {
    let root = tempfile::tempdir().unwrap();
    for index in 0..=MAX_VISITED {
        fs::write(root.path().join(index.to_string()), "").unwrap();
    }
    assert!(discover_directory(root.path()).is_err());
    let other = tempfile::tempdir().unwrap();
    fs::write(
        other.path().join("SKILL.md"),
        vec![b'x'; MAX_SKILL + 1],
    )
    .unwrap();
    assert!(prepare_directory(other.path()).is_err());
    assert!(prepare_directory(Path::new("relative")).is_err());
}

#[cfg(windows)]
#[test]
fn ancestor_junction_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("skill");
    fs::create_dir(&target).unwrap();
    fs::write(
        target.join("SKILL.md"),
        "---\nname: test\ndescription: Test\n---\nBody",
    )
    .unwrap();
    let link = root.path().join("linked");
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(outside.path())
        .status()
        .unwrap();
    assert!(status.success());
    assert!(prepare_directory(&link.join("skill")).is_err());
    fs::remove_dir(link).unwrap();
}
