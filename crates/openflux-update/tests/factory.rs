//! The factory software reference: parsing, validation, and how the index
//! carries it.
#![cfg(feature = "signing")]

mod common;

use std::collections::BTreeMap;

use openflux_update::engine::Catalog;
use openflux_update::factory::{
    FactoryReference, PressRefinementSource, ReferenceFile, SourceLocation,
};
use openflux_update::manifest::{Channel, Index};
use openflux_update::signing::{generate, GeneratedKey};
use openflux_update::trust::{
    verify_bundled_index, verify_index, IndexError, MemoryTrustStore, TrustAnchors, TrustState,
};
use openflux_update::version::Component;
use time::macros::datetime;
use time::OffsetDateTime;

const NOW: OffsetDateTime = datetime!(2030-06-01 00:00 UTC);

fn reference_toml(label: &str) -> String {
    format!(
        r#"
[factory]
label = "{label}"
container_url = "https://downloads.example.com/keyboard/2.3/package.zip"
container_size = 1000000

[[factory.files]]
component = "firmware"
member = "assets/firmware/scanner_2.3.uf2"
version = "2.3"
size = 1000
sha256 = "{a}"

[[factory.files]]
component = "bridge"
member = "assets/packages/bridge_2.3_aarch64.ipk"
version = "2.3"
size = 2000
sha256 = "{b}"

[[factory.files]]
component = "display"
member = "assets/packages/display_2.3_aarch64.ipk"
version = "2.3"
size = 3000
sha256 = "{c}"
"#,
        a = "a".repeat(64),
        b = "b".repeat(64),
        c = "c".repeat(64),
    )
}

fn reference(label: &str) -> FactoryReference {
    FactoryReference::parse_toml(&reference_toml(label)).unwrap()
}

#[test]
fn parses_the_reference_file() {
    let f = reference("Factory software 2.3");
    assert_eq!(f.label, "Factory software 2.3");
    assert_eq!(f.container_size, 1_000_000);
    assert_eq!(f.files.len(), 3);
    let bridge = f.file(Component::Bridge).unwrap();
    assert_eq!(bridge.member, "assets/packages/bridge_2.3_aarch64.ipk");
    assert_eq!(bridge.size, 2000);
    assert!(f.file(Component::Host).is_none());
}

