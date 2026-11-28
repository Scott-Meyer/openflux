//! End-to-end runs of `keyglass-release` the way the release workflow uses it.
#![cfg(feature = "cli")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use keyglass_update::manifest::{Index, Release};
use keyglass_update::trust::{
    verify_bundled_index, verify_index, MemoryTrustStore, PublicKey, TrustAnchors, TrustState,
};

const BIN: &str = env!("CARGO_BIN_EXE_keyglass-release");
/// A frozen registry with every component at 1.0.0, so these scenarios don't
/// depend on the versions the repository is at.
const COMPAT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/compat.toml");

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "keyglass-release-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
    fn join(&self, s: &str) -> PathBuf {
        self.0.join(s)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut c = Command::new(BIN);
    c.args(args).env_remove("SOURCE_DATE_EPOCH");
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().unwrap()
}

fn ok(args: &[&str], env: &[(&str, &str)]) -> String {
    let out = run(args, env);
    assert!(
        out.status.success(),
        "{args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
}

fn fails(args: &[&str], env: &[(&str, &str)]) -> String {
    let out = run(args, env);
    assert!(!out.status.success(), "{args:?} unexpectedly succeeded");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Path as an argument. Leaked so argument lists can be built from temporaries;
/// each test process is short-lived.
fn s(p: &Path) -> &'static str {
    Box::leak(p.to_str().unwrap().to_owned().into_boxed_str())
}

/// Generate a key pair; returns (public key path, secret key text).
fn keygen(dir: &Dir, name: &str) -> (PathBuf, String) {
    let keys = dir.join(&format!("keys-{name}"));
    let out = ok(&["keygen", "--out-dir", s(&keys), "--name", name], &[]);
    assert!(
        out.contains("offline"),
        "keygen warns about offline storage"
    );
    let mut pubkey = None;
    let mut secret = None;
    for e in fs::read_dir(&keys).unwrap() {
        let p = e.unwrap().path();
        match p.extension().and_then(|e| e.to_str()) {
            Some("pub") => pubkey = Some(p),
            Some("key") => secret = Some(fs::read_to_string(&p).unwrap()),
            _ => {}
        }
    }
    (pubkey.unwrap(), secret.unwrap())
}

struct World {
    dir: Dir,
    release_pub: PathBuf,
    release_key: String,
    root_pub: PathBuf,
    root_key: String,
}

fn world(name: &str) -> World {
    let dir = Dir::new(name);
    let (release_pub, release_key) = keygen(&dir, "release");
    let (root_pub, root_key) = keygen(&dir, "root");
    fs::create_dir_all(dir.join("site")).unwrap();
    fs::create_dir_all(dir.join("releases")).unwrap();
    World {
        dir,
        release_pub,
        release_key,
        root_pub,
        root_key,
    }
}

impl World {
    fn env(&self) -> Vec<(&str, &str)> {
        vec![
            ("RELEASE_KEY", self.release_key.as_str()),
            ("ROOT_KEY", self.root_key.as_str()),
        ]
    }

    /// Build, sign and describe one release the way the release workflow does.
    fn publish(&self, component: &str, version: &str, channel: &str, compat: &Path) {
        let name = format!("keyglass-{component}-{version}.bin");
        let artifact = self.dir.join("site").join(&name);
        fs::write(&artifact, format!("{component} {version} payload")).unwrap();
        ok(
            &["sign", "--key-env", "RELEASE_KEY", s(&artifact)],
            &self.env(),
        );
        ok(
            &[
                "manifest",
                "--component",
                component,
                "--version",
                version,
                "--channel",
                channel,
                "--compat",
                s(compat),
                "--artifact",
                s(&artifact),
                "--base-url",
                ".",
                "--notes-url",
                &format!("https://example.invalid/{component}/{version}"),
                "-o",
                s(&self
                    .dir
                    .join("releases")
                    .join(format!("{component}-{version}.json"))),
            ],
            &[],
        );
    }

    fn index(&self, previous: &str, extra: &[&str]) -> PathBuf {
        let out = self.dir.join("site/index.json");
        let tmp = self.dir.join("index.next.json");
        let mut args = vec![
            "index",
            "--releases",
            s(&self.dir.0.join("releases")),
            "--previous",
            previous,
            "--expires-days",
            "30",
        ];
        args.extend_from_slice(extra);
        args.extend_from_slice(&["-o", s(&tmp)]);
        ok(&args, &[]);
        fs::rename(&tmp, &out).unwrap();
        ok(&["sign", "--key-env", "RELEASE_KEY", s(&out)], &self.env());
        out
    }

    fn anchors(&self) -> TrustAnchors {
        TrustAnchors {
            root_keys: vec![
                PublicKey::parse(&fs::read_to_string(&self.root_pub).unwrap()).unwrap(),
            ],
            release_keys: vec![
                PublicKey::parse(&fs::read_to_string(&self.release_pub).unwrap()).unwrap(),
            ],
        }
    }

    fn read_index(&self) -> Index {
        Index::parse(&fs::read(self.dir.join("site/index.json")).unwrap()).unwrap()
    }
}

/// compat.toml with every component at `version`.
fn compat_at(dir: &Dir, version: &str) -> PathBuf {
    let text = fs::read_to_string(COMPAT)
        .unwrap()
        .replace("version = \"1.0.0\"", &format!("version = \"{version}\""));
    let p = dir.join(&format!("compat-{version}.toml"));
    fs::write(&p, text).unwrap();
    p
}

#[test]
fn publish_verify_and_bundle() {
    let w = world("publish");
    let compat = PathBuf::from(COMPAT);
    for c in ["firmware", "bridge", "display"] {
        w.publish(c, "1.0.0", "stable", &compat);
    }
    let index = w.index("none", &["--pubkey", s(&w.release_pub)]);

    // The index and an artifact verify with the CLI and with the library.
    let out = ok(&["verify", "--pubkey", s(&w.release_pub), s(&index)], &[]);
    assert!(out.contains("good signature"));
    ok(
        &[
            "verify",
            "--pubkey",
            s(&w.release_pub),
            s(&w.dir.join("site/keyglass-bridge-1.0.0.bin")),
        ],
        &[],
    );
    let json = fs::read(&index).unwrap();
    let sig = fs::read_to_string(w.dir.join("site/index.json.sig")).unwrap();
    let v = verify_index(
        &json,
        &sig,
        &w.anchors(),
        &mut MemoryTrustStore::default(),
        time::OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert_eq!(v.index().sequence, 1);
    assert_eq!(v.index().all_releases().count(), 3);
    let bridge = v
        .index()
        .all_releases()
        .find(|r| r.label() == "bridge@1.0.0")
        .unwrap();
    assert!(bridge.interfaces.provides.contains_key("bridge-usb"));
    assert_eq!(bridge.artifacts[0].signatures.len(), 1);

    // A wrong key fails.
    let (other_pub, _) = keygen(&w.dir, "other");
    assert!(
        fails(&["verify", "--pubkey", s(&other_pub), s(&index)], &[])
            .contains("not among the given keys")
    );

    // Bundle the keyboard software for an installer.
    let bundle = w.dir.join("bundle");
    ok(
        &[
            "bundle",
            "--index",
            s(&index),
            "--pubkey",
            s(&w.release_pub),
            "--root-pubkey",
            s(&w.root_pub),
            "--channel",
            "stable",
            "--component",
            "firmware",
            "--component",
            "bridge",
            "--component",
            "display",
            "-o",
            s(&bundle),
        ],
        &[],
    );
    let mut names: Vec<String> = fs::read_dir(&bundle)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "index.json",
            "index.json.sig",
            "keyglass-bridge-1.0.0.bin",
            "keyglass-display-1.0.0.bin",
            "keyglass-firmware-1.0.0.bin"
        ]
    );
    let bj = fs::read(bundle.join("index.json")).unwrap();
    let bs = fs::read_to_string(bundle.join("index.json.sig")).unwrap();
    verify_bundled_index(&bj, &bs, &w.anchors(), &TrustState::default()).unwrap();

    // A tampered artifact is refused and not left behind.
    fs::write(
        w.dir.join("site/keyglass-display-1.0.0.bin"),
        "display 1.0.0 payloaX",
    )
    .unwrap();
    let bad = w.dir.join("bundle-bad");
    let err = fails(
        &[
            "bundle",
            "--index",
            s(&index),
            "--pubkey",
            s(&w.release_pub),
            "--root-pubkey",
            s(&w.root_pub),
            "--component",
            "display",
            "-o",
            s(&bad),
        ],
        &[],
    );
    assert!(err.contains("sha256"), "{err}");
    assert!(!bad.join("keyglass-display-1.0.0.bin").exists());
}

