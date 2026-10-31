use super::*;

fn zip_skill(name: &str, files: &[(&str, &[u8])]) -> Source {
    use std::io::Write;
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file("SKILL.md", options).unwrap();
    writer
        .write_all(skill_md(name, "run the script").as_bytes())
        .unwrap();
    for (path, data) in files {
        writer.start_file(*path, options).unwrap();
        writer.write_all(data).unwrap();
    }
    Source::Bytes {
        file_name: "skill.zip".into(),
        bytes: writer.finish().unwrap().into_inner(),
    }
}

fn install(store: &Store, source: &Source, replace: bool) -> Vec<SkillMeta> {
    let preview = store.preview(source).unwrap();
    store.install(source, &preview.digest, replace).unwrap()
}

#[test]
fn packaged_files_are_listed_and_readable_by_revision() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path(), &[]).unwrap();
    let source = zip_skill(
        "tool",
        &[
            ("scripts/run.py", b"print(1)\n"),
            ("assets/logo.bin", &[0, 255]),
        ],
    );
    let preview = store.preview(&source).unwrap();
    let listed: Vec<_> = preview
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.kind, f.size))
        .collect();
    assert_eq!(
        listed,
        [
            ("assets/logo.bin", SkillFileKind::Asset, 2),
            ("scripts/run.py", SkillFileKind::Script, 9),
        ]
    );
    let list = store.install(&source, &preview.digest, false).unwrap();
    let meta = &list[0];
    assert!(meta.has_scripts);
    assert_eq!(meta.file_count, 2);
    assert_eq!(store.get(&meta.id).unwrap().files, preview.files);
    assert_eq!(store.files(&meta.id, 1).unwrap(), preview.files);
    assert_eq!(
        store.file(&meta.id, 1, "assets/logo.bin").unwrap(),
        [0, 255]
    );

    let missing = store.file(&meta.id, 1, "scripts/other.py").unwrap_err();
    assert_eq!(missing.code(), KitErrorCode::NotFound, "{missing}");
    let error = assert_conflict(
        store.file(&meta.id, 2, "scripts/run.py"),
        "skill_changed",
    );
    assert_eq!(error.details().entity(), Some(meta.id.as_str()));
    assert_conflict(store.files(&meta.id, 2), "skill_changed");

    // Deleting the skill removes its files with it.
    store.delete(&meta.id).unwrap();
    assert_conflict(
        store.file(&meta.id, 1, "scripts/run.py"),
        "skill_changed",
    );
}

#[test]
fn reads_by_revision_never_mix_in_a_concurrent_replace() {
    let dir = TempDir::new().unwrap();
    let reader = Store::open(dir.path(), &[]).unwrap();
    let writer = Store::open(dir.path(), &[]).unwrap();
    // Odd revisions hold one file, even revisions two, each with its own bytes.
    let odd = zip_skill("tool", &[("scripts/run.py", b"print(1)\n")]);
    let even = zip_skill(
        "tool",
        &[
            ("references/two.md", b"two\n"),
            ("scripts/run.py", b"print(2)\n"),
        ],
    );
    let id = install(&writer, &odd, false)[0].id.clone();

    std::thread::scope(|scope| {
        let replacing = scope.spawn(|| {
            for round in 0..60 {
                install(
                    &writer,
                    if round % 2 == 0 { &even } else { &odd },
                    true,
                );
            }
        });
        while !replacing.is_finished() {
            let revision = reader.list().unwrap()[0].revision;
            let (script, count): (&[u8], usize) = if revision % 2 == 1 {
                (b"print(1)\n", 1)
            } else {
                (b"print(2)\n", 2)
            };
            match reader.file(&id, revision, "scripts/run.py") {
                Ok(content) => {
                    assert_eq!(content, script, "revision {revision}");
                }
                Err(error) => {
                    assert_eq!(error.details().reason(), Some("skill_changed"))
                }
            }
            match reader.files(&id, revision) {
                Ok(files) => {
                    assert_eq!(files.len(), count, "revision {revision}");
                }
                Err(error) => {
                    assert_eq!(error.details().reason(), Some("skill_changed"))
                }
            }
        }
    });
}

