//! `openflux-release`: produce, sign and check OpenFlux release metadata.
//!
//! Releases are published only by the release workflows. This tool is what
//! those workflows run; locally it is useful for checking `compat.toml`,
//! inspecting manifests and verifying downloads. Signing needs a release key,
//! which exists only as a secret in the release environment.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use openflux_update::compat::Compat;
use openflux_update::factory::{ReferenceFile, SourceLocation};
use openflux_update::manifest::{Artifact, Channel, Index, KeyStatement, Release, SignedStatement};
use openflux_update::signing::{self, SecretKey};
use openflux_update::trust::{self, KeyId, PublicKey, Signature};
use openflux_update::trust::{MemoryTrustStore, TrustAnchors};
use openflux_update::version::Component;
use time::OffsetDateTime;

type Result<T> = std::result::Result<T, String>;

#[derive(Parser)]
#[command(
    name = "openflux-release",
    version,
    about = "Produce, sign and check OpenFlux release metadata",
    long_about = "Produce, sign and check OpenFlux release metadata.\n\n\
        Releases are published only by the release workflows; signing keys exist only as secrets \
        in the release environment. Locally this tool checks compat.toml, inspects manifests and \
        verifies downloaded files."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write release.json for one component release.
    Manifest(ManifestArgs),
    /// Merge releases into the next index.json.
    Index(IndexArgs),
    /// Sign a file with a release or root key, writing <file>.sig (release workflow only).
    Sign(SignArgs),
    /// Verify <file>.sig against a public key.
    Verify(VerifyArgs),
    /// Generate a key pair for offline storage.
    Keygen(KeygenArgs),
    /// Write a root-signed statement that trusts or revokes a release key.
    Statement(StatementArgs),
    /// Check compat.toml and print the interfaces of each component.
    Compat(CompatArgs),
    /// Download and verify the keyboard software a host installer bundles.
    Bundle(BundleArgs),
    /// Check a factory-reference.toml file.
    FactoryCheck(FactoryCheckArgs),
}

#[derive(Args)]
struct ManifestArgs {
    #[arg(long)]
    component: Component,
    #[arg(long)]
    version: semver::Version,
    #[arg(long)]
    channel: Channel,
    #[arg(long, default_value = "compat.toml")]
    compat: PathBuf,
    /// Artifact file. Its signature is taken from <path>.sig when present.
    #[arg(long = "artifact", required = true)]
    artifacts: Vec<PathBuf>,
    /// URL the artifacts are published under; each artifact's URL is
    /// <base-url>/<file name>.
    #[arg(long)]
    base_url: String,
    #[arg(long)]
    notes_url: String,
    /// Release time (RFC 3339). Defaults to SOURCE_DATE_EPOCH when set, else the current time.
    #[arg(long)]
    released: Option<String>,
    #[arg(short = 'o', long)]
    output: PathBuf,
}

#[derive(Args)]
struct IndexArgs {
    /// Directory of release.json files (every *.json file in it).
    #[arg(long)]
    releases: PathBuf,
    /// The published index.json, or `none` for the first index.
    /// Its releases and key statements are carried over and its sequence is
    /// incremented.
    #[arg(long)]
    previous: String,
    #[arg(long)]
    expires_days: u32,
    /// Public key of the release key that will sign this index. Required for
    /// the first index and for --signature; otherwise defaults to the key of
    /// the previous index. Pass it on every publish after a key rotation so the
    /// index records the active key.
    #[arg(long)]
    pubkey: Option<PathBuf>,
    /// Signature of a published artifact by the --pubkey key, to add to that
    /// artifact's signatures (for example after a key rotation). The artifact
    /// itself must sit next to it as <file> for <file>.sig; it is checked
    /// against the published size, hash and signature before the signature is
    /// added.
    #[arg(long = "signature")]
    signatures: Vec<PathBuf>,
    /// factory-reference.toml to record in the index: the factory reference
    /// and the press-refinement source. Without it the previous index's are
    /// kept.
    #[arg(long)]
    factory: Option<PathBuf>,
    /// Root-signed statement file to add (from `statement`).
    #[arg(long = "statement")]
    statements: Vec<PathBuf>,
    /// Mark a published release as withdrawn: <component>@<version>.
    #[arg(long = "withdraw")]
    withdraw: Vec<String>,
    /// Accept artifacts without signatures (test indexes only).
    #[arg(long)]
    allow_unsigned: bool,
    #[arg(short = 'o', long)]
    output: PathBuf,
}