#[test]
fn bundle_needs_stable_releases() {
    let w = world("bundle-beta");
    let compat = compat_at(&w.dir, "1.1.0-beta.1");
    w.publish("bridge", "1.1.0-beta.1", "beta", &compat);
    let index = w.index("none", &["--pubkey", s(&w.release_pub)]);
    let base = [
        "bundle",
        "--index",
        s(&index),
        "--pubkey",
        s(&w.release_pub),
        "--root-pubkey",
        s(&w.root_pub),
        "--component",
        "bridge",
    ];

    let mut args = base.to_vec();
    args.extend_from_slice(&["-o", s(&w.dir.join("b1"))]);
    let err = fails(&args, &[]);
    assert!(
        err.contains("no stable bridge release published yet; release keyboard components first"),
        "{err}"
    );

    let mut args = base.to_vec();
    args.extend_from_slice(&["--allow-missing", "-o", s(&w.dir.join("b2"))]);
    ok(&args, &[]);
    assert!(w.dir.join("b2/index.json").exists());

    let mut args = base.to_vec();
    args.extend_from_slice(&["--channel", "beta", "-o", s(&w.dir.join("b3"))]);
    ok(&args, &[]);
    assert!(w.dir.join("b3/keyglass-bridge-1.1.0-beta.1.bin").exists());
}