#[test]
fn concurrent_duplicate_keeps_instructions_and_script_from_one_revision() {
    use rusqlite::{Connection, TransactionBehavior, params};
    use std::sync::Barrier;
    let dir = TempDir::new().unwrap();
    let reader = Store::open(dir.path(), &[]).unwrap();
    let source = zip_skill("tool", &[("scripts/run.py", b"run the script\n")]);
    let id = install(&reader, &source, false)[0].id.clone();
    let mut writer = Connection::open(dir.path().join("skills.db")).unwrap();
    writer
        .busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    // Race one replace against one copy per round; a copy loop can starve the
    // independent writer past SQLite's busy timeout on slower Windows runners.
    for round in 0..100 {
        let start = Barrier::new(2);
        std::thread::scope(|scope| {
            let replacing = scope.spawn(|| {
                start.wait();
                let tx = writer
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .unwrap();
                let body = if round % 2 == 0 {
                    "first revision"
                } else {
                    "second revision"
                };
                tx.execute(
                    "UPDATE skills SET body = ?1, revision = revision + 1 "
                    "WHERE id = ?2",
                    params![body, id],
                )
                .unwrap();
                tx.execute(
                    "UPDATE skill_files SET content = ?1, size = ?2 "
                    "WHERE skill_id = ?3",
                    params![
                        body.as_bytes(),
                        i64::try_from(body.len()).unwrap(),
                        id,
                    ]
                )?;
                tx.commit().unwrap();
            });
            start.wait();
            let name = format!("copy-{round}");
            let list = reader.duplicate(&id, &name).unwrap();
            let copy = list.iter().find(|skill| skill.name == name).unwrap();
            let body = reader.get(&copy.id).unwrap().body;
            let script = reader
                .file(&copy.id, copy.revision, "scripts/run.py")
                .unwrap();
            assert_eq!(
                body.as_bytes(),
                script,
                "mixed source revisions in {name}",
            );
            replacing.join().unwrap();
        });
    }
}

#[test]
fn a_changed_file_alone_bumps_the_revision() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path(), &[]).unwrap();
    let v1 = zip_skill("tool", &[("scripts/run.py", b"print(1)\n")]);
    let id = install(&store, &v1, false)[0].id.clone();

    // Same bytes again: digest unchanged, revision unchanged.
    assert_eq!(install(&store, &v1, true)[0].revision, 1);

    let v2 = zip_skill("tool", &[("scripts/run.py", b"print(2)\n")]);
    assert_ne!(
        store.preview(&v2).unwrap().digest,
        store.preview(&v1).unwrap().digest
    );
    assert_eq!(install(&store, &v2, true)[0].revision, 2);
    assert_eq!(
        store.file(&id, 2, "scripts/run.py").unwrap(),
        b"print(2)\n"
    );

    // Dropping every file is a change too, and the old files are gone.
    let list = install(&store, &bytes("tool", "run the script"), true);
    assert_eq!(
        (list[0].revision, list[0].file_count, list[0].has_scripts),
        (3, 0, false)
    );
    assert_eq!(
        store
            .file(&id, 3, "scripts/run.py")
            .unwrap_err()
            .code(),
        KitErrorCode::NotFound
    );
}

#[test]
fn editing_the_text_keeps_files_and_duplicate_copies_them() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path(), &[]).unwrap();
    let source = zip_skill(
        "tool",
        &[("references/guide.md", b"guide\n"), ("LICENSE", b"MIT\n")],
    );
    let id = install(&store, &source, false)[0].id.clone();

    let edit = ManualSave {
        id: Some(id.clone()),
        expected_revision: Some(1),
        ..manual("tool", "edited")
    };
    let list = store.save_manual(&edit).unwrap();
    assert_eq!((list[0].revision, list[0].file_count), (2, 1));
    assert_eq!(store.get(&id).unwrap().license.as_deref(), Some("MIT\n"));
    assert_eq!(
        store.file(&id, 2, "references/guide.md").unwrap(),
        b"guide\n"
    );

    let list = store.duplicate(&id, "tool-copy").unwrap();
    let copy = list.iter().find(|s| s.name == "tool-copy").unwrap();
    assert_eq!(copy.file_count, 1);
    assert_eq!(
        store.get(&copy.id).unwrap().license.as_deref(),
        Some("MIT\n")
    );
    assert_eq!(
        store.file(&copy.id, 1, "references/guide.md").unwrap(),
        b"guide\n"
    );

    // Hand-written skills never have files.
    let list = store.save_manual(&manual("plain", "text")).unwrap();
    let plain = list.iter().find(|s| s.name == "plain").unwrap();
    assert_eq!((plain.file_count, plain.has_scripts), (0, false));
    assert!(store.get(&plain.id).unwrap().files.is_empty());
}

#[test]
fn migration_from_v1_keeps_skills_and_revisions() {
    let dir = TempDir::new().unwrap();
    // Build a schema-1 database the way the previous release left it.
    let store = Store::open(dir.path(), &BUILTIN_V1).unwrap();
    store.save_manual(&manual("mine", "body")).unwrap();
    let before = store.list().unwrap();
    drop(store);
    {
        let db = rusqlite::Connection::open(
            dir.path().join("skills.db"),
        )
        .unwrap();
    }

    let store = Store::open(dir.path(), &BUILTIN_V1).unwrap();
    assert_eq!(store.list().unwrap(), before);
    let mine = before.iter().find(|s| s.name == "mine").unwrap();
    assert_eq!(store.get(&mine.id).unwrap().body, "body");
    // The stored digest still matches, so re-saving the same text keeps the
    // revision.
    let same = ManualSave {
        id: Some(mine.id.clone()),
        expected_revision: Some(1),
        ..manual("mine", "body")
    };
    let after = store.save_manual(&same).unwrap();
    assert_eq!(
        after.iter().find(|s| s.name == "mine").unwrap().revision,
        1
    );
    // And the new table works on the migrated database.
    let tool = install(
        &store,
        &zip_skill("tool", &[("scripts/a.sh", b"echo\n")]),
        false,
    );
    assert!(tool.iter().any(|s| s.name == "tool" && s.has_scripts));
}

