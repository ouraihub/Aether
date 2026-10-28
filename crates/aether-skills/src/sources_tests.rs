//! Remote acquisition boundary checks; no network or command wiring required.
#![expect(clippy::unwrap_used, reason = "test assertions")]

use super::*;

fn entry(path: &str, kind: &str, mode: &str, size: u64) -> TreeEntry {
    TreeEntry {
        path: path.into(),
        kind: kind.into(),
        mode: mode.into(),
        size: Some(size),
    }
}

fn skill_tree() -> Vec<TreeEntry> {
    vec![
        entry("skills", "tree", "040000", 0),
        entry("skills/one", "tree", "040000", 0),
        entry("skills/one/SKILL.md", "blob", "100644", 80),
        entry("skills/one/scripts", "tree", "040000", 0),
        entry("skills/one/scripts/a.py", "blob", "100755", 8),
        entry("skills/one/README.md", "blob", "100644", 30),
        entry("skills/two", "tree", "040000", 0),
        entry("skills/two/SKILL.md", "blob", "100644", 80),
        entry("package.json", "blob", "100644", 8),
    ]
}

#[test]
fn github_urls_reject_untrusted_and_normalized_paths() {
    for url in [
        "http://github.com/a/b",
        "https://127.0.0.1/a/b",
        "https://github.com.evil/a/b",
        "https://u:p@github.com/a/b",
        "https://github.com:444/a/b",
        "https://github.com/a/b?token=secret",
        "https://github.com/a/b#fragment",
        "https://github.com/a/b/tree/main/../x",
        "https://github.com/a/b/tree/main/%2e%2e/x",
        "https://github.com/a/b/tree/main/x%2Fy",
        "https://github.com/a/b/tree/main//x",
        "https://github.com/a/b/blob/main/README.md",
        "https://github.com/a/b/tree/main\\x",
    ] {
        assert!(parse_github_url(url).is_err(), "{url}");
    }
    for url in [
        "https://github.com/a/b",
        "https://github.com/a/b/",
        "https://github.com/a/b/tree/main",
        "https://github.com/a/b/blob/main/SKILL.md",
        "https://github.com/a/b/blob/feature/slash/skills/one/SKILL.md",
        "https://raw.githubusercontent.com/a/b/refs/heads/feature/slash/skills/one/SKILL.md",
    ] {
        assert!(parse_github_url(url).is_ok(), "{url}");
    }
    let parsed = parse_github_url(
        "https://raw.githubusercontent.com/a/b/refs/heads/feature/slash/one/SKILL.md",
    )
    .unwrap();
    assert_eq!(parsed.namespace.as_deref(), Some("heads"));
    assert_eq!(parsed.tail, ["feature", "slash", "one"]);
}

#[test]
fn selection_never_mixes_sibling_skills_or_repository_files() {
    let entries = skill_tree();
    let target = vec!["skills".into(), "one".into()];
    let (files, ignored) = selected(&entries, &target).unwrap();
    assert_eq!(
        files.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
        ["skills/one/SKILL.md", "skills/one/scripts/a.py"]
    );
    assert_eq!(ignored, ["README.md"]);
    assert_eq!(candidates(&entries).unwrap().len(), 2);
    let location = parse_github_url("https://github.com/a/b").unwrap();
    assert_eq!(
        choose_target(&entries, &location, vec![])
            .unwrap_err()
            .details()
            .reason(),
        Some("multiple_skills")
    );
}

#[test]
fn ref_namespace_collisions_are_explicit_errors() {
    let branch = GitObject {
        sha: "a".repeat(40),
        kind: "commit".into(),
    };
    let tag = GitObject {
        sha: "b".repeat(40),
        kind: "commit".into(),
    };
    assert_eq!(
        choose_ref(Some(branch), Some(tag))
            .unwrap_err()
            .details()
            .reason(),
        Some("ambiguous_ref")
    );
    let branch = GitObject {
        sha: "a".repeat(40),
        kind: "commit".into(),
    };
    assert_eq!(
        choose_ref(Some(branch), None).unwrap().unwrap().sha,
        "a".repeat(40)
    );
}