#[test]
fn sequence_withdrawal_and_immutability() {
    let w = world("sequence");
    let compat = PathBuf::from(COMPAT);
    w.publish("bridge", "1.0.0", "stable", &compat);
    w.publish("display", "1.0.0", "stable", &compat);
    let index = w.index("none", &["--pubkey", s(&w.release_pub)]);
    let prev = w.dir.join("prev.json");

    // Each index is one more than the previous, and carries its releases.
    fs::copy(&index, &prev).unwrap();
    fs::remove_file(w.dir.join("releases/display-1.0.0.json")).unwrap();
    w.index(s(&prev), &[]);
    let i = w.read_index();
    assert_eq!(i.sequence, 2);
    assert_eq!(i.all_releases().count(), 2);

    // Withdrawn by flag, and it stays withdrawn when the release file is merged again.
    fs::copy(&index, &prev).unwrap();
    w.index(s(&prev), &["--withdraw", "bridge@1.0.0"]);
    fs::copy(&index, &prev).unwrap();
    w.index(s(&prev), &[]);
    let i = w.read_index();
    assert_eq!(i.sequence, 4);
    assert!(
        i.all_releases()
            .find(|r| r.label() == "bridge@1.0.0")
            .unwrap()
            .withdrawn
    );

    // Withdrawn in the incoming release file while the previous index is not.
    let f = w.dir.join("releases/display-1.0.0.json");
    let mut d: Release = i
        .all_releases()
        .find(|r| r.label() == "display@1.0.0")
        .unwrap()
        .clone();
    d.withdrawn = true;
    fs::write(&f, serde_json::to_string_pretty(&d).unwrap()).unwrap();
    fs::copy(&index, &prev).unwrap();
    w.index(s(&prev), &[]);
    assert!(
        w.read_index()
            .all_releases()
            .find(|r| r.label() == "display@1.0.0")
            .unwrap()
            .withdrawn
    );

    // Any other change to a published release is refused.
    d.withdrawn = false;
    d.notes_url.push_str("/changed");
    fs::write(&f, serde_json::to_string_pretty(&d).unwrap()).unwrap();
    fs::copy(&index, &prev).unwrap();
    let err = fails(
        &[
            "index",
            "--releases",
            s(&w.dir.join("releases")),
            "--previous",
            s(&prev),
            "--expires-days",
            "30",
            "-o",
            s(&w.dir.join("x.json")),
        ],
        &[],
    );
    assert!(err.contains("immutable"), "{err}");
    fs::remove_file(&f).unwrap();
    assert!(fails(
        &[
            "index",
            "--releases",
            s(&w.dir.join("releases")),
            "--previous",
            s(&prev),
            "--expires-days",
            "30",
            "--withdraw",
            "bridge@9.0.0",
            "-o",
            s(&w.dir.join("x.json"))
        ],
        &[],
    )
    .contains("no such release"));
}