#[derive(Args)]
struct SignArgs {
    /// Environment variable holding the minisign secret key.
    #[arg(long)]
    key_env: String,
    /// Environment variable holding the key's password, for a password-protected key.
    #[arg(long)]
    password_env: Option<String>,
    file: PathBuf,
}

#[derive(Args)]
struct VerifyArgs {
    /// Minisign public key file; may be repeated to accept any of several keys.
    #[arg(long, required = true)]
    pubkey: Vec<PathBuf>,
    /// Signature file. Defaults to <file>.sig.
    #[arg(long)]
    signature: Option<PathBuf>,
    file: PathBuf,
}

#[derive(Args)]
struct KeygenArgs {
    #[arg(long)]
    out_dir: PathBuf,
    /// File name stem; the key id is appended.
    #[arg(long, default_value = "openflux")]
    name: String,
}

#[derive(Args)]
struct StatementArgs {
    /// Environment variable holding the root secret key.
    #[arg(long)]
    root_key_env: String,
    #[arg(long)]
    password_env: Option<String>,
    /// Trust the release key in this public key file.
    #[arg(long, conflicts_with = "revoke", required_unless_present = "revoke")]
    trust: Option<PathBuf>,
    /// Revoke the release key with this id.
    #[arg(long)]
    revoke: Option<KeyId>,
    #[arg(long, requires = "revoke")]
    reason: Option<String>,
    #[arg(short = 'o', long)]
    output: PathBuf,
}

#[derive(Args)]
struct FactoryCheckArgs {
    file: PathBuf,
}

#[derive(Args)]
struct BundleArgs {
    /// URL or path of index.json; index.json.sig is fetched from next to it.
    #[arg(long)]
    index: String,
    /// Release public key file; may be repeated.
    #[arg(long, required = true)]
    pubkey: Vec<PathBuf>,
    /// Root public key file for the index's key statements; may be repeated.
    /// Defaults to the root key built into this tool.
    #[arg(long)]
    root_pubkey: Vec<PathBuf>,
    #[arg(long, default_value = "stable")]
    channel: Channel,
    /// Component to bundle; may be repeated.
    #[arg(long = "component", required = true)]
    components: Vec<Component>,
    /// Skip components with no eligible release instead of failing.
    #[arg(long)]
    allow_missing: bool,
    /// Output directory: index.json, index.json.sig and the artifacts, flat.
    #[arg(short = 'o', long)]
    output: PathBuf,
}

#[derive(Args)]
struct CompatArgs {
    #[arg(long, default_value = "compat.toml")]
    compat: PathBuf,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Manifest(a) => manifest(a),
        Command::Index(a) => index(a),
        Command::Sign(a) => sign(a),
        Command::Verify(a) => verify(a),
        Command::Keygen(a) => keygen(a),
        Command::Statement(a) => statement(a),
        Command::Compat(a) => compat(a),
        Command::Bundle(a) => bundle(a),
        Command::FactoryCheck(a) => factory_check(a),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("openflux-release: {e}");
            ExitCode::FAILURE
        }
    }
}

fn read(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_text(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn write(path: &Path, contents: &str) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    fs::write(path, contents).map_err(|e| format!("{}: {e}", path.display()))
}

fn sig_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".sig");
    PathBuf::from(s)
}