#[test]
fn package_order_and_stored_hashes_share_the_pure_identity() {
    use crate::content::{ContentIdentity, digest};

    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path(), &[]).unwrap();
    let source = zip_skill(
        "guide",
        &[("scripts/z.sh", b"abc"), ("assets/a.bin", b"")],
    );
    let preview = store.preview(&source).unwrap();
    let expected = digest(
        ContentIdentity {
            name: &preview.name,
            description: &preview.description,
            version: preview.version.as_deref(),
            body: &preview.body,
            license: preview.license.as_deref(),
        },
        [
            (
                "assets/a.bin",
                concat!(
                    concat!(
                        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                    ),
                ),
            ),
            (
                "scripts/z.sh",
                concat!(
                    concat!(
                        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
                    ),
                ),
            ),
        ],
    );
    assert_eq!(preview.digest, expected);
    assert_eq!(store.preview(&reordered).unwrap().digest, expected);
    let installed = store.install(&reordered, &expected, false).unwrap();
    let id = installed[0].id.clone();
    let unchanged = ManualSave {
        id: Some(id.clone()),
        expected_revision: Some(1),
        name: preview.name,
        description: preview.description,
        version: preview.version,
        body: preview.body,
    };
    // Editing reads the persisted hashes in the transaction; it must agree
    // with the package's borrowed, path-sorted hashes without a revision bump.
    assert_eq!(store.save_manual(&unchanged).unwrap()[0].revision, 1);
    assert_eq!(store.file(&id, 1, "scripts/z.sh").unwrap(), b"abc");
    assert_eq!(install(&store, &source, true)[0].revision, 1);
}

#[test]
fn extra_and_display_order_do_not_change_content_identity_or_revision() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path(), &[]).unwrap();
    let with_extra = |value: &str| Source::Bytes {
        file_name: "SKILL.md".into(),
        bytes: format!(
            "---\nname: guide\ndescription: test skill\naudience: {value}\n"
            "---\nbody\n"
        )
        .into_bytes(),
    };
    let first = with_extra("commuters");
    let second = with_extra("everyone");
    let preview = store.preview(&first).unwrap();
    let id = store.install(&first, &preview.digest, false).unwrap()[0]
        .id
        .clone();
    assert_eq!(store.preview(&second).unwrap().digest, preview.digest);
    let replaced = store.install(&second, &preview.digest, true).unwrap();
    assert_eq!(
        (replaced[0].id.as_str(), replaced[0].revision),
        (id.as_str(), 1)
    );
    assert_eq!(
        store
            .get(&id)
            .unwrap()
            .extra
            .get("audience")
            .map(String::as_str),
        Some("everyone")
    );
    let list = store.save_manual(&manual("other", "text")).unwrap();
    let ids: Vec<_> = list.iter().rev().map(|skill| skill.id.clone()).collect();
    store.reorder(&ids).unwrap();
    assert_eq!(store.get(&id).unwrap().meta.revision, 1);
    assert_eq!(store.preview(&second).unwrap().digest, preview.digest);
}

#[test]
fn adding_and_renaming_a_file_changes_identity_and_revision() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path(), &[]).unwrap();
    let empty = zip_skill("tool", &[]);
    let added = zip_skill("tool", &[("assets/a.bin", b"abc")]);
    let renamed = zip_skill("tool", &[("assets/b.bin", b"abc")]);
    let id = install(&store, &empty, false)[0].id.clone();
    assert_eq!(install(&store, &added, true)[0].revision, 2);
    let preview = store.preview(&renamed).unwrap();
    assert_conflict(
        store.install(&renamed, &store.preview(&added).unwrap().digest, true),
        "content_changed",
    );
    assert_eq!(store.get(&id).unwrap().meta.revision, 2);
    assert_eq!(store.file(&id, 2, "assets/a.bin").unwrap(), b"abc");
    assert_eq!(
        store.install(&renamed, &preview.digest, true).unwrap()[0].revision,
        3
    );
    assert_eq!(store.file(&id, 3, "assets/b.bin").unwrap(), b"abc");
    assert_eq!(
        store
            .file(&id, 3, "assets/a.bin")
            .unwrap_err()
            .code(),
        KitErrorCode::NotFound
    );
}