#[test]
fn validation_table() {
    let base = reference_toml("Factory software 2.3");
    let edit = |from: &str, to: &str| {
        assert!(base.contains(from), "fixture lacks {from:?}");
        base.replacen(from, to, 1)
    };
    let bridge_block = format!(
        "[[factory.files]]\ncomponent = \"bridge\"\nmember = \"assets/packages/bridge_2.3_aarch64.ipk\"\nversion = \"2.3\"\nsize = 2000\nsha256 = \"{}\"\n\n",
        "b".repeat(64)
    );
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "plain http",
            edit("https://downloads", "http://downloads"),
            "not an https URL",
        ),
        (
            "no host",
            edit(
                "https://downloads.example.com/keyboard/2.3/package.zip",
                "https:///package.zip",
            ),
            "not an https URL",
        ),
        (
            "empty label",
            edit("label = \"Factory software 2.3\"", "label = \" \""),
            "label is empty",
        ),
        (
            "zero container size",
            edit("container_size = 1000000", "container_size = 0"),
            "container_size must be greater than zero",
        ),
        (
            "zero file size",
            edit("size = 1000\n", "size = 0\n"),
            "firmware: size must be greater than zero",
        ),
        (
            "file larger than container",
            edit("size = 1000\n", "size = 2000000\n"),
            "firmware: size is larger than the container",
        ),
        (
            "short sha256",
            edit(&"a".repeat(64), "abc"),
            "firmware: sha256 must be 64 lowercase hex digits",
        ),
        (
            "uppercase sha256",
            edit(&"a".repeat(64), &"A".repeat(64)),
            "firmware: sha256 must be 64 lowercase hex digits",
        ),
        (
            "non-hex sha256",
            edit(&"a".repeat(64), &"g".repeat(64)),
            "firmware: sha256 must be 64 lowercase hex digits",
        ),
        (
            "duplicate component",
            edit("component = \"bridge\"", "component = \"firmware\""),
            "firmware is listed more than once",
        ),
        (
            "missing component",
            edit(&bridge_block, ""),
            "bridge is missing",
        ),
        (
            "host component",
            edit("component = \"display\"", "component = \"host\""),
            "host is not a factory component",
        ),
        (
            "unknown component",
            edit("component = \"display\"", "component = \"screen\""),
            "unknown variant `screen`",
        ),
        (
            "empty member",
            edit(
                "member = \"assets/firmware/scanner_2.3.uf2\"",
                "member = \"\"",
            ),
            "firmware: member is empty",
        ),
        (
            "empty version",
            edit("version = \"2.3\"", "version = \"\""),
            "firmware: version is empty",
        ),
        (
            "unknown field",
            edit(
                "container_size = 1000000",
                "container_size = 1000000\nmirror = \"x\"",
            ),
            "unknown field",
        ),
        (
            "no factory table",
            base.replace("[factory]", "[other]")
                .replace("factory.files", "other.files"),
            "factory",
        ),
    ];
    let mut failures = Vec::new();
    for (name, text, want) in cases {
        match FactoryReference::parse_toml(&text) {
            Ok(_) => failures.push(format!("{name}: accepted")),
            Err(e) if e.to_string().contains(want) => {}
            Err(e) => failures.push(format!("{name}: want {want:?} in {e}")),
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

fn index(sequence: u64, signer: &GeneratedKey, factory: Option<FactoryReference>) -> Index {
    let mut r = common::release(Component::Bridge, "1.0.0", common::br_set());
    r.artifacts[0].signatures = vec![signer.secret.sign_bytes(b"x", "artifact").unwrap()];
    Index {
        schema: 1,
        sequence,
        issued: NOW - time::Duration::days(1),
        expires: NOW + time::Duration::days(30),
        key_id: signer.secret.id(),
        releases: BTreeMap::from([(Channel::Stable, vec![r])]),
        key_statements: Vec::new(),
        factory,
        press_refinement_source: None,
    }
}

fn signed(index: &Index, signer: &GeneratedKey) -> (Vec<u8>, String) {
    let json = index.to_json().into_bytes();
    let sig = signer.secret.sign_bytes(&json, "index").unwrap();
    (json, sig)
}

fn anchors(k: &GeneratedKey) -> TrustAnchors {
    TrustAnchors {
        root_keys: Vec::new(),
        release_keys: vec![k.secret.public_key().clone()],
    }
}

#[test]
fn index_round_trips_the_reference_and_omits_it_when_absent() {
    let k = generate().unwrap();
    let with = index(1, &k, Some(reference("Factory software 2.3")));
    let parsed = Index::parse(with.to_json().as_bytes()).unwrap();
    assert_eq!(parsed, with);
    let json: serde_json::Value = serde_json::from_str(&with.to_json()).unwrap();
    assert_eq!(json["factory"]["files"][1]["component"], "bridge");
    assert_eq!(json["factory"]["container_size"], 1_000_000);

    let without = index(1, &k, None);
    assert!(!without.to_json().contains("factory"));
    assert_eq!(
        Index::parse(without.to_json().as_bytes()).unwrap().factory,
        None
    );
}

#[test]
fn the_index_signature_covers_the_reference() {
    let k = generate().unwrap();
    let (json, sig) = signed(&index(1, &k, Some(reference("Factory software 2.3"))), &k);
    let v = verify_index(
        &json,
        &sig,
        &anchors(&k),
        &mut MemoryTrustStore::default(),
        NOW,
    )
    .unwrap();
    let f = v.factory().unwrap();
    assert_eq!(f.file(Component::Firmware).unwrap().sha256, "a".repeat(64));
    assert_eq!(Catalog::from_index(&v).factory(), Some(f));

    let text = String::from_utf8(json).unwrap();
    let tampering = [
        (
            "pinned hash",
            text.replacen(&"a".repeat(64), &"d".repeat(64), 1),
        ),
        (
            "container url",
            text.replace("downloads.example.com", "elsewhere.example.com"),
        ),
        (
            "file size",
            text.replacen("\"size\": 1000,", "\"size\": 1001,", 1),
        ),
    ];
    for (name, tampered) in tampering {
        assert_ne!(tampered, text, "{name}: fixture did not change");
        assert!(
            matches!(
                verify_index(
                    tampered.as_bytes(),
                    &sig,
                    &anchors(&k),
                    &mut MemoryTrustStore::default(),
                    NOW
                ),
                Err(IndexError::BadSignature(_))
            ),
            "{name}"
        );
        assert!(
            matches!(
                verify_bundled_index(
                    tampered.as_bytes(),
                    &sig,
                    &anchors(&k),
                    &TrustState::default()
                ),
                Err(IndexError::BadSignature(_))
            ),
            "{name} (bundled)"
        );
    }
}

#[test]
fn an_invalid_reference_makes_the_index_malformed() {
    let k = generate().unwrap();
    let mut bad = reference("Factory software 2.3");
    bad.container_url = "http://downloads.example.com/package.zip".into();
    let (json, sig) = signed(&index(1, &k, Some(bad)), &k);
    assert!(matches!(
        verify_index(
            &json,
            &sig,
            &anchors(&k),
            &mut MemoryTrustStore::default(),
            NOW
        ),
        Err(IndexError::Malformed(_))
    ));
}

#[test]
fn catalog_keeps_the_reference_from_the_newest_index_in_any_order() {
    let k = generate().unwrap();
    let a = anchors(&k);
    let verify = |i: &Index| {
        let (json, sig) = signed(i, &k);
        verify_bundled_index(&json, &sig, &a, &TrustState::default()).unwrap()
    };
    let older = verify(&index(2, &k, Some(reference("Factory software 2.2"))));
    let newer = verify(&index(3, &k, Some(reference("Factory software 2.3"))));
    let newest_without = verify(&index(4, &k, None));

    let orders = [
        [&older, &newer, &newest_without],
        [&newest_without, &newer, &older],
        [&newer, &older, &newest_without],
        [&older, &newest_without, &newer],
    ];
    for order in orders {
        let mut cat = Catalog::default();
        for i in order {
            cat.add_index(i).unwrap();
        }
        assert_eq!(cat.factory().unwrap().label, "Factory software 2.3");
    }

    // The same index twice is fine; two different references at one sequence are not.
    let mut cat = Catalog::from_index(&newer);
    cat.add_index(&newer).unwrap();
    let rival = verify(&index(3, &k, Some(reference("Factory software 9"))));
    assert!(cat.add_index(&rival).is_err());
    let mut cat = Catalog::from_index(&rival);
    assert!(cat.add_index(&newer).is_err());

    assert_eq!(Catalog::from_index(&newest_without).factory(), None);
}

/// The repository's own reference file, read as a fixture.
const REPO_REFERENCE: &str = include_str!("../../../factory-reference.toml");

#[test]
fn the_repository_reference_is_valid_and_embeds_in_a_signed_index() {
    let file = ReferenceFile::parse_toml(REPO_REFERENCE).unwrap();
    let f = file.factory.clone();
    for c in Component::DEVICE {
        let file = f.file(c).unwrap();
        assert!(file.size > 0 && file.size <= f.container_size, "{c}");
    }
    let refinement = file
        .press_refinement_source
        .expect("the repository records a press-refinement source");
    assert_eq!(refinement.payload_length, 1456);
    assert!(matches!(
        &refinement.sources[0],
        SourceLocation::Container {
            size: 168_502_476,
            ..
        }
    ));
    let k = generate().unwrap();
    let mut idx = index(1, &k, Some(f.clone()));
    idx.press_refinement_source = Some(refinement.clone());
    let (json, sig) = signed(&idx, &k);
    let v = verify_index(
        &json,
        &sig,
        &anchors(&k),
        &mut MemoryTrustStore::default(),
        NOW,
    )
    .unwrap();
    assert_eq!(v.factory(), Some(&f));
    assert_eq!(v.press_refinement_source(), Some(&refinement));
    assert_eq!(
        Catalog::from_index(&v).press_refinement_source(),
        Some(&refinement)
    );
}

fn source_toml() -> String {
    format!(
        r#"
[press_refinement_source]
description = "Software 2.4"
sources = [
    {{ kind = "container", url = "https://downloads.example.com/keyboard/2.4/package.zip", size = 2000000, sha256 = "{e}" }},
]
member = "assets/firmware/scanner_2.4.uf2"
member_size = 4096
member_sha256 = "{a}"
image_base = 0x10000000
image_size = 2048
image_sha256 = "{b}"
payload_ranges = [{{ offset = 0x100, length = 12 }}, {{ offset = 0x10, length = 4 }}]
payload_length = 16
payload_sha256 = "{c}"
"#,
        a = "a".repeat(64),
        b = "b".repeat(64),
        c = "c".repeat(64),
        e = "e".repeat(64),
    )
}

fn refinement_source() -> PressRefinementSource {
    let text = reference_toml("Factory software 2.3") + &source_toml();
    ReferenceFile::parse_toml(&text)
        .unwrap()
        .press_refinement_source
        .unwrap()
}

#[test]
fn the_press_refinement_source_is_optional_strict_and_checked() {
    let without = ReferenceFile::parse_toml(&reference_toml("Factory software 2.3")).unwrap();
    assert_eq!(without.press_refinement_source, None);
    let d = refinement_source();
    assert_eq!(d.payload_ranges[0].offset, 0x100);

    let base = reference_toml("Factory software 2.3") + &source_toml();
    let edit = |from: &str, to: &str| {
        assert!(base.contains(from), "fixture lacks {from:?}");
        base.replacen(from, to, 1)
    };
    let cases = [
        (
            "unknown source field",
            edit("member_size = 4096", "member_size = 4096\nmirror = 1"),
            "unknown field",
        ),
        (
            "unknown source field",
            edit("size = 2000000,", "size = 2000000, region = \"eu\","),
            "unknown field",
        ),
        (
            "unknown table",
            base.clone() + "\n[extra]\nx = 1\n",
            "unknown field",
        ),
        (
            "short ranges",
            edit("length = 4 }", "length = 3 }"),
            "payload_length is 16",
        ),
        (
            "range outside",
            edit("offset = 0x100", "offset = 0x7ff"),
            "outside the image",
        ),
        (
            "http source",
            edit("url = \"https://", "url = \"http://"),
            "not an https URL",
        ),
        (
            "bad sha",
            edit(
                &format!("image_sha256 = \"{}\"", "b".repeat(64)),
                "image_sha256 = \"B\"",
            ),
            "image_sha256",
        ),
        (
            "no sources",
            base.replace(
                &base[base.find("sources = [").unwrap()
                    ..base.find("member = \"assets/firmware/scanner_2.4").unwrap()],
                "sources = []\n",
            ),
            "sources is empty",
        ),
        (
            "unknown kind",
            edit("kind = \"container\"", "kind = \"carrierPigeon\""),
            "doesn't know",
        ),
    ];
    let mut failures = Vec::new();
    for (name, text, want) in cases {
        match ReferenceFile::parse_toml(&text) {
            Ok(_) => failures.push(format!("{name}: accepted")),
            Err(e) if e.to_string().contains(want) => {}
            Err(e) => failures.push(format!("{name}: want {want:?} in {e}")),
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn hosts_keep_reading_an_index_whose_press_refinement_source_lists_a_source_kind_they_dont_know() {
    let k = generate().unwrap();
    let mut idx = index(1, &k, None);
    idx.press_refinement_source = Some(refinement_source());
    let mut json: serde_json::Value = serde_json::from_str(&idx.to_json()).unwrap();
    assert_eq!(
        json["press_refinement_source"]["sources"][0]["kind"],
        "container"
    );
    json["press_refinement_source"]["sources"]
        .as_array_mut()
        .unwrap()
        .insert(0, serde_json::json!({"kind": "keyboardFlash", "slot": 2}));
    let parsed = Index::parse(json.to_string().as_bytes()).unwrap();
    let d = parsed.press_refinement_source.unwrap();
    assert_eq!(d.sources[0], SourceLocation::Unrecognized);
    assert!(matches!(d.sources[1], SourceLocation::Container { .. }));
    assert!(d.check_publishable().is_err());

    // An index without a press-refinement source doesn't mention one.
    assert!(!index(1, &k, None).to_json().contains("press_refinement"));
}

#[test]
fn the_newest_index_with_a_press_refinement_source_supplies_it() {
    let k = generate().unwrap();
    let verify = |i: &Index| {
        let (json, sig) = signed(i, &k);
        verify_bundled_index(&json, &sig, &anchors(&k), &TrustState::default()).unwrap()
    };
    let mut older = index(2, &k, None);
    older.press_refinement_source = Some(refinement_source());
    let mut newer = index(3, &k, None);
    let mut moved = refinement_source();
    moved.sources = vec![SourceLocation::Container {
        url: "https://mirror.example.com/package.zip".into(),
        size: 2_000_000,
        sha256: "e".repeat(64),
    }];
    newer.press_refinement_source = Some(moved.clone());
    let newest_without = index(4, &k, None);
    let (older, newer, newest_without) = (verify(&older), verify(&newer), verify(&newest_without));
    for order in [
        [&older, &newer, &newest_without],
        [&newest_without, &newer, &older],
    ] {
        let mut cat = Catalog::default();
        for i in order {
            cat.add_index(i).unwrap();
        }
        assert_eq!(cat.press_refinement_source(), Some(&moved));
    }
}
