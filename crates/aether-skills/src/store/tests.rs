//! Store behaviour required by docs/90 SK3-2: confirm by digest, explicit
//! replace, revision guards, built-in sync and run snapshots.
#![expect(clippy::unwrap_used, reason = "test assertions")]

use std::fs;
use std::path::Path;

use aether_support::{KitError, KitErrorCode};
use tempfile::TempDir;

use super::{BuiltinSkill, Store};
use crate::dto::{
    ManualSave, PreviewAction, SkillFileKind, SkillMeta, SkillOrigin, SkillRef,
};

fn skill_md(name: &str, body: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: test skill\n"
        "version: 1.0.0\n---\n{body}\n",
    )
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

fn assert_conflict<T: std::fmt::Debug>(
    result: Result<T, KitError>,
    reason: &str,
) -> KitError {
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

    // Swapped between preview and install: still valid, but not what the user
    // approved.
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
    let names: Vec<_> = store
        .list()
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
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
#[path = "tests_files.rs"]
mod files;
