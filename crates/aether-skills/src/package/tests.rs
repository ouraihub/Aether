//! One case per `fixtures/skills/` entry (docs/90 SK3-1). ZIPs, links and oversize
//! files are built in temp dirs instead of committed as binaries.
#![expect(clippy::unwrap_used, clippy::panic, reason = "test assertions")]

use std::io::Write;
use std::path::{Path, PathBuf};

use aether_support::{KitError, KitErrorCode};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use super::{
    MAX_DEPTH, MAX_ENTRIES, MAX_FILE, MAX_SKILL, MAX_TOTAL, MAX_ZIP, Package, Source, read,
};
use crate::dto::SkillFileKind;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/skills")
        .join(name)
}

fn ok(source: &Source) -> Package {
    read(source).unwrap_or_else(|e| panic!("expected ok, got {e}"))
}

fn err(source: &Source) -> KitError {
    match read(source) {
        Ok(package) => panic!("expected error, got {package:?}"),
        Err(error) => error,
    }
}

/// Asserts `bad_request` with the given `field` and `reason`.
fn assert_bad(error: &KitError, field: &str, reason: &str) {
    assert_eq!(error.code(), KitErrorCode::BadRequest, "{error}");
    assert_eq!(error.details().field(), Some(field), "{error}");
    assert_eq!(error.details().reason(), Some(reason), "{error}");
}

fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, data) in entries {
        writer
            .start_file(*name, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn zip_source(bytes: Vec<u8>) -> Source {
    Source::Bytes {
        file_name: "skill.zip".to_owned(),
        bytes,
    }
}

const SKILL: &[u8] = b"---\nname: zipped\ndescription: from a zip\n---\nbody\n";

#[test]
fn valid_directory_keeps_body_license_and_extra_verbatim() {
    let package = ok(&Source::Path(fixture("valid")));
    let skill = &package.skill;
    assert_eq!(skill.name, "travel-planner");
    assert_eq!(skill.description, "帮用户规划 \"公共交通\" 出行");
    assert_eq!(skill.version.as_deref(), Some("1.2.0"));
    assert_eq!(
        skill.extra.get("audience").map(String::as_str),
        Some("commuters")
    );
    assert_eq!(skill.extra.len(), 1);
    // A `---` rule inside the body is content, not a new document.
    assert_eq!(
        skill.body,
        "# 出行规划\n\n先确认出发地与目的地。\n\n---\n\n分隔线之后的正文原样保留。\n"
    );
    assert_eq!(package.license.as_deref(), Some("MIT License\n"));
}

#[test]
fn missing_version_stays_unversioned() {
    let package = ok(&Source::Bytes {
        file_name: "SKILL.md".to_owned(),
        bytes: SKILL.to_vec(),
    });
    assert_eq!(package.skill.version, None);
    assert_eq!(package.license, None);
}

#[test]
fn unsupported_yaml_reports_the_offending_line() {
    for (file, line) in [
        ("duplicate-key.md", "4"),
        ("nested.md", "4"),
        ("anchor.md", "3"),
        ("block-scalar.md", "3"),
        ("multi-doc.md", "5"),
    ] {
        assert_bad(
            &err(&Source::Path(fixture(file))),
            line,
            "frontmatter_unsupported",
        );
    }
}

#[test]
fn directory_with_script_lists_unsupported_files() {
    let error = err(&Source::Path(fixture("with-script")));
    assert_bad(&error, "run.sh", "unsupported_files");
}

#[test]
fn credential_is_located_without_echoing_it() {
    let error = err(&Source::Path(fixture("secret.md")));
    assert_bad(&error, "SKILL.md:6", "secret_detected");
    // Display and Debug cover everything the UI or a log could see.
    let visible = format!("{error} {error:?}");
    assert!(!visible.contains("sk-ant"), "{visible}");
}

#[test]
fn crlf_and_bom_are_normalized() {
    let skill = ok(&Source::Path(fixture("crlf-bom.md"))).skill;
    assert_eq!(skill.name, "crlf-bom");
    assert_eq!(skill.description, "Windows editor output");
    assert_eq!(skill.body, "line one\nline two\n");
}

#[test]
fn zip_under_single_top_level_directory_is_accepted() {
    let bytes = zip_bytes(&[("pkg/SKILL.md", SKILL), ("pkg/LICENSE.txt", b"MIT")]);
    let package = ok(&zip_source(bytes));
    assert_eq!(package.skill.name, "zipped");
    assert_eq!(package.license.as_deref(), Some("MIT"));
}

#[test]
fn zip_bomb_stops_at_the_bytes_actually_inflated() {
    // 10 MiB of one repeated byte deflates to ~10 KiB: small archive, oversize content.
    let mut bomb = SKILL.to_vec();
    bomb.resize(10 * 1024 * 1024, b'a');
    let bytes = zip_bytes(&[("SKILL.md", &bomb)]);
    assert!(bytes.len() < MAX_ZIP);
    let error = err(&zip_source(bytes));
    assert_bad(&error, "SKILL.md", "too_large");
    assert_eq!(error.details().limit(), Some(MAX_SKILL as u64));
}

#[test]
fn zip_rejects_traversal_absolute_and_backslash_paths() {
    for (name, reason) in [
        ("../SKILL.md", "path_traversal"),
        ("pkg/../../SKILL.md", "path_traversal"),
        ("/SKILL.md", "absolute_path"),
        ("C:/SKILL.md", "absolute_path"),
        ("pkg\\SKILL.md", "backslash_path"),
    ] {
        let error = err(&zip_source(zip_bytes(&[(name, SKILL)])));
        assert_bad(&error, "zip", reason);
    }
}

#[test]
fn zip_rejects_paths_equal_after_normalization() {
    let bytes = zip_bytes(&[("SKILL.md", SKILL), ("./SKILL.md", SKILL)]);
    assert_bad(&err(&zip_source(bytes)), "zip", "duplicate_path");
}

#[test]
fn zip_rejects_symlink_entries() {
    let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .add_symlink("SKILL.md", "/etc/passwd", SimpleFileOptions::default())
        .unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    assert_bad(&err(&zip_source(bytes)), "zip", "link");
}

#[test]
fn zip_with_script_lists_unsupported_files() {
    let bytes = zip_bytes(&[("SKILL.md", SKILL), ("run.sh", b"echo"), ("a.py", b"")]);
    assert_bad(
        &err(&zip_source(bytes)),
        "a.py, run.sh",
        "unsupported_files",
    );
}

#[test]
fn zip_entry_count_and_archive_size_are_capped() {
    let names: Vec<String> = (0..=MAX_ENTRIES).map(|i| format!("f{i}")).collect();
    let entries: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b""[..])).collect();
    assert_bad(
        &err(&zip_source(zip_bytes(&entries))),
        "entries",
        "too_large",
    );
    assert_bad(&err(&zip_source(vec![0; MAX_ZIP + 1])), "zip", "too_large");
}

#[test]
fn oversize_body_is_rejected() {
    let mut text = SKILL.to_vec();
    text.resize(MAX_SKILL + 1, b'a');
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("SKILL.md");
    std::fs::write(&path, &text).unwrap();
    let error = err(&Source::Path(path));
    assert_bad(&error, "SKILL.md", "too_large");
    assert_eq!(error.details().limit(), Some(MAX_SKILL as u64));
    // Exactly at the limit is fine.
    text.truncate(MAX_SKILL);
    ok(&Source::Bytes {
        file_name: "SKILL.md".to_owned(),
        bytes: text,
    });
}

