//! Fixed vectors for the byte contract in docs/14 §3.1; no store or filesystem.

use super::{ContentIdentity, digest};

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

fn identity<'a>(version: Option<&'a str>, license: Option<&'a str>) -> ContentIdentity<'a> {
    ContentIdentity {
        name: "guide",
        description: "test skill",
        version,
        body: "body",
        license,
    }
}

#[test]
fn optional_fields_and_conditional_package_tail_match_fixed_vectors() {
    for (version, license, expected) in [
        (
            None,
            None,
            "a738f7fd408226db7f39ffb331ade74d1095d569a343ea9ce666f074351573cb",
        ),
        (
            Some(""),
            None,
            "06407a8e82b49c73b6adf3f212420b8e0077c95883fa8f085824ebf8c2666a59",
        ),
        (
            None,
            Some(""),
            "fe3469f99f29a254a6189f75e5c62c8d2b3813b10dbbe5fae054c7c60c1b1883",
        ),
        (
            None,
            Some("MIT"),
            "cfdfc1aa6248e2e40e3d68d436bda2531eb9572fe0fa09f6dede0ac490ab5047",
        ),
    ] {
        assert_eq!(digest(identity(version, license), []), expected);
    }
}

#[test]
fn sorted_files_include_package_marker_license_and_each_path_hash_pair() {
    let files = [("assets/a.bin", EMPTY_SHA256), ("scripts/z.sh", ABC_SHA256)];
    let mut remaining = files.into_iter();
    // A one-pass iterator is sufficient; the first file must not be lost when
    // deciding whether to write the package tail.
    assert_eq!(
        digest(
            identity(None, None),
            std::iter::from_fn(|| remaining.next())
        ),
        "42d4089aa899ca6909e2ce724a633a78ca73bbe26724409adb4ba20b32f69c7c"
    );
    assert_eq!(
        digest(identity(None, Some("MIT")), files),
        "8f44d65feca19643d4376b111768d900e616d9b8a32d5e5102d5fe1893ad5833"
    );
    assert_ne!(
        digest(identity(None, None), files),
        digest(
            identity(None, None),
            [("assets/b.bin", EMPTY_SHA256), files[1]]
        )
    );
    assert_ne!(
        digest(identity(None, None), files),
        digest(
            identity(None, None),
            [("assets/a.bin", ABC_SHA256), files[1]]
        )
    );
}

#[test]
fn utf8_lengths_are_bytes_and_text_is_not_normalized() {
    assert_eq!(
        digest(
            ContentIdentity {
                name: "guide",
                description: "出行🚆",
                version: Some("版一"),
                body: "正文\r\né",
                license: Some("许可"),
            },
            [("references/a.md", ABC_SHA256)],
        ),
        "1440f8fa80ce9d82fae4c910d2ae1465a7b3e706555e6bb64da2f133302bebea"
    );
    let raw = ContentIdentity {
        body: " body\r\n",
        ..identity(None, None)
    };
    assert_eq!(
        digest(raw, []),
        "00b766b13756e19a9fd3e8e6d83d257715e4910b71adafcac4f955b19617f89a"
    );
    assert_ne!(
        digest(
            ContentIdentity {
                body: " body\r\n",
                ..identity(None, None)
            },
            []
        ),
        digest(
            ContentIdentity {
                body: "body\n",
                ..identity(None, None)
            },
            []
        )
    );
}

#[test]
fn field_boundaries_cannot_be_collapsed_by_concatenation() {
    for (name, description, expected) in [
        (
            "ab",
            "c",
            "dd531464772eb577001db78677cddc1d7f126d29ddc0065cc4a683173ee60c6a",
        ),
        (
            "a",
            "bc",
            "d65e972a55cc33c4332caaabe6a9d248300960b88220152e339a8ffa8b685e7a",
        ),
    ] {
        assert_eq!(
            digest(
                ContentIdentity {
                    name,
                    description,
                    ..identity(None, None)
                },
                []
            ),
            expected
        );
    }
}