fn now() -> OffsetDateTime {
    let t = OffsetDateTime::now_utc();
    t.replace_nanosecond(0).unwrap_or(t)
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn load_compat(path: &Path) -> Result<Compat> {
    Compat::parse(&read_text(path)?).map_err(|e| format!("{}: {e}", path.display()))
}

fn secret_from_env(key_env: &str, password_env: Option<&str>) -> Result<SecretKey> {
    let text =
        std::env::var(key_env).map_err(|_| format!("environment variable {key_env} is not set"))?;
    let password = match password_env {
        Some(p) => {
            Some(std::env::var(p).map_err(|_| format!("environment variable {p} is not set"))?)
        }
        None => None,
    };
    SecretKey::parse(&text, password).map_err(|e| e.to_string())
}

fn to_json<T: serde::Serialize>(v: &T) -> String {
    let mut s = serde_json::to_string_pretty(v).expect("serializable");
    s.push('\n');
    s
}

fn manifest(a: ManifestArgs) -> Result<()> {
    let compat = load_compat(&a.compat)?;
    let declared = &compat.components[&a.component].version;
    if declared != &a.version {
        return Err(format!(
            "{} declares {} version {declared}, but --version is {}; release tooling must update compat.toml with the release",
            a.compat.display(),
            a.component,
            a.version
        ));
    }
    let released = match (&a.released, std::env::var("SOURCE_DATE_EPOCH")) {
        (Some(s), _) => OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|e| format!("--released {s:?}: {e}"))?,
        (None, Ok(epoch)) => {
            let secs: i64 = epoch
                .trim()
                .parse()
                .map_err(|_| format!("SOURCE_DATE_EPOCH {epoch:?} is not a number"))?;
            OffsetDateTime::from_unix_timestamp(secs)
                .map_err(|e| format!("SOURCE_DATE_EPOCH: {e}"))?
        }
        (None, Err(_)) => now(),
    };
    let base = a.base_url.trim_end_matches('/');
    let mut artifacts = Vec::new();
    for path in &a.artifacts {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("{}: not a file name", path.display()))?
            .to_string();
        let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let (sha256, size) =
            trust::sha256_hex(file).map_err(|e| format!("{}: {e}", path.display()))?;
        let sig = sig_path(path);
        let signatures = if sig.exists() {
            let text = read_text(&sig)?;
            Signature::parse(&text).map_err(|e| format!("{}: {e}", sig.display()))?;
            vec![text]
        } else {
            Vec::new()
        };
        artifacts.push(Artifact {
            url: format!("{base}/{name}"),
            name,
            sha256,
            size,
            signatures,
        });
    }
    let release = Release {
        schema: Release::SCHEMA,
        component: a.component,
        version: a.version,
        channel: a.channel,
        released,
        notes_url: a.notes_url,
        interfaces: compat.interface_set(a.component),
        artifacts,
        withdrawn: false,
    };
    release.validate().map_err(|e| e.to_string())?;
    write(&a.output, &to_json(&release))?;
    let unsigned = release
        .artifacts
        .iter()
        .filter(|x| x.signatures.is_empty())
        .count();
    eprintln!(
        "wrote {} for {} ({} artifacts{})",
        a.output.display(),
        release.label(),
        release.artifacts.len(),
        if unsigned > 0 {
            format!(", {unsigned} unsigned")
        } else {
            String::new()
        }
    );
    Ok(())
}

fn parse_withdraw(s: &str) -> Result<(Component, semver::Version)> {
    let (c, v) = s
        .split_once('@')
        .ok_or_else(|| format!("--withdraw {s:?}: expected <component>@<version>"))?;
    let c: Component = c.parse().map_err(|e| format!("--withdraw {s:?}: {e}"))?;
    let v: semver::Version = v.parse().map_err(|e| format!("--withdraw {s:?}: {e}"))?;
    Ok((c, v))
}