#[test]
fn manifest_and_index_refuse_inconsistent_input() {
    let w = world("refuse");
    let artifact = w.dir.join("site/a.bin");
    fs::write(&artifact, "x").unwrap();
    let manifest = |version: &str, channel: &str, compat: &Path| {
        run(
            &[
                "manifest",
                "--component",
                "bridge",
                "--version",
                version,
                "--channel",
                channel,
                "--compat",
                s(compat),
                "--artifact",
                s(&artifact),
                "--base-url",
                "https://example.invalid",
                "--notes-url",
                "https://example.invalid/n",
                "-o",
                s(&w.dir.join("releases/bridge.json")),
            ],
            &[],
        )
    };
    let err = String::from_utf8(manifest("1.2.0", "stable", Path::new(COMPAT)).stderr).unwrap();
    assert!(err.contains("declares bridge version 1.0.0"), "{err}");
    let beta = compat_at(&w.dir, "1.1.0-beta.1");
    let err = String::from_utf8(manifest("1.1.0-beta.1", "stable", &beta).stderr).unwrap();
    assert!(err.contains("belongs to beta"), "{err}");

    // An unsigned artifact cannot be indexed unless explicitly allowed.
    assert!(manifest("1.0.0", "stable", Path::new(COMPAT))
        .status
        .success());
    let base = [
        "index",
        "--releases",
        s(&w.dir.join("releases")),
        "--previous",
        "none",
        "--expires-days",
        "30",
        "--pubkey",
        s(&w.release_pub),
    ];
    let mut args = base.to_vec();
    args.extend_from_slice(&["-o", s(&w.dir.join("i.json"))]);
    assert!(fails(&args, &[]).contains("not signed"));
    args.push("--allow-unsigned");
    ok(&args, &[]);

    // The first index needs the signing key.
    let err = fails(
        &[
            "index",
            "--releases",
            s(&w.dir.join("releases")),
            "--previous",
            "none",
            "--expires-days",
            "30",
            "--allow-unsigned",
            "-o",
            s(&w.dir.join("j.json")),
        ],
        &[],
    );
    assert!(err.contains("--pubkey"), "{err}");

    // Signing without the secret fails clearly.
    assert!(fails(
        &["sign", "--key-env", "KEYGLASS_TEST_UNSET_KEY", s(&artifact)],
        &[]
    )
    .contains("is not set"));
}

/// Write an index from the release directory and sign it with the key in `key`.
fn publish_index(w: &World, out: &Path, previous: &str, extra: &[&str], key: &str) {
    let mut args = vec![
        "index",
        "--releases",
        s(&w.dir.join("releases")),
        "--previous",
        previous,
        "--expires-days",
        "30",
    ];
    args.extend_from_slice(extra);
    args.extend_from_slice(&["-o", s(out)]);
    ok(&args, &[]);
    ok(&["sign", "--key-env", "KEY", s(out)], &[("KEY", key)]);
}

fn accept(
    w: &World,
    index: &Path,
    store: &mut MemoryTrustStore,
) -> Result<keyglass_update::VerifiedIndex, keyglass_update::trust::IndexError> {
    let json = fs::read(index).unwrap();
    let sig = fs::read_to_string(format!("{}.sig", index.display())).unwrap();
    verify_index(
        &json,
        &sig,
        &w.anchors(),
        store,
        time::OffsetDateTime::now_utc(),
    )
}

fn check_artifact(
    w: &World,
    v: &keyglass_update::VerifiedIndex,
    artifact: &keyglass_update::Artifact,
) -> Result<keyglass_update::trust::VerifiedArtifact, keyglass_update::trust::ArtifactError> {
    let file = fs::File::open(w.dir.join("site").join(&artifact.name)).unwrap();
    keyglass_update::verify_artifact(file, artifact, v.trusted_keys())
}

