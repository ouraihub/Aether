//! Store behaviour required by docs/90 SK3-2: confirm by digest, explicit replace,
//! revision guards, built-in sync and run snapshots.
#![expect(clippy::unwrap_used, reason = "test assertions")]

use std::fs;
use std::path::Path;

use aether_support::{KitError, KitErrorCode};
use tempfile::TempDir;

use super::{BuiltinSkill, Store};
use crate::dto::{ManualSave, PreviewAction, SkillFileKind, SkillMeta, SkillOrigin, SkillRef};
use crate::package::Source;

fn skill_md(name: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: test skill\nversion: 1.0.0\n---\n{body}\n")
}

fn write_skill(dir: &Path, name: &str, body: &str) -> Source {
    let path = dir.join("SKILL.md");
    fs::write(&path, skill_md(name, body)).unwrap();
    Source::Path(path)
}

fn bytes(name: &str, body: &str) -> Source {
    Source::Bytes {
        file_name: "SKILL.md".into(),
        bytes: skill_md(name, body).into_bytes(),
    }
}

fn manual(name: &str, body: &str) -> ManualSave {
    ManualSave {
        id: None,
        expected_revision: None,
        name: name.into(),
        description: "hand written".into(),
        version: None,
        body: body.into(),
    }
}

fn assert_conflict<T: std::fmt::Debug>(result: Result<T, KitError>, reason: &str) -> KitError {
    let error = result.unwrap_err();
    assert_eq!(error.code(), KitErrorCode::Conflict, "{error}");
    assert_eq!(error.details().reason(), Some(reason), "{error}");
    error
}

#[test]
fn install_requires_reconfirm_when_file_changed_after_preview() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path(), &[]).unwrap();
    let pkg = TempDir::new().unwrap();
    let source = write_skill(pkg.path(), "travel", "take the bus");
    let first = store.preview(&source).unwrap();
    assert_eq!(first.action, PreviewAction::Install);
    assert!(first.current.is_none());

    // Swapped between preview and install: still valid, but not what the user approved.
    write_skill(pkg.path(), "travel", "take a taxi");
    assert_conflict(
        store.install(&source, &first.digest, false),
        "content_changed",
    );
    assert!(store.list().unwrap().is_empty());

    let second = store.preview(&source).unwrap();
    assert_ne!(second.digest, first.digest);
    let list = store.install(&source, &second.digest, false).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(store.get(&list[0].id).unwrap().body, "take a taxi\n");
}

#[test]
fn same_name_needs_replace_and_bumps_revision() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path(), &[]).unwrap();
    let v1 = store.preview(&bytes("travel", "one")).unwrap();
    let installed = store
        .install(&bytes("travel", "one"), &v1.digest, false)
        .unwrap();
    let id = installed[0].id.clone();
    assert_eq!(installed[0].revision, 1);
    assert_eq!(installed[0].origin, SkillOrigin::Imported);

    let v2 = store.preview(&bytes("travel", "two")).unwrap();
    assert_eq!(v2.action, PreviewAction::Replace);
    let current = v2.current.as_ref().unwrap();
    assert_eq!(
        (current.version.as_deref(), current.revision),
        (Some("1.0.0"), 1)
    );

    assert_conflict(
        store.install(&bytes("travel", "two"), &v2.digest, false),
        "exists",
    );
    assert_eq!(store.get(&id).unwrap().body, "one\n");

    let replaced = store
        .install(&bytes("travel", "two"), &v2.digest, true)
        .unwrap();
    assert_eq!(replaced.len(), 1);
    assert_eq!(
        (replaced[0].id.as_str(), replaced[0].revision),
        (id.as_str(), 2)
    );
    assert_eq!(store.get(&id).unwrap().body, "two\n");
}