#[cfg(windows)]
#[test]
fn directory_junction_is_rejected_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    // A valid package behind the junction: following it would succeed, so only the
    // link check can make this fail. Temp paths use `\`, which `mklink` requires.
    let target = tempfile::tempdir().unwrap();
    std::fs::write(target.path().join("SKILL.md"), SKILL).unwrap();
    let target = target.path();
    let link = dir.path().join("junction");
    // Junctions need no privilege, unlike symlinks (docs/14 §7.4).
    let status = std::process::Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(target)
        .output()
        .unwrap();
    assert!(status.status.success(), "{status:?}");
    // As the import root and as an entry inside a package directory.
    assert_bad(&err(&Source::Path(link)), "path", "link");
    std::fs::write(dir.path().join("SKILL.md"), SKILL).unwrap();
    assert_bad(
        &err(&Source::Path(dir.path().to_owned())),
        "junction",
        "link",
    );
}

#[test]
fn file_symlink_inside_directory_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(windows)]
    let made =
        std::os::windows::fs::symlink_file(fixture("valid/SKILL.md"), dir.path().join("SKILL.md"));
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(fixture("valid/SKILL.md"), dir.path().join("SKILL.md"));
    // Windows symlinks need Developer Mode or admin; junction coverage is above.
    if made.is_err() {
        return;
    }
    assert_bad(
        &err(&Source::Path(dir.path().to_owned())),
        "SKILL.md",
        "link",
    );
}

#[test]
fn relative_path_is_refused() {
    assert_bad(
        &err(&Source::Path(PathBuf::from("SKILL.md"))),
        "path",
        "not_absolute",
    );
}

fn paths(package: &Package) -> Vec<(&str, SkillFileKind)> {
    package
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.kind))
        .collect()
}

#[test]
fn directory_with_scripts_references_and_assets_is_read_recursively() {
    let package = ok(&Source::Path(fixture("with-files")));
    assert_eq!(package.skill.name, "with-files");
    assert_eq!(
        paths(&package),
        [
            ("assets/data.json", SkillFileKind::Asset),
            ("references/guide.md", SkillFileKind::Reference),
            ("scripts/lib/util.sh", SkillFileKind::Script),
            ("scripts/run.py", SkillFileKind::Script),
        ]
    );
    let run = &package.files[3];
    assert_eq!(run.content, b"print(\"hi\")\n");
    // Lowercase hex sha256 of the bytes as read.
    assert_eq!(run.sha256.len(), 64);
    assert_ne!(run.sha256, package.files[2].sha256);
}

#[test]
fn zip_keeps_binary_assets_byte_for_byte() {
    let png: &[u8] = &[0x89, b'P', b'N', b'G', 0xff, 0x00, 0xfe];
    let bytes = zip_bytes(&[
        ("pkg/SKILL.md", SKILL),
        ("pkg/scripts/run.py", b"print(1)\n"),
        ("pkg/references/a/b/c/deep.md", b"deep\n"),
        ("pkg/assets/logo.png", png),
    ]);
    let package = ok(&zip_source(bytes));
    assert_eq!(
        paths(&package),
        [
            ("assets/logo.png", SkillFileKind::Asset),
            ("references/a/b/c/deep.md", SkillFileKind::Reference),
            ("scripts/run.py", SkillFileKind::Script),
        ]
    );
    assert_eq!(package.files[0].content, png);
}

#[test]
fn package_paths_must_be_plain_names_within_four_levels() {
    for (name, reason) in [
        ("scripts/.hidden.sh", "invalid_name"),
        ("scripts/.git/config", "invalid_name"),
        ("scripts/my script.sh", "invalid_name"),
        ("references/指南.md", "invalid_name"),
        ("assets/a/b/c/d/deep.bin", "too_deep"),
    ] {
        let bytes = zip_bytes(&[("SKILL.md", SKILL), (name, b"x")]);
        let error = err(&zip_source(bytes));
        assert_bad(&error, name, reason);
        if reason == "too_deep" {
            assert_eq!(error.details().limit(), Some(MAX_DEPTH as u64));
        }
    }
    // Outside the three directories nothing changed: still `unsupported_files`.
    let bytes = zip_bytes(&[
        ("SKILL.md", SKILL),
        ("resources/x.md", b"x"),
        ("scripts", b"x"),
    ]);
    assert_bad(
        &err(&zip_source(bytes)),
        "resources/x.md, scripts",
        "unsupported_files",
    );
}