#[test]
fn key_rotation_end_to_end() {
    let w = world("rotate");
    let compat = PathBuf::from(COMPAT);
    let a_key = w.release_key.clone();
    let a_id = PublicKey::parse(&fs::read_to_string(&w.release_pub).unwrap())
        .unwrap()
        .id();
    let (b_pub, b_key) = keygen(&w.dir, "next");
    let b_id = PublicKey::parse(&fs::read_to_string(&b_pub).unwrap())
        .unwrap()
        .id();
    let artifact_name = "keyglass-bridge-1.0.0.bin";

    // 1. Publish with key A.
    w.publish("bridge", "1.0.0", "stable", &compat);
    let idx1 = w.dir.join("site/index-1.json");
    publish_index(&w, &idx1, "none", &["--pubkey", s(&w.release_pub)], &a_key);

    // 2. The root key trusts B.
    let trust_b = w.dir.join("trust-b.json");
    ok(
        &[
            "statement",
            "--root-key-env",
            "ROOT_KEY",
            "--trust",
            s(&b_pub),
            "-o",
            s(&trust_b),
        ],
        &w.env(),
    );

    // 3. Sign the published artifact again, with B.
    let resign = w.dir.join("resign");
    fs::create_dir_all(&resign).unwrap();
    let copy = resign.join(artifact_name);
    fs::copy(w.dir.join("site").join(artifact_name), &copy).unwrap();
    ok(
        &["sign", "--key-env", "KEY", s(&copy)],
        &[("KEY", b_key.as_str())],
    );
    let b_sig = resign.join(format!("{artifact_name}.sig"));

    // The signature is checked before it is added.
    let err = fails(
        &[
            "index",
            "--releases",
            s(&w.dir.join("releases")),
            "--previous",
            s(&idx1),
            "--expires-days",
            "30",
            "--pubkey",
            s(&w.release_pub),
            "--signature",
            s(&b_sig),
            "-o",
            s(&w.dir.join("wrong-key.json")),
        ],
        &[],
    );
    assert!(err.contains("signature was made by key"), "{err}");

    // 4. Publish the next index with B, adding B's signature.
    let idx2 = w.dir.join("site/index-2.json");
    publish_index(
        &w,
        &idx2,
        s(&idx1),
        &[
            "--pubkey",
            s(&b_pub),
            "--statement",
            s(&trust_b),
            "--signature",
            s(&b_sig),
        ],
        &b_key,
    );
    let i2 = Index::parse(&fs::read(&idx2).unwrap()).unwrap();
    assert_eq!(i2.sequence, 2);
    assert_eq!(i2.key_id, b_id);
    let mut signers = i2.all_releases().next().unwrap().artifacts[0].signed_by();
    signers.sort();
    let mut both = vec![a_id, b_id];
    both.sort();
    assert_eq!(signers, both);

    // 5. The root key revokes A; the next index, signed by B, carries it.
    let revoke_a = w.dir.join("revoke-a.json");
    ok(
        &[
            "statement",
            "--root-key-env",
            "ROOT_KEY",
            "--revoke",
            &a_id.to_string(),
            "--reason",
            "rotation",
            "-o",
            s(&revoke_a),
        ],
        &w.env(),
    );
    let idx3 = w.dir.join("site/index-3.json");
    publish_index(
        &w,
        &idx3,
        s(&idx2),
        &["--pubkey", s(&b_pub), "--statement", s(&revoke_a)],
        &b_key,
    );
    // The release directory still holds the release.json with only A's
    // signature; merging it must not drop B's.
    let i3 = Index::parse(&fs::read(&idx3).unwrap()).unwrap();
    assert_eq!(i3.key_id, b_id);
    assert_eq!(
        i3.all_releases().next().unwrap().artifacts[0]
            .signatures
            .len(),
        2
    );

    // 6. A host built with only the original anchors (root and A).
    let mut store = MemoryTrustStore::default();
    let v1 = accept(&w, &idx1, &mut store).unwrap();
    let a_only = v1.index().all_releases().next().unwrap().artifacts[0].clone();
    assert_eq!(check_artifact(&w, &v1, &a_only).unwrap().signed_by, a_id);

    let v2 = accept(&w, &idx2, &mut store).unwrap();
    let art2 = v2.index().all_releases().next().unwrap().artifacts[0].clone();
    check_artifact(&w, &v2, &art2).unwrap();

    let v3 = accept(&w, &idx3, &mut store).unwrap();
    assert!(store.state.revoked.contains(&a_id));
    let art3 = v3.index().all_releases().next().unwrap().artifacts[0].clone();
    assert_eq!(check_artifact(&w, &v3, &art3).unwrap().signed_by, b_id);

    // After the revocation, A-only signatures are refused...
    assert!(matches!(
        check_artifact(&w, &v3, &a_only),
        Err(keyglass_update::trust::ArtifactError::UntrustedKey { .. })
    ));
    // ...and so is an index signed by A, even one that omits the revocation.
    let idx4 = w.dir.join("site/index-4.json");
    let releases_only = w.dir.join("releases-empty");
    fs::create_dir_all(&releases_only).unwrap();
    ok(
        &[
            "index",
            "--releases",
            s(&releases_only),
            "--previous",
            s(&idx1),
            "--expires-days",
            "30",
            "--pubkey",
            s(&w.release_pub),
            "-o",
            s(&idx4),
        ],
        &[],
    );
    let mut i4 = Index::parse(&fs::read(&idx4).unwrap()).unwrap();
    i4.sequence = 9;
    fs::write(&idx4, i4.to_json()).unwrap();
    ok(
        &["sign", "--key-env", "KEY", s(&idx4)],
        &[("KEY", a_key.as_str())],
    );
    assert!(matches!(
        accept(&w, &idx4, &mut store),
        Err(keyglass_update::trust::IndexError::RevokedKey { .. })
    ));

    // The installer bundle built from the rotated index verifies with B.
    let bundle = w.dir.join("bundle");
    ok(
        &[
            "bundle",
            "--index",
            s(&idx3),
            "--pubkey",
            s(&w.release_pub),
            "--root-pubkey",
            s(&w.root_pub),
            "--component",
            "bridge",
            "-o",
            s(&bundle),
        ],
        &[],
    );
    assert!(bundle.join(artifact_name).exists());
}