#[test]
fn edit_with_stale_expected_revision_conflicts() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path(), &[]).unwrap();
    let id = store.save_manual(&manual("notes", "draft")).unwrap()[0]
        .id
        .clone();

    // Window A saves revision 1 → 2; window B still holds revision 1.
    let edit = |body: &str, expected| ManualSave {
        id: Some(id.clone()),
        expected_revision: expected,
        ..manual("notes", body)
    };
    assert_eq!(
        store.save_manual(&edit("from a", Some(1))).unwrap()[0].revision,
        2
    );
    let error = assert_conflict(
        store.save_manual(&edit("from b", Some(1))),
        "revision_changed",
    );
    assert_eq!(error.details().entity(), Some(id.as_str()));
    assert_eq!(store.get(&id).unwrap().body, "from a");

    let error = store.save_manual(&edit("from b", None)).unwrap_err();
    assert_eq!(error.code(), KitErrorCode::BadRequest);
    // Saving unchanged content keeps the revision.
    assert_eq!(
        store.save_manual(&edit("from a", Some(2))).unwrap()[0].revision,
        2
    );
}

#[test]
fn renaming_onto_a_taken_name_conflicts() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path(), &[]).unwrap();
    store.save_manual(&manual("first", "a")).unwrap();
    assert_conflict(store.save_manual(&manual("first", "b")), "exists");
}

static BUILTIN_V1: [BuiltinSkill; 2] = [
    BuiltinSkill {
        name: "guide",
        description: "built in",
        version: Some("1"),
        body: "old",
    },
    BuiltinSkill {
        name: "retired",
        description: "built in",
        version: None,
        body: "gone",
    },
];
static BUILTIN_V2: [BuiltinSkill; 1] = [BuiltinSkill {
    name: "guide",
    description: "built in",
    version: Some("2"),
    body: "new",
}];

#[test]
fn builtin_sync_updates_builtin_and_keeps_user_copy() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path(), &BUILTIN_V1).unwrap();
    let list = store.list().unwrap();
    let guide = list.iter().find(|s| s.name == "guide").unwrap().clone();
    assert_eq!((guide.origin, guide.revision), (SkillOrigin::Builtin, 1));

    // Built-ins are read-only and their names reserved.
    assert_eq!(
        store.delete(&guide.id).unwrap_err().code(),
        KitErrorCode::BadRequest
    );
    assert_conflict(store.save_manual(&manual("guide", "x")), "builtin_name");
    let preview = store.preview(&bytes("guide", "x"));
    assert_conflict(preview, "builtin_name");

    let copy = store.duplicate(&guide.id, "my-guide").unwrap();
    let copy = copy.iter().find(|s| s.name == "my-guide").unwrap().clone();
    assert_eq!(copy.origin, SkillOrigin::Manual);
    drop(store);

    // Reopening with the same content does not bump the revision.
    let store = Store::open(dir.path(), &BUILTIN_V1).unwrap();
    assert_eq!(store.get(&guide.id).unwrap().meta.revision, 1);
    drop(store);

    let store = Store::open(dir.path(), &BUILTIN_V2).unwrap();
    let updated = store.get(&guide.id).unwrap();
    assert_eq!((updated.body.as_str(), updated.meta.revision), ("new", 2));
    let kept = store.get(&copy.id).unwrap();
    assert_eq!((kept.body.as_str(), kept.meta.revision), ("old", 1));
    assert_eq!(kept.meta.version.as_deref(), Some("1"));
    let names: Vec<_> = store.list().unwrap().into_iter().map(|s| s.name).collect();
    assert_eq!(names, ["guide", "my-guide"]);
}

