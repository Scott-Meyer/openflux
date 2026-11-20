#![cfg(feature = "signing")]

mod common;

use std::collections::BTreeMap;
use std::io::Cursor;

use common::*;
use openflux_update::manifest::{Channel, Index, KeyStatement, SignedStatement};
use openflux_update::signing::{generate, GeneratedKey};
use openflux_update::trust::{
    verify_artifact, verify_index, ArtifactError, IndexError, MemoryTrustStore, PublicKey,
    TrustAnchors, TrustState, TrustStore,
};
use openflux_update::version::Component::Bridge;
use time::macros::datetime;
use time::OffsetDateTime;

const NOW: OffsetDateTime = datetime!(2030-06-01 00:00 UTC);

struct Keys {
    root: GeneratedKey,
    release: GeneratedKey,
    next: GeneratedKey,
    stranger: GeneratedKey,
}

fn keys() -> Keys {
    Keys {
        root: generate().unwrap(),
        release: generate().unwrap(),
        next: generate().unwrap(),
        stranger: generate().unwrap(),
    }
}

fn anchors(k: &Keys) -> TrustAnchors {
    TrustAnchors {
        root_keys: vec![k.root.secret.public_key().clone()],
        release_keys: vec![k.release.secret.public_key().clone()],
    }
}

fn statement(signer: &GeneratedKey, s: &KeyStatement) -> SignedStatement {
    let text = serde_json::to_string(s).unwrap();
    SignedStatement {
        signature: signer
            .secret
            .sign_bytes(text.as_bytes(), "key statement")
            .unwrap(),
        statement: text,
    }
}

fn trust(signer: &GeneratedKey, key: &GeneratedKey) -> SignedStatement {
    let pk = key.secret.public_key();
    statement(
        signer,
        &KeyStatement::Trust {
            key_id: pk.id(),
            public_key: pk.to_base64(),
            issued: NOW,
        },
    )
}

fn revoke(signer: &GeneratedKey, key: &GeneratedKey) -> SignedStatement {
    statement(
        signer,
        &KeyStatement::Revoke {
            key_id: key.secret.id(),
            issued: NOW,
            reason: String::new(),
        },
    )
}

fn index(sequence: u64, signer: &GeneratedKey, statements: Vec<SignedStatement>) -> Index {
    let mut r = release(Bridge, "1.1.0", br_set());
    for a in &mut r.artifacts {
        a.signatures = vec![signer.secret.sign_bytes(b"x", "artifact").unwrap()];
    }
    Index {
        schema: 1,
        sequence,
        issued: NOW - time::Duration::days(1),
        expires: NOW + time::Duration::days(30),
        key_id: signer.secret.id(),
        releases: BTreeMap::from([(Channel::Stable, vec![r])]),
        key_statements: statements,
        factory: None,
        press_refinement_source: None,
    }
}

fn signed(index: &Index, signer: &GeneratedKey) -> (Vec<u8>, String) {
    let json = index.to_json().into_bytes();
    let sig = signer.secret.sign_bytes(&json, "index").unwrap();
    (json, sig)
}

fn accept(
    json: &[u8],
    sig: &str,
    a: &TrustAnchors,
    store: &mut MemoryTrustStore,
) -> Result<u64, IndexError> {
    verify_index(json, sig, a, store, NOW).map(|v| v.index().sequence)
}