#[test]
fn compat_check_reports_the_registry() {
    let out = ok(&["compat", "--compat", COMPAT], &[]);
    assert!(out.contains("is consistent"));
    assert!(out.contains("bridge 1.0.0"));
    assert!(out.contains("requires uart >=1.0, <2.0"));
    let w = Dir::new("compat-bad");
    let bad = w.join("compat.toml");
    fs::write(
        &bad,
        fs::read_to_string(COMPAT).unwrap().replace(
            "requires = { uart = \">=1.0, <2.0\" }",
            "requires = { uart = \">=1.1, <2.0\" }",
        ),
    )
    .unwrap();
    assert!(fails(&["compat", "--compat", s(&bad)], &[])
        .contains("bridge requires uart >=1.1, <2.0 but firmware provides 1.0"));
}

#[test]
fn help_says_where_releases_come_from() {
    let out = ok(&["--help"], &[]);
    assert!(out.contains("release workflows"));
    let sign = ok(&["sign", "--help"], &[]);
    assert!(sign.contains("release workflow"));
}

fn factory_toml(label: &str, firmware_sha: char) -> String {
    format!(
        "[factory]\nlabel = \"{label}\"\ncontainer_url = \"https://downloads.example.com/package.zip\"\ncontainer_size = 100000\n\
         [[factory.files]]\ncomponent = \"firmware\"\nmember = \"a/firmware.uf2\"\nversion = \"2.3\"\nsize = 10\nsha256 = \"{}\"\n\
         [[factory.files]]\ncomponent = \"bridge\"\nmember = \"a/bridge.ipk\"\nversion = \"2.3\"\nsize = 20\nsha256 = \"{}\"\n\
         [[factory.files]]\ncomponent = \"display\"\nmember = \"a/display.ipk\"\nversion = \"2.3\"\nsize = 30\nsha256 = \"{}\"\n",
        firmware_sha.to_string().repeat(64),
        "b".repeat(64),
        "c".repeat(64),
    )
}

#[test]
fn factory_check_validates_the_reference() {
    let d = Dir::new("factory-check");
    let good = d.join("factory-reference.toml");
    fs::write(&good, factory_toml("Factory software 2.3", 'a')).unwrap();
    let out = ok(&["factory-check", s(&good)], &[]);
    assert!(out.contains("is valid: Factory software 2.3"), "{out}");
    assert!(out.contains("bridge 2.3 a/bridge.ipk"), "{out}");

    let bad = d.join("bad.toml");
    fs::write(
        &bad,
        factory_toml("Factory software 2.3", 'a').replace("https://", "http://"),
    )
    .unwrap();
    assert!(fails(&["factory-check", s(&bad)], &[]).contains("not an https URL"));
}