#[test]
fn selection_rejects_links_devices_case_collisions_and_oversize() {
    let target = vec!["skills".into(), "one".into()];
    for bad in [
        entry("skills/one/scripts/a.py", "blob", "120000", 4),
        entry("skills/one/assets", "commit", "160000", 0),
        entry("skills/one/scripts/CON.txt", "blob", "100644", 4),
        entry("skills/one/scripts/../bad", "blob", "100644", 4),
        entry(
            "skills/one/assets/big",
            "blob",
            "100644",
            MAX_FILE as u64 + 1,
        ),
        entry("skills/one/scripts/A.py", "blob", "100644", 4),
        entry("skills/one/scripts/child/SKILL.md", "blob", "100644", 4),
        entry("skills/one/scripts/a/b/c/d/file", "blob", "100644", 4),
    ] {
        let mut entries = skill_tree();
        entries.push(bad);
        assert!(selected(&entries, &target).is_err());
    }
    let mut entries = skill_tree();
    for index in 0..17 {
        entries.push(entry(
            &format!("skills/one/scripts/f{index}"),
            "blob",
            "100644",
            MAX_FILE as u64,
        ));
    }
    assert_eq!(
        selected(&entries, &target).unwrap_err().details().reason(),
        Some("too_large")
    );
}

#[test]
fn package_depth_is_relative_not_repository_depth() {
    let directory = "a/b/c/d/e/f";
    let mut entries = Vec::new();
    let target: Vec<String> = directory.split('/').map(str::to_owned).collect();
    for index in 1..=target.len() {
        entries.push(entry(&target[..index].join("/"), "tree", "040000", 0));
    }
    entries.push(entry(
        &format!("{directory}/SKILL.md"),
        "blob",
        "100644",
        40,
    ));
    assert!(selected(&entries, &target).is_ok());
    assert!(package_path("scripts/a/b/c/file.py"));
    assert!(!package_path("scripts/a/b/c/d/file.py"));
}

#[test]
fn prepared_zip_uses_real_source_preview_and_digest_install() {
    let bytes = zip_package(&[
        (
            "SKILL.md".into(),
            b"---\nname: sample\ndescription: Safe sample\n---\nRead this.\n".to_vec(),
        ),
        ("LICENSE.txt".into(), b"Example license".to_vec()),
        ("scripts/a.py".into(), b"print(1)".to_vec()),
    ])
    .unwrap();
    let prepared = PreparedSkill {
        source: crate::Source::Bytes {
            file_name: "sample.zip".into(),
            bytes,
        },
        origin: "https://github.com/a/b/blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/SKILL.md"
            .into(),
        ignored: vec!["README.md".into()],
    };
    let encoded = serde_json::to_value(&prepared).unwrap();
    assert_eq!(encoded["source"]["bytes"]["fileName"], "sample.zip");
    let source: crate::Source = serde_json::from_value(encoded["source"].clone()).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let store = crate::Store::open(directory.path(), &[]).unwrap();
    let preview = store.preview(&source).unwrap();
    assert_eq!(preview.name, "sample");
    assert_eq!(preview.files.len(), 1);
    let installed = store.install(&source, &preview.digest, false).unwrap();
    assert_eq!(installed.len(), 1);
}

#[tokio::test]
#[ignore = "requires public GitHub network access and unauthenticated API quota"]
async fn public_github_prepare_preview_install() {
    let prepared =
        prepare("https://github.com/anthropics/skills/blob/main/skills/internal-comms/SKILL.md")
            .await
            .unwrap();
    let pinned = parse_github_url(&prepared.origin).unwrap();
    assert!(sha(&pinned.tail[0]));
    let directory = tempfile::tempdir().unwrap();
    let store = crate::Store::open(directory.path(), &[]).unwrap();
    let preview = store.preview(&prepared.source).unwrap();
    assert_eq!(preview.name, "internal-comms");
    let installed = store
        .install(&prepared.source, &preview.digest, false)
        .unwrap();
    assert_eq!(installed.len(), 1);
}