#[test]
fn index_acceptance_table() {
    let k = keys();
    let a = anchors(&k);
    type Setup = Box<dyn Fn(&Keys) -> (Vec<u8>, String, TrustState)>;
    let cases: Vec<(&str, Setup, Result<u64, &str>)> = vec![
        (
            "valid",
            Box::new(|k| {
                let (j, s) = signed(&index(3, &k.release, vec![]), &k.release);
                (j, s, TrustState::default())
            }),
            Ok(3),
        ),
        (
            "same sequence again is accepted",
            Box::new(|k| {
                let (j, s) = signed(&index(3, &k.release, vec![]), &k.release);
                (
                    j,
                    s,
                    TrustState {
                        highest_sequence: 3,
                        ..Default::default()
                    },
                )
            }),
            Ok(3),
        ),
        (
            "replayed older index",
            Box::new(|k| {
                let (j, s) = signed(&index(4, &k.release, vec![]), &k.release);
                (
                    j,
                    s,
                    TrustState {
                        highest_sequence: 5,
                        ..Default::default()
                    },
                )
            }),
            Err("Replayed"),
        ),
        (
            "expired",
            Box::new(|k| {
                let mut i = index(3, &k.release, vec![]);
                i.issued = NOW - time::Duration::days(40);
                i.expires = NOW - time::Duration::seconds(1);
                let (j, s) = signed(&i, &k.release);
                (j, s, TrustState::default())
            }),
            Err("Expired"),
        ),
        (
            "signed by an unknown key",
            Box::new(|k| {
                let (j, s) = signed(&index(3, &k.stranger, vec![]), &k.stranger);
                (j, s, TrustState::default())
            }),
            Err("UntrustedKey"),
        ),
        (
            "tampered after signing",
            Box::new(|k| {
                let (j, s) = signed(&index(3, &k.release, vec![]), &k.release);
                let j = String::from_utf8(j)
                    .unwrap()
                    .replace("\"sequence\": 3", "\"sequence\": 9")
                    .into_bytes();
                (j, s, TrustState::default())
            }),
            Err("BadSignature"),
        ),
        (
            "declares a different key than signed it",
            Box::new(|k| {
                let mut i = index(3, &k.release, vec![trust(&k.root, &k.next)]);
                i.key_id = k.next.secret.id();
                let (j, s) = signed(&i, &k.release);
                (j, s, TrustState::default())
            }),
            Err("KeyIdMismatch"),
        ),
        (
            "rotated key trusted by a root statement",
            Box::new(|k| {
                let (j, s) = signed(&index(3, &k.next, vec![trust(&k.root, &k.next)]), &k.next);
                (j, s, TrustState::default())
            }),
            Ok(3),
        ),
        (
            "rotated key without a statement",
            Box::new(|k| {
                let (j, s) = signed(&index(3, &k.next, vec![]), &k.next);
                (j, s, TrustState::default())
            }),
            Err("UntrustedKey"),
        ),
        (
            "rotation statement signed by a release key, not root",
            Box::new(|k| {
                let (j, s) = signed(
                    &index(3, &k.next, vec![trust(&k.release, &k.next)]),
                    &k.next,
                );
                (j, s, TrustState::default())
            }),
            Err("InvalidStatement"),
        ),
        (
            "revoked key in the index",
            Box::new(|k| {
                let (j, s) = signed(
                    &index(3, &k.release, vec![revoke(&k.root, &k.release)]),
                    &k.release,
                );
                (j, s, TrustState::default())
            }),
            Err("RevokedKey"),
        ),
        (
            "revocation remembered from an earlier index",
            Box::new(|k| {
                let (j, s) = signed(&index(3, &k.release, vec![]), &k.release);
                (
                    j,
                    s,
                    TrustState {
                        highest_sequence: 2,
                        revoked: [k.release.secret.id()].into(),
                    },
                )
            }),
            Err("RevokedKey"),
        ),
        (
            "revocation overrides trust",
            Box::new(|k| {
                let (j, s) = signed(
                    &index(
                        3,
                        &k.next,
                        vec![trust(&k.root, &k.next), revoke(&k.root, &k.next)],
                    ),
                    &k.next,
                );
                (j, s, TrustState::default())
            }),
            Err("RevokedKey"),
        ),
        (
            "trust statement whose key id does not match its key",
            Box::new(|k| {
                let st = statement(
                    &k.root,
                    &KeyStatement::Trust {
                        key_id: k.stranger.secret.id(),
                        public_key: k.next.secret.public_key().to_base64(),
                        issued: NOW,
                    },
                );
                let (j, s) = signed(&index(3, &k.release, vec![st]), &k.release);
                (j, s, TrustState::default())
            }),
            Err("InvalidStatement"),
        ),
        (
            "garbage signature",
            Box::new(|k| {
                let (j, _) = signed(&index(3, &k.release, vec![]), &k.release);
                (j, "not a signature".into(), TrustState::default())
            }),
            Err("BadSignatureFormat"),
        ),
    ];
    let mut failures = Vec::new();
    for (name, setup, want) in cases {
        let (json, sig, state) = setup(&k);
        let mut store = MemoryTrustStore { state };
        let got = accept(&json, &sig, &a, &mut store);
        let ok = match (&got, &want) {
            (Ok(g), Ok(w)) => g == w,
            (Err(e), Err(w)) => format!("{e:?}").starts_with(w),
            _ => false,
        };
        if !ok {
            failures.push(format!("{name}: want {want:?}, got {got:?}"));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn acceptance_persists_sequence_and_revocations() {
    let k = keys();
    let a = anchors(&k);
    let mut store = MemoryTrustStore::default();
    // Rotate: next is trusted, the original release key is revoked.
    let i = index(
        7,
        &k.next,
        vec![trust(&k.root, &k.next), revoke(&k.root, &k.release)],
    );
    let (j, s) = signed(&i, &k.next);
    let v = verify_index(&j, &s, &a, &mut store, NOW).unwrap();
    assert_eq!(v.signed_by(), k.next.secret.id());
    assert_eq!(store.state.highest_sequence, 7);
    assert!(store.state.revoked.contains(&k.release.secret.id()));
    assert!(v.trusted_keys().get(k.next.secret.id()).is_some());
    assert!(v.trusted_keys().get(k.release.secret.id()).is_none());

    // A later index signed with the revoked key is refused even without the statement.
    let (j, s) = signed(&index(8, &k.release, vec![]), &k.release);
    assert!(matches!(
        verify_index(&j, &s, &a, &mut store, NOW),
        Err(IndexError::RevokedKey { .. })
    ));
    // And an older index cannot be replayed.
    let (j, s) = signed(&index(6, &k.next, vec![trust(&k.root, &k.next)]), &k.next);
    assert!(matches!(
        verify_index(&j, &s, &a, &mut store, NOW),
        Err(IndexError::Replayed {
            sequence: 6,
            highest: 7
        })
    ));
}

struct FailingStore;
impl TrustStore for FailingStore {
    type Error = String;
    fn load(&self) -> Result<TrustState, String> {
        Ok(TrustState::default())
    }
    fn save(&mut self, _: &TrustState) -> Result<(), String> {
        Err("disk full".into())
    }
}

#[test]
fn index_is_not_accepted_unless_its_state_is_saved() {
    let k = keys();
    let (j, s) = signed(&index(1, &k.release, vec![]), &k.release);
    let err = verify_index(&j, &s, &anchors(&k), &mut FailingStore, NOW).unwrap_err();
    assert_eq!(err, IndexError::Store("disk full".into()));
}

#[test]
fn artifact_verification_table() {
    let k = keys();
    let data = b"openflux bridge package".to_vec();
    let (sha, size) = openflux_update::trust::sha256_hex(&data[..]).unwrap();
    let good = || {
        let mut r = release(Bridge, "1.1.0", br_set());
        let a = &mut r.artifacts[0];
        a.sha256 = sha.clone();
        a.size = size;
        a.signatures = vec![k.release.secret.sign_bytes(&data, "artifact").unwrap()];
        r.artifacts[0].clone()
    };
    let trusted = anchors(&k).bundled_keys(&TrustState::default());
    type ArtifactCase = (
        &'static str,
        openflux_update::Artifact,
        Vec<u8>,
        Result<(), &'static str>,
    );
    let cases: Vec<ArtifactCase> = vec![
        ("good", good(), data.clone(), Ok(())),
        ("truncated", good(), data[..5].to_vec(), Err("Size")),
        (
            "same size, different bytes",
            good(),
            data.iter().map(|b| b ^ 1).collect(),
            Err("Hash"),
        ),
        (
            "unsigned",
            {
                let mut a = good();
                a.signatures.clear();
                a
            },
            data.clone(),
            Err("Unsigned"),
        ),
        (
            "signed by an untrusted key",
            {
                let mut a = good();
                a.signatures = vec![k.stranger.secret.sign_bytes(&data, "x").unwrap()];
                a
            },
            data.clone(),
            Err("UntrustedKey"),
        ),
        (
            "signature over other bytes",
            {
                let mut a = good();
                a.signatures = vec![k.release.secret.sign_bytes(b"other", "x").unwrap()];
                a
            },
            data.clone(),
            Err("Signature"),
        ),
        (
            "a trusted signature among untrusted ones",
            {
                let mut a = good();
                let mut sigs = vec![k.stranger.secret.sign_bytes(&data, "x").unwrap()];
                sigs.extend(a.signatures.clone());
                a.signatures = sigs;
                a
            },
            data.clone(),
            Ok(()),
        ),
        (
            "an untrusted signature is preferred to none, a bad trusted one is reported",
            {
                let mut a = good();
                a.signatures = vec![
                    k.stranger.secret.sign_bytes(&data, "x").unwrap(),
                    k.release.secret.sign_bytes(b"other", "x").unwrap(),
                ];
                a
            },
            data.clone(),
            Err("Signature"),
        ),
    ];
    let mut failures = Vec::new();
    for (name, artifact, bytes, want) in cases {
        let got = verify_artifact(Cursor::new(bytes), &artifact, &trusted);
        let ok = match (&got, &want) {
            (Ok(v), Ok(())) => v.signed_by == k.release.secret.id(),
            (Err(e), Err(w)) => format!("{e:?}").starts_with(w),
            _ => false,
        };
        if !ok {
            failures.push(format!(
                "{name}: want {want:?}, got {:?}",
                got.map_err(|e: ArtifactError| e.to_string())
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn bundled_keys_honor_revocations() {
    let k = keys();
    let state = TrustState {
        highest_sequence: 0,
        revoked: [k.release.secret.id()].into(),
    };
    assert_eq!(anchors(&k).bundled_keys(&state).ids().count(), 0);
}

#[test]
fn public_keys_parse_from_file_or_line() {
    let k = generate().unwrap();
    let from_file = PublicKey::parse(&k.public_text).unwrap();
    let line = k.public_text.lines().nth(1).unwrap();
    assert_eq!(PublicKey::parse(line).unwrap().id(), from_file.id());
    assert_eq!(from_file.id().to_string().len(), 16);
    assert!(k.public_text.contains(&from_file.id().to_string()));
    assert!(PublicKey::parse("untrusted comment: x\nnot base64").is_err());
}

#[test]
fn key_ids_round_trip_in_minisign_notation() {
    let k = generate().unwrap();
    let id = k.secret.id();
    let s = id.to_string();
    assert_eq!(s.parse::<openflux_update::KeyId>().unwrap(), id);
    assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{s}\""));
    assert!("XYZ".parse::<openflux_update::KeyId>().is_err());
}

#[test]
fn bundled_index_skips_freshness_but_not_trust() {
    let k = keys();
    let a = anchors(&k);
    let mut old = index(2, &k.release, vec![]);
    old.issued = NOW - time::Duration::days(400);
    old.expires = NOW - time::Duration::days(370);
    let (j, s) = signed(&old, &k.release);
    let state = TrustState {
        highest_sequence: 9,
        ..Default::default()
    };

    // Expired and older than what the host has seen: refused online...
    let mut store = MemoryTrustStore {
        state: state.clone(),
    };
    assert!(matches!(
        verify_index(&j, &s, &a, &mut store, NOW),
        Err(IndexError::Expired { .. })
    ));
    // ...accepted as the installer's bundle, without touching the stored state.
    let v = openflux_update::verify_bundled_index(&j, &s, &a, &state).unwrap();
    assert!(v.is_bundled());
    assert_eq!(v.index().sequence, 2);

    // Signature and revocation rules apply.
    let revoked = TrustState {
        highest_sequence: 9,
        revoked: [k.release.secret.id()].into(),
    };
    assert!(matches!(
        openflux_update::verify_bundled_index(&j, &s, &a, &revoked),
        Err(IndexError::RevokedKey { .. })
    ));
    let tampered = String::from_utf8(j.clone())
        .unwrap()
        .replace("\"sequence\": 2", "\"sequence\": 3");
    assert!(matches!(
        openflux_update::verify_bundled_index(tampered.as_bytes(), &s, &a, &state),
        Err(IndexError::BadSignature(_))
    ));
}

#[test]
fn withdrawal_wins_when_bundled_and_online_indexes_merge() {
    use openflux_update::engine::Catalog;
    let k = keys();
    let a = anchors(&k);
    let bundled = index(2, &k.release, vec![]);
    let mut online = bundled.clone();
    online.sequence = 3;
    online.releases.get_mut(&Channel::Stable).unwrap()[0].withdrawn = true;

    let (bj, bs) = signed(&bundled, &k.release);
    let (oj, os) = signed(&online, &k.release);
    let b = openflux_update::verify_bundled_index(&bj, &bs, &a, &TrustState::default()).unwrap();
    let o = verify_index(&oj, &os, &a, &mut MemoryTrustStore::default(), NOW).unwrap();

    let mut first = Catalog::from_index(&b);
    first.add_index(&o).unwrap();
    let mut second = Catalog::from_index(&o);
    second.add_index(&b).unwrap();
    for (name, cat) in [
        ("bundled then online", &first),
        ("online then bundled", &second),
    ] {
        assert_eq!(cat.releases().len(), 1, "{name}");
        assert!(cat.releases()[0].withdrawn, "{name}");
    }
}

#[test]
fn catalog_merges_reject_conflicting_metadata() {
    use openflux_update::engine::Catalog;
    let base = release(Bridge, "1.1.0", br_set());
    let mut withdrawn = base.clone();
    withdrawn.withdrawn = true;
    for (name, a, b) in [
        ("withdrawn first", withdrawn.clone(), base.clone()),
        ("withdrawn second", base.clone(), withdrawn.clone()),
    ] {
        let cat = Catalog::from_trusted_releases([a, b]).unwrap();
        assert!(cat.releases()[0].withdrawn, "{name}");
    }
    // Signatures by different keys over the same bytes combine, in either order.
    let k = keys();
    let mut by_a = base.clone();
    by_a.artifacts[0].signatures = vec![k.release.secret.sign_bytes(b"x", "a").unwrap()];
    let mut by_b = base.clone();
    by_b.artifacts[0].signatures = vec![k.next.secret.sign_bytes(b"x", "b").unwrap()];
    let ab = Catalog::from_trusted_releases([by_a.clone(), by_b.clone()]).unwrap();
    let ba = Catalog::from_trusted_releases([by_b.clone(), by_a.clone()]).unwrap();
    assert_eq!(ab.releases(), ba.releases());
    let mut signers = ab.releases()[0].artifacts[0].signed_by();
    signers.sort();
    let mut want = vec![k.release.secret.id(), k.next.secret.id()];
    want.sort();
    assert_eq!(signers, want);

    // Two signatures by the same key keep one, the same one in either order.
    let mut by_a2 = base.clone();
    by_a2.artifacts[0].signatures = vec![k.release.secret.sign_bytes(b"x", "a again").unwrap()];
    let one = Catalog::from_trusted_releases([by_a.clone(), by_a2.clone()]).unwrap();
    let two = Catalog::from_trusted_releases([by_a2.clone(), by_a.clone()]).unwrap();
    assert_eq!(one.releases(), two.releases());
    assert_eq!(one.releases()[0].artifacts[0].signatures.len(), 1);

    // Withdrawal and signatures together, in all orders.
    let mut withdrawn_b = by_b.clone();
    withdrawn_b.withdrawn = true;
    let orders = [
        vec![by_a.clone(), withdrawn_b.clone(), by_a2.clone()],
        vec![withdrawn_b.clone(), by_a2.clone(), by_a.clone()],
        vec![by_a2.clone(), by_a.clone(), withdrawn_b.clone()],
    ];
    let results: Vec<_> = orders
        .iter()
        .map(|o| {
            Catalog::from_trusted_releases(o.clone())
                .unwrap()
                .releases()
                .to_vec()
        })
        .collect();
    assert!(results.windows(2).all(|w| w[0] == w[1]));
    assert!(results[0][0].withdrawn);
    assert_eq!(results[0][0].artifacts[0].signatures.len(), 2);

    // A signature that does not parse is an error, not a silent drop.
    let mut garbage = base.clone();
    garbage.artifacts[0].signatures = vec!["not a signature".into()];
    assert!(Catalog::from_trusted_releases([by_a.clone(), garbage]).is_err());

    type Change = Box<dyn Fn(&mut openflux_update::Release)>;
    let conflicts: Vec<(&str, Change)> = vec![
        ("notes url", Box::new(|r| r.notes_url.push('x'))),
        (
            "artifact hash",
            Box::new(|r| r.artifacts[0].sha256 = "1".repeat(64)),
        ),
        ("interfaces", Box::new(|r| r.interfaces.requires.clear())),
        (
            "release time",
            Box::new(|r| r.released += time::Duration::seconds(1)),
        ),
    ];
    for (name, change) in conflicts {
        let mut other = base.clone();
        change(&mut other);
        let err = Catalog::from_trusted_releases([base.clone(), other.clone()]).unwrap_err();
        assert!(err.to_string().contains("immutable"), "{name}: {err}");
        let mut cat = Catalog::from_trusted_releases([other]).unwrap();
        assert!(cat.add_trusted([base.clone()]).is_err(), "{name} reversed");
    }
}