#[test]
fn directory_depth_and_names_are_checked_while_walking() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("SKILL.md"), SKILL).unwrap();
    let deep = dir.path().join("scripts/a/b/c/d");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("x.sh"), "echo").unwrap();
    let source = Source::Path(dir.path().to_owned());
    let error = err(&source);
    assert_bad(&error, "scripts/a/b/c/d", "too_deep");

    std::fs::remove_dir_all(dir.path().join("scripts/a")).unwrap();
    std::fs::create_dir_all(dir.path().join("references/.cache")).unwrap();
    assert_bad(&err(&source), "references/.cache", "invalid_name");
}

#[test]
fn scripts_and_references_must_be_utf8_but_assets_may_be_binary() {
    let latin1: &[u8] = b"caf\xe9\n";
    for name in ["scripts/run.py", "references/notes.md"] {
        let bytes = zip_bytes(&[("SKILL.md", SKILL), (name, latin1)]);
        assert_bad(&err(&zip_source(bytes)), name, "not_utf8");
    }
    let bytes = zip_bytes(&[("SKILL.md", SKILL), ("assets/notes.txt", latin1)]);
    assert_eq!(ok(&zip_source(bytes)).files[0].content, latin1);
}

#[test]
fn credentials_in_packaged_text_files_are_located() {
    let key = format!("x = 1\n-----BEGIN RSA {} KEY-----\n", "PRIVATE");
    for name in ["scripts/run.py", "references/notes.md", "assets/config.ini"] {
        let bytes = zip_bytes(&[("SKILL.md", SKILL), (name, key.as_bytes())]);
        let error = err(&zip_source(bytes));
        assert_bad(&error, &format!("{name}:2"), "secret_detected");
        assert!(!format!("{error} {error:?}").contains("BEGIN"), "{error}");
    }
}

#[test]
fn oversize_packaged_file_is_rejected_at_one_mib() {
    // Highly compressible, so the archive stays far below the ZIP cap.
    let mut data = vec![b'a'; MAX_FILE];
    let bytes = zip_bytes(&[("SKILL.md", SKILL), ("assets/big.txt", &data)]);
    assert_eq!(ok(&zip_source(bytes)).files[0].content.len(), MAX_FILE);
    data.push(b'a');
    let bytes = zip_bytes(&[("SKILL.md", SKILL), ("assets/big.txt", &data)]);
    let error = err(&zip_source(bytes));
    assert_bad(&error, "assets/big.txt", "too_large");
    assert_eq!(error.details().limit(), Some(MAX_FILE as u64));
}

#[test]
fn cumulative_inflated_size_is_capped() {
    let data = vec![b'a'; MAX_FILE];
    let names: Vec<String> = (0..=MAX_TOTAL / MAX_FILE)
        .map(|i| format!("assets/f{i}.txt"))
        .collect();
    let mut entries: Vec<(&str, &[u8])> = vec![("SKILL.md", SKILL)];
    entries.extend(names.iter().map(|n| (n.as_str(), data.as_slice())));
    let bytes = zip_bytes(&entries);
    assert!(bytes.len() < MAX_ZIP);
    let error = err(&zip_source(bytes));
    assert_bad(&error, "total", "too_large");
    assert_eq!(error.details().limit(), Some(MAX_TOTAL as u64));
}

#[test]
fn directory_entry_count_is_capped() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("SKILL.md"), SKILL).unwrap();
    let assets = dir.path().join("assets");
    std::fs::create_dir(&assets).unwrap();
    for i in 0..MAX_ENTRIES {
        std::fs::write(assets.join(format!("f{i}")), "x").unwrap();
    }
    assert_bad(
        &err(&Source::Path(dir.path().to_owned())),
        "entries",
        "too_large",
    );
}