#[test]
fn snapshot_pins_revision_and_rejects_changed_skill() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path(), &[]).unwrap();
    store.save_manual(&manual("alpha", "body a")).unwrap();
    let list = store.save_manual(&manual("beta", "body b")).unwrap();
    let refs: Vec<_> = list
        .iter()
        .map(|s| SkillRef {
            id: s.id.clone(),
            revision: s.revision,
        })
        .collect();

    let snaps = store.snapshot(&refs).unwrap();
    assert_eq!(snaps.len(), 2);
    assert_eq!(
        (snaps[0].name.as_str(), snaps[0].body.as_str()),
        ("alpha", "body a")
    );

    // Reordering does not change revisions, so the selection stays valid.
    let reversed: Vec<_> = list.iter().rev().map(|s| s.id.clone()).collect();
    store.reorder(&reversed).unwrap();
    assert_eq!(store.snapshot(&refs).unwrap().len(), 2);

    let edit = ManualSave {
        id: Some(refs[1].id.clone()),
        expected_revision: Some(1),
        ..manual("beta", "changed")
    };
    store.save_manual(&edit).unwrap();
    let error = assert_conflict(store.snapshot(&refs), "skill_changed");
    assert_eq!(error.details().entity(), Some(refs[1].id.as_str()));

    store.delete(&refs[0].id).unwrap();
    let error = assert_conflict(store.snapshot(&refs[..1]), "skill_changed");
    assert_eq!(error.details().entity(), Some(refs[0].id.as_str()));
}

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
    let error = assert_conflict(store.file(&meta.id, 2, "scripts/run.py"), "skill_changed");
    assert_eq!(error.details().entity(), Some(meta.id.as_str()));
    assert_conflict(store.files(&meta.id, 2), "skill_changed");

    // Deleting the skill removes its files with it.
    store.delete(&meta.id).unwrap();
    assert_conflict(store.file(&meta.id, 1, "scripts/run.py"), "skill_changed");
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
                install(&writer, if round % 2 == 0 { &even } else { &odd }, true);
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
                Ok(content) => assert_eq!(content, script, "revision {revision}"),
                Err(error) => assert_eq!(error.details().reason(), Some("skill_changed")),
            }
            match reader.files(&id, revision) {
                Ok(files) => assert_eq!(files.len(), count, "revision {revision}"),
                Err(error) => assert_eq!(error.details().reason(), Some("skill_changed")),
            }
        }
    });
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
    assert_eq!(store.file(&id, 2, "scripts/run.py").unwrap(), b"print(2)\n");

    // Dropping every file is a change too, and the old files are gone.
    let list = install(&store, &bytes("tool", "run the script"), true);
    assert_eq!(
        (list[0].revision, list[0].file_count, list[0].has_scripts),
        (3, 0, false)
    );
    assert_eq!(
        store.file(&id, 3, "scripts/run.py").unwrap_err().code(),
        KitErrorCode::NotFound
    );
}

#[test]
fn editing_the_text_keeps_files_and_duplicate_copies_them() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path(), &[]).unwrap();
    let source = zip_skill("tool", &[("references/guide.md", b"guide\n")]);
    let id = install(&store, &source, false)[0].id.clone();

    let edit = ManualSave {
        id: Some(id.clone()),
        expected_revision: Some(1),
        ..manual("tool", "edited")
    };
    let list = store.save_manual(&edit).unwrap();
    assert_eq!((list[0].revision, list[0].file_count), (2, 1));
    assert_eq!(
        store.file(&id, 2, "references/guide.md").unwrap(),
        b"guide\n"
    );

    let list = store.duplicate(&id, "tool-copy").unwrap();
    let copy = list.iter().find(|s| s.name == "tool-copy").unwrap();
    assert_eq!(copy.file_count, 1);
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
        let db = rusqlite::Connection::open(dir.path().join("skills.db")).unwrap();
        db.execute_batch("DROP TABLE skill_files; PRAGMA user_version = 1;")
            .unwrap();
    }

    let store = Store::open(dir.path(), &BUILTIN_V1).unwrap();
    assert_eq!(store.list().unwrap(), before);
    let mine = before.iter().find(|s| s.name == "mine").unwrap();
    assert_eq!(store.get(&mine.id).unwrap().body, "body");
    // The stored digest still matches, so re-saving the same text keeps the revision.
    let same = ManualSave {
        id: Some(mine.id.clone()),
        expected_revision: Some(1),
        ..manual("mine", "body")
    };
    let after = store.save_manual(&same).unwrap();
    assert_eq!(after.iter().find(|s| s.name == "mine").unwrap().revision, 1);
    // And the new table works on the migrated database.
    let tool = install(
        &store,
        &zip_skill("tool", &[("scripts/a.sh", b"echo\n")]),
        false,
    );
    assert!(tool.iter().any(|s| s.name == "tool" && s.has_scripts));
}