fn index(a: IndexArgs) -> Result<()> {
    let previous = if a.previous == "none" {
        None
    } else {
        let p = PathBuf::from(&a.previous);
        Some(Index::parse(&read(&p)?).map_err(|e| format!("{}: {e}", p.display()))?)
    };

    let mut releases: BTreeMap<(Component, semver::Version), Release> = BTreeMap::new();
    if let Some(prev) = &previous {
        for r in prev.all_releases() {
            releases.insert((r.component, r.version.clone()), r.clone());
        }
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(&a.releases)
        .map_err(|e| format!("{}: {e}", a.releases.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    entries.sort();
    let mut added = 0;
    for path in entries {
        let r = Release::parse(&read(&path)?).map_err(|e| format!("{}: {e}", path.display()))?;
        let key = (r.component, r.version.clone());
        match releases.get_mut(&key) {
            Some(existing) => existing
                .merge(&r)
                .map_err(|e| format!("{}: {e}", path.display()))?,
            None => {
                releases.insert(key, r);
                added += 1;
            }
        }
    }
    let active_key = match &a.pubkey {
        Some(p) => {
            Some(PublicKey::parse(&read_text(p)?).map_err(|e| format!("{}: {e}", p.display()))?)
        }
        None => None,
    };
    let key_id = match (&active_key, &previous) {
        (Some(k), _) => k.id(),
        (None, Some(prev)) => prev.key_id,
        (None, None) => {
            return Err("the first index needs --pubkey for the key that will sign it".to_string())
        }
    };

    for sig_file in &a.signatures {
        let key = active_key
            .as_ref()
            .ok_or_else(|| "--signature needs --pubkey for the key that made it".to_string())?;
        let text = read_text(sig_file)?;
        let sig = Signature::parse(&text).map_err(|e| format!("{}: {e}", sig_file.display()))?;
        let file = sig_file.with_extension("");
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|_| sig_file.extension().is_some_and(|x| x == "sig"))
            .ok_or_else(|| format!("{}: expected <artifact>.sig", sig_file.display()))?
            .to_string();
        let mut matches: Vec<&mut Artifact> = releases
            .values_mut()
            .flat_map(|r| r.artifacts.iter_mut())
            .filter(|art| art.name == name)
            .collect();
        let art = match matches.as_mut_slice() {
            [one] => one,
            [] => {
                return Err(format!(
                    "{}: no published artifact is named {name}",
                    sig_file.display()
                ))
            }
            _ => {
                return Err(format!(
                    "{}: several published artifacts are named {name}",
                    sig_file.display()
                ))
            }
        };
        let bytes = fs::File::open(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let (sha256, size) =
            trust::sha256_hex(bytes).map_err(|e| format!("{}: {e}", file.display()))?;
        if sha256 != art.sha256 || size != art.size {
            return Err(format!(
                "{}: does not match the published {name}",
                file.display()
            ));
        }
        let bytes = fs::File::open(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        key.verify(&sig, bytes)
            .map_err(|e| format!("{}: {e}", sig_file.display()))?;
        art.add_signatures(std::slice::from_ref(&text))
            .map_err(|e| e.to_string())?;
    }
    for w in &a.withdraw {
        let key = parse_withdraw(w)?;
        let r = releases
            .get_mut(&key)
            .ok_or_else(|| format!("--withdraw {w}: no such release"))?;
        r.withdrawn = true;
    }
    if !a.allow_unsigned {
        for r in releases.values() {
            for art in &r.artifacts {
                if art.signatures.is_empty() {
                    return Err(format!(
                        "{}: artifact {} is not signed; sign artifacts before writing their manifest",
                        r.label(),
                        art.name
                    ));
                }
            }
        }
    }

    let mut statements: Vec<SignedStatement> = previous
        .as_ref()
        .map(|p| p.key_statements.clone())
        .unwrap_or_default();
    for path in &a.statements {
        let s: SignedStatement = serde_json::from_slice(&read(path)?)
            .map_err(|e| format!("{}: not a statement file: {e}", path.display()))?;
        serde_json::from_str::<KeyStatement>(&s.statement)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Signature::parse(&s.signature).map_err(|e| format!("{}: {e}", path.display()))?;
        if !statements.contains(&s) {
            statements.push(s);
        }
    }

    let (factory, press_refinement_source, factory_note) = match &a.factory {
        Some(p) => {
            let file = load_factory(p)?;
            (
                Some(file.factory),
                file.press_refinement_source,
                "recorded from --factory",
            )
        }
        None => match previous.as_ref() {
            Some(p) if p.factory.is_some() || p.press_refinement_source.is_some() => (
                p.factory.clone(),
                p.press_refinement_source.clone(),
                "kept from the previous index",
            ),
            _ => (None, None, "none"),
        },
    };

    let issued = now();
    let mut by_channel: BTreeMap<Channel, Vec<Release>> = BTreeMap::new();
    for r in releases.into_values() {
        by_channel.entry(r.channel).or_default().push(r);
    }
    let index = Index {
        schema: Index::SCHEMA,
        sequence: previous.as_ref().map_or(1, |p| p.sequence + 1),
        issued,
        expires: issued + time::Duration::days(i64::from(a.expires_days.max(1))),
        key_id,
        releases: by_channel,
        key_statements: statements,
        factory,
        press_refinement_source,
    };
    index
        .validate_with(!a.allow_unsigned)
        .map_err(|e| e.to_string())?;
    write(&a.output, &index.to_json())?;
    eprintln!(
        "wrote {}: sequence {}, {} new releases, factory reference {}, expires {}, to be signed by key {}",
        a.output.display(),
        index.sequence,
        added,
        factory_note,
        rfc3339(index.expires),
        key_id
    );
    Ok(())
}

fn sign(a: SignArgs) -> Result<()> {
    let key = secret_from_env(&a.key_env, a.password_env.as_deref())?;
    let file = fs::File::open(&a.file).map_err(|e| format!("{}: {e}", a.file.display()))?;
    let name = a
        .file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let comment = format!("timestamp:{}\tfile:{name}", now().unix_timestamp());
    let sig = key.sign(file, &comment).map_err(|e| e.to_string())?;
    let out = sig_path(&a.file);
    write(&out, &sig)?;
    eprintln!("wrote {} (key {})", out.display(), key.id());
    Ok(())
}

fn verify(a: VerifyArgs) -> Result<()> {
    let sig_file = a.signature.clone().unwrap_or_else(|| sig_path(&a.file));
    let sig = Signature::parse(&read_text(&sig_file)?)
        .map_err(|e| format!("{}: {e}", sig_file.display()))?;
    let mut keys = Vec::new();
    for p in &a.pubkey {
        keys.push(PublicKey::parse(&read_text(p)?).map_err(|e| format!("{}: {e}", p.display()))?);
    }
    let key = keys
        .iter()
        .find(|k| k.id() == sig.key_id())
        .ok_or_else(|| {
            format!(
                "{} was signed by key {}, which is not among the given keys ({})",
                a.file.display(),
                sig.key_id(),
                keys.iter()
                    .map(|k| k.id().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    let file = fs::File::open(&a.file).map_err(|e| format!("{}: {e}", a.file.display()))?;
    key.verify(&sig, file)
        .map_err(|e| format!("{}: {e}", a.file.display()))?;
    println!("{}: good signature by key {}", a.file.display(), key.id());
    println!("trusted comment: {}", sig.trusted_comment());
    Ok(())
}

fn keygen(a: KeygenArgs) -> Result<()> {
    let generated = signing::generate().map_err(|e| e.to_string())?;
    let id = generated.secret.id();
    fs::create_dir_all(&a.out_dir).map_err(|e| format!("{}: {e}", a.out_dir.display()))?;
    let stem = format!("{}-{id}", a.name);
    let pub_path = a.out_dir.join(format!("{stem}.pub"));
    let key_path = a.out_dir.join(format!("{stem}.key"));
    for p in [&pub_path, &key_path] {
        if p.exists() {
            return Err(format!("{} already exists", p.display()));
        }
    }
    write(&pub_path, &generated.public_text)?;
    write_secret(&key_path, &generated.secret_text)?;
    eprintln!(
        "wrote {} and {} (key {id})",
        pub_path.display(),
        key_path.display()
    );
    eprintln!(
        "warning: {} is an unencrypted secret key. Move it to offline storage and delete this copy. \
         A release key is used only as a secret of the release environment; a root key never leaves offline storage \
         except to sign key statements.",
        key_path.display()
    );
    Ok(())
}

#[cfg(unix)]
fn write_secret(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(contents.as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(not(unix))]
fn write_secret(path: &Path, contents: &str) -> Result<()> {
    write(path, contents)
}

fn statement(a: StatementArgs) -> Result<()> {
    let root = secret_from_env(&a.root_key_env, a.password_env.as_deref())?;
    let issued = now();
    let stmt = match (&a.trust, a.revoke) {
        (Some(p), None) => {
            let pk =
                PublicKey::parse(&read_text(p)?).map_err(|e| format!("{}: {e}", p.display()))?;
            KeyStatement::Trust {
                key_id: pk.id(),
                public_key: pk.to_base64(),
                issued,
            }
        }
        (None, Some(key_id)) => KeyStatement::Revoke {
            key_id,
            issued,
            reason: a.reason.clone().unwrap_or_default(),
        },
        _ => return Err("give exactly one of --trust or --revoke".to_string()),
    };
    let text = serde_json::to_string(&stmt).expect("statement serializes");
    let comment = format!(
        "timestamp:{}\tkey-statement:{}",
        issued.unix_timestamp(),
        stmt.key_id()
    );
    let signature = root
        .sign_bytes(text.as_bytes(), &comment)
        .map_err(|e| e.to_string())?;
    let signed = SignedStatement {
        statement: text,
        signature,
    };
    write(&a.output, &to_json(&signed))?;
    eprintln!(
        "wrote {} (signed by root key {})",
        a.output.display(),
        root.id()
    );
    Ok(())
}

fn compat(a: CompatArgs) -> Result<()> {
    let compat = load_compat(&a.compat)?;
    println!("{} is consistent", a.compat.display());
    for (c, def) in &compat.components {
        let set = compat.interface_set(*c);
        println!("{c} {}", def.version);
        for (name, versions) in &set.provides {
            let v: Vec<String> = versions.iter().map(|v| v.to_string()).collect();
            println!("  provides {name} {}", v.join(", "));
        }
        for (name, reqs) in &set.requires {
            let r: Vec<String> = reqs.iter().map(|r| r.to_string()).collect();
            println!("  requires {name} {}", r.join(" or "));
        }
        if !def.update_transport.is_empty() {
            println!("  updated over {}", def.update_transport.join(", "));
        }
    }
    Ok(())
}

fn is_url(s: &str) -> bool {
    s.starts_with("https://") || s.starts_with("http://")
}

/// Open a URL or a local path for reading, refusing more than `limit` bytes.
fn open_source(location: &str, limit: u64) -> Result<Box<dyn std::io::Read>> {
    if is_url(location) {
        let response = ureq::get(location)
            .call()
            .map_err(|e| format!("{location}: {e}"))?;
        Ok(Box::new(
            response
                .into_body()
                .into_with_config()
                .limit(limit)
                .reader(),
        ))
    } else {
        let f = fs::File::open(location).map_err(|e| format!("{location}: {e}"))?;
        Ok(Box::new(std::io::Read::take(f, limit)))
    }
}

fn fetch(location: &str, limit: u64) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut open_source(location, limit)?, &mut out)
        .map_err(|e| format!("{location}: {e}"))?;
    Ok(out)
}

fn bundle(a: BundleArgs) -> Result<()> {
    const INDEX_LIMIT: u64 = 64 * 1024 * 1024;
    let mut release_keys = Vec::new();
    for p in &a.pubkey {
        release_keys
            .push(PublicKey::parse(&read_text(p)?).map_err(|e| format!("{}: {e}", p.display()))?);
    }
    let root_keys = if a.root_pubkey.is_empty() {
        openflux_update::keys::trust_anchors().root_keys
    } else {
        let mut keys = Vec::new();
        for p in &a.root_pubkey {
            keys.push(
                PublicKey::parse(&read_text(p)?).map_err(|e| format!("{}: {e}", p.display()))?,
            );
        }
        keys
    };
    let anchors = TrustAnchors {
        root_keys,
        release_keys,
    };

    let index_json = fetch(&a.index, INDEX_LIMIT)?;
    let sig_location = format!("{}.sig", a.index);
    let index_sig = String::from_utf8(fetch(&sig_location, 64 * 1024)?)
        .map_err(|_| format!("{sig_location}: not text"))?;
    let verified = trust::verify_index(
        &index_json,
        &index_sig,
        &anchors,
        &mut MemoryTrustStore::default(),
        now(),
    )
    .map_err(|e| format!("{}: {e}", a.index))?;
    let index = verified.index();

    let mut chosen = Vec::new();
    for &c in &a.components {
        let newest = index
            .all_releases()
            .filter(|r| r.component == c && !r.withdrawn)
            .filter(|r| a.channel == Channel::Beta || r.channel == Channel::Stable)
            .max_by(|x, y| x.version.cmp(&y.version));
        match newest {
            Some(r) => chosen.push(r.clone()),
            None if a.allow_missing => {
                eprintln!("skipping {c}: no {} {c} release published yet", a.channel)
            }
            None => {
                return Err(format!(
                    "no {} {c} release published yet; release keyboard components first",
                    a.channel
                ))
            }
        }
    }

    fs::create_dir_all(&a.output).map_err(|e| format!("{}: {e}", a.output.display()))?;
    let index_dir = if is_url(&a.index) {
        None
    } else {
        Path::new(&a.index).parent().map(Path::to_path_buf)
    };
    for r in &chosen {
        for art in &r.artifacts {
            let location = if is_url(&art.url) {
                art.url.clone()
            } else {
                let dir = index_dir.as_ref().ok_or_else(|| {
                    format!(
                        "{}: artifact {} has a relative url but the index is remote",
                        r.label(),
                        art.name
                    )
                })?;
                dir.join(&art.url).to_string_lossy().into_owned()
            };
            let part = a.output.join(format!("{}.part", art.name));
            {
                let mut src = open_source(&location, art.size + 1)?;
                let mut dst =
                    fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
                std::io::copy(&mut src, &mut dst).map_err(|e| format!("{location}: {e}"))?;
            }
            let file = fs::File::open(&part).map_err(|e| format!("{}: {e}", part.display()))?;
            if let Err(e) = trust::verify_artifact(file, art, verified.trusted_keys()) {
                let _ = fs::remove_file(&part);
                return Err(format!("{}: {e}", r.label()));
            }
            let dest = a.output.join(&art.name);
            fs::rename(&part, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        }
        eprintln!("bundled {} ({} artifacts)", r.label(), r.artifacts.len());
    }
    let index_out = a.output.join("index.json");
    fs::write(&index_out, &index_json).map_err(|e| format!("{}: {e}", index_out.display()))?;
    write(&a.output.join("index.json.sig"), &index_sig)?;
    eprintln!(
        "wrote {} (index sequence {}, signed by key {})",
        a.output.display(),
        index.sequence,
        verified.signed_by()
    );
    Ok(())
}

fn load_factory(path: &Path) -> Result<ReferenceFile> {
    ReferenceFile::parse_toml(&read_text(path)?).map_err(|e| format!("{}: {e}", path.display()))
}

fn factory_check(a: FactoryCheckArgs) -> Result<()> {
    let file = load_factory(&a.file)?;
    let f = &file.factory;
    println!("{} is valid: {}", a.file.display(), f.label);
    println!("container {} ({} bytes)", f.container_url, f.container_size);
    for c in Component::DEVICE {
        if let Some(file) = f.file(c) {
            println!(
                "  {c} {} {} ({} bytes, sha256 {})",
                file.version, file.member, file.size, file.sha256
            );
        }
    }
    match &file.press_refinement_source {
        None => println!("no press-refinement source"),
        Some(d) => {
            println!(
                "press-refinement source {}: {} ({} bytes, sha256 {})",
                d.description, d.member, d.member_size, d.member_sha256
            );
            for src in &d.sources {
                if let SourceLocation::Container { url, size, sha256 } = src {
                    println!("  container {url} ({size} bytes, sha256 {sha256})");
                }
            }
            println!(
                "  image at 0x{:08x} ({} bytes, sha256 {}), payload {} bytes, sha256 {}",
                d.image_base, d.image_size, d.image_sha256, d.payload_length, d.payload_sha256
            );
        }
    }
    Ok(())
}