#[test]
fn index_records_replaces_and_keeps_the_factory_reference() {
    let w = world("factory-index");
    w.publish("bridge", "1.0.0", "stable", Path::new(COMPAT));
    let f1 = w.dir.join("factory-1.toml");
    fs::write(&f1, factory_toml("Factory software 2.3", 'a')).unwrap();
    let f2 = w.dir.join("factory-2.toml");
    fs::write(&f2, factory_toml("Factory software 2.4", 'd')).unwrap();
    let prev = w.dir.join("prev.json");
    let label = |w: &World| w.read_index().factory.map(|f| f.label);

    // Recorded from --factory, and covered by the signature.
    let index = w.index(
        "none",
        &["--pubkey", s(&w.release_pub), "--factory", s(&f1)],
    );
    assert_eq!(label(&w).as_deref(), Some("Factory software 2.3"));
    let json = fs::read(&index).unwrap();
    let sig = fs::read_to_string(w.dir.join("site/index.json.sig")).unwrap();
    let v = verify_index(
        &json,
        &sig,
        &w.anchors(),
        &mut MemoryTrustStore::default(),
        time::OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert_eq!(v.factory().unwrap().label, "Factory software 2.3");

    // Kept when the next publish does not pass --factory.
    fs::copy(&index, &prev).unwrap();
    let out = ok(
        &[
            "index",
            "--releases",
            s(&w.dir.join("releases")),
            "--previous",
            s(&prev),
            "--expires-days",
            "30",
            "-o",
            s(&index),
        ],
        &[],
    );
    assert!(
        out.contains("factory reference kept from the previous index"),
        "{out}"
    );
    assert_eq!(label(&w).as_deref(), Some("Factory software 2.3"));

    // Replaced by a later --factory.
    fs::copy(&index, &prev).unwrap();
    w.index(s(&prev), &["--factory", s(&f2)]);
    let i = w.read_index();
    assert_eq!(i.sequence, 3);
    let f = i.factory.unwrap();
    assert_eq!(f.label, "Factory software 2.4");
    assert_eq!(
        f.file(keyglass_update::Component::Firmware).unwrap().sha256,
        "d".repeat(64)
    );

    // An invalid reference is refused before anything is written.
    let bad = w.dir.join("bad.toml");
    fs::write(
        &bad,
        factory_toml("x", 'a').replace("component = \"display\"", "component = \"bridge\""),
    )
    .unwrap();
    let err = fails(
        &[
            "index",
            "--releases",
            s(&w.dir.join("releases")),
            "--previous",
            s(&prev),
            "--expires-days",
            "30",
            "--factory",
            s(&bad),
            "-o",
            s(&w.dir.join("bad.json")),
        ],
        &[],
    );
    assert!(
        err.contains("bridge is listed more than once") && err.contains("display is missing"),
        "{err}"
    );
    assert!(!w.dir.join("bad.json").exists());
}

#[test]
fn factory_check_accepts_the_repository_reference() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../factory-reference.toml");
    let out = ok(&["factory-check", path], &[]);
    assert!(out.contains("is valid"), "{out}");
    for c in ["firmware", "bridge", "display"] {
        assert!(out.contains(&format!("\n  {c} ")), "{c}: {out}");
    }
    assert!(out.contains("press-refinement source "), "{out}");
    assert!(out.contains("payload 1456 bytes"), "{out}");
}

#[test]
fn index_carries_and_keeps_the_press_refinement_source() {
    let w = world("refinement-source-index");
    w.publish("bridge", "1.0.0", "stable", Path::new(COMPAT));
    let repo = concat!(env!("CARGO_MANIFEST_DIR"), "/../../factory-reference.toml");
    let index = w.index("none", &["--pubkey", s(&w.release_pub), "--factory", repo]);
    let refinement = w.read_index().press_refinement_source.expect("recorded");
    assert_eq!(refinement.payload_length, 1456);

    let prev = w.dir.join("prev.json");
    fs::copy(&index, &prev).unwrap();
    w.index(s(&prev), &[]);
    assert_eq!(w.read_index().press_refinement_source, Some(refinement));
}
