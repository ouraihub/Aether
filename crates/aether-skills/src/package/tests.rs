//! One case per `fixtures/skills/` entry (docs/90 SK3-1). ZIPs, links and oversize
//! files are built in temp dirs instead of committed as binaries.
#![expect(clippy::unwrap_used, clippy::panic, reason = "test assertions")]

use std::io::Write;
use std::path::{Path, PathBuf};

use aether_support::{KitError, KitErrorCode};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use super::{MAX_SKILL, MAX_ZIP, Package, Source, read};

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
    let names: Vec<String> = (0..9).map(|i| format!("f{i}")).collect();
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
