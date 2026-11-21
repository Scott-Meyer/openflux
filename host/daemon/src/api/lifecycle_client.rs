//! Client side of `openfluxd --request-restart` and `--request-stop`.
//!
//! The command finds the running daemon through its discovery file (`config/keys.json` in the data
//! directory), authenticates with the owner-only enrollment secret and calls
//! `POST /v1/daemon/lifecycle`.
//!
//! Installers treat "not running" as safe to replace files, so it is reported only on positive
//! evidence: no process holds the daemon's instance lock, and either there is no discovery file or
//! the recorded port refuses connections. Anything else that goes wrong is an error.

use crate::lifecycle::ExitKind;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::CertificateDer;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What happened to an exit request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestOutcome {
    /// The daemon was idle and has exited (for a restart, the service manager starts it again).
    Done,
    /// The daemon is working on the keyboard and exits by itself when it finishes.
    Deferred {
        busy: Vec<String>,
    },
    /// No daemon is running: no process holds the instance lock, and there is no discovery file or
    /// its port refuses connections.
    NotRunning,
    Error(String),
}

impl RequestOutcome {
    pub fn exit_code(&self) -> i32 {
        match self {
            RequestOutcome::Done => 0,
            RequestOutcome::Deferred { .. } => 3,
            RequestOutcome::NotRunning => 4,
            RequestOutcome::Error(_) => 1,
        }
    }

    /// The word printed for this outcome.
    pub fn label(&self, kind: ExitKind) -> String {
        match (self, kind) {
            (RequestOutcome::Done, ExitKind::Restart) => "restarted".into(),
            (RequestOutcome::Done, ExitKind::Stop) => "stopped".into(),
            (RequestOutcome::Deferred { .. }, _) => "deferred".into(),
            (RequestOutcome::NotRunning, _) => "not-running".into(),
            (RequestOutcome::Error(e), _) => format!("error: {e}"),
        }
    }
}

/// How to reach the daemon, read from its discovery file.
#[derive(Debug, Clone)]
pub struct DaemonEndpoint {
    pub addr: SocketAddr,
    /// PEM certificate to trust; `None` for a plaintext daemon.
    pub cert_pem: Option<String>,
}

impl DaemonEndpoint {
    /// Reads `config/keys.json` in `data_dir`. `None` only when the file doesn't exist; a file that
    /// exists but can't be read is an error.
    pub fn discover(data_dir: &Path) -> Result<Option<Self>, String> {
        let keys = data_dir.join("config/keys.json");
        let raw = match std::fs::read_to_string(&keys) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("read {}: {e}", keys.display())),
        };
        let doc: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| format!("{} is not valid JSON: {e}", keys.display()))?;
        let port = doc
            .get("openPort")
            .and_then(|v| v.as_u64())
            .and_then(|p| u16::try_from(p).ok())
            .ok_or_else(|| format!("{} has no openPort", keys.display()))?;
        let host: std::net::IpAddr = doc
            .get("host")
            .and_then(|v| v.as_str())
            .and_then(|h| h.parse().ok())
            .unwrap_or(std::net::IpAddr::from([127, 0, 0, 1]));
        let cert_pem = if doc.get("scheme").and_then(|v| v.as_str()) == Some("http") {
            None
        } else {
            let cert_path = doc
                .get("publicKeyLocation")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("{} has no publicKeyLocation", keys.display()))?;
            Some(std::fs::read_to_string(cert_path).map_err(|e| format!("read {cert_path}: {e}"))?)
        };
        Ok(Some(Self {
            addr: SocketAddr::new(host, port),
            cert_pem,
        }))
    }
}

/// A plain HTTP/1.1 response.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

/// Error from [`request`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    /// Nothing listens on the address: the connection was actively refused.
    Refused(String),
    /// Anything else: a timeout, a TLS or HTTP failure, a dropped connection.
    Failed(String),
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestError::Refused(e) | RequestError::Failed(e) => f.write_str(e),
        }
    }
}

fn tls_config(cert_pem: &str) -> Result<Arc<rustls::ClientConfig>, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut roots = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(cert_pem.as_bytes()) {
        let cert = cert.map_err(|e| format!("parse certificate: {e}"))?;
        roots
            .add(cert)
            .map_err(|e| format!("trust certificate: {e}"))?;
    }
    Ok(Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    ))
}

/// Sends one request to a local daemon over HTTPS (trusting only `cert_pem`) or plain HTTP.
pub fn request(
    addr: SocketAddr,
    cert_pem: Option<&str>,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
    timeout: Duration,
) -> Result<HttpResponse, RequestError> {
    let stream =
        TcpStream::connect_timeout(&addr, timeout.min(Duration::from_secs(5))).map_err(|e| {
            if e.kind() == std::io::ErrorKind::ConnectionRefused {
                RequestError::Refused(format!("connect {addr}: {e}"))
            } else {
                RequestError::Failed(format!("connect {addr}: {e}"))
            }
        })?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(b) = body {
        req.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ));
    } else {
        req.push_str("Content-Length: 0\r\n\r\n");
    }
    let fail = |e: std::io::Error| RequestError::Failed(format!("{method} {path}: {e}"));
    let mut raw = Vec::new();
    match cert_pem {
        Some(pem) => {
            let config = tls_config(pem).map_err(RequestError::Failed)?;
            let name =
                rustls::pki_types::ServerName::try_from("localhost").expect("valid server name");
            let conn = rustls::ClientConnection::new(config, name)
                .map_err(|e| RequestError::Failed(e.to_string()))?;
            let mut tls = rustls::StreamOwned::new(conn, stream);
            tls.write_all(req.as_bytes()).map_err(fail)?;
            read_to_close(&mut tls, &mut raw).map_err(fail)?;
        }
        None => {
            let mut stream = stream;
            stream.write_all(req.as_bytes()).map_err(fail)?;
            read_to_close(&mut stream, &mut raw).map_err(fail)?;
        }
    }
    parse_response(&raw)
        .ok_or_else(|| RequestError::Failed(format!("{method} {path}: malformed response")))
}

fn read_to_close(r: &mut impl Read, out: &mut Vec<u8>) -> std::io::Result<()> {
    match r.read_to_end(out) {
        Ok(_) => Ok(()),
        // The server may close without a TLS close_notify once the response is complete.
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof && !out.is_empty() => Ok(()),
        Err(e) => Err(e),
    }
}

fn parse_response(raw: &[u8]) -> Option<HttpResponse> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..split]).ok()?;
    let status = head.split_whitespace().nth(1)?.parse().ok()?;
    let mut body = raw[split + 4..].to_vec();
    if head.lines().any(|l| {
        l.to_ascii_lowercase().starts_with("transfer-encoding:")
            && l.to_ascii_lowercase().contains("chunked")
    }) {
        body = dechunk(&body)?;
    }
    Some(HttpResponse { status, body })
}

fn dechunk(mut data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = data.windows(2).position(|w| w == b"\r\n")?;
        let size =
            usize::from_str_radix(std::str::from_utf8(&data[..line_end]).ok()?.trim(), 16).ok()?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(data.get(..size)?);
        data = data.get(size + 2..)?;
    }
}

fn read_secret(data_dir: &Path) -> Result<String, String> {
    if let Ok(s) = std::env::var("OPENFLUX_ENROLLMENT_SECRET") {
        if !s.is_empty() {
            return Ok(s);
        }
    }
    let p = data_dir.join("config/enrollment.secret");
    std::fs::read_to_string(&p)
        .map(|s| s.trim().to_string())
        .map_err(|e| format!("read {}: {e}", p.display()))
}

/// Asks the daemon whose data directory is `data_dir` to exit, and when it exits right away,
/// waits up to `wait` for it to be gone.
pub fn request_exit(data_dir: &Path, kind: ExitKind, wait: Duration) -> RequestOutcome {
    request_exit_with_timeout(data_dir, kind, wait, Duration::from_secs(10))
}

/// `Ok` when no process holds the instance lock, so the daemon is gone; otherwise an error that
/// adds the reason to `refused`, which says why the daemon couldn't be reached.
fn refused_means_gone(data_dir: &Path, refused: &str) -> Result<(), String> {
    match crate::lifecycle::InstanceLock::probe(data_dir) {
        crate::lifecycle::InstanceProbe::Free => Ok(()),
        crate::lifecycle::InstanceProbe::Held => Err(format!(
            "{refused}, but a daemon process still holds the instance lock; it may be starting or not responding"
        )),
        crate::lifecycle::InstanceProbe::Unknown(e) => Err(format!("{refused}, and the instance lock couldn't be checked: {e}")),
    }
}

/// [`request_exit`] with an explicit timeout for each request to the daemon.
pub fn request_exit_with_timeout(
    data_dir: &Path,
    kind: ExitKind,
    wait: Duration,
    timeout: Duration,
) -> RequestOutcome {
    let endpoint = match DaemonEndpoint::discover(data_dir) {
        Ok(Some(e)) => e,
        Ok(None) => {
            return match refused_means_gone(data_dir, "there is no discovery file") {
                Ok(()) => RequestOutcome::NotRunning,
                Err(e) => RequestOutcome::Error(e),
            }
        }
        Err(e) => return RequestOutcome::Error(e),
    };
    let secret = match read_secret(data_dir) {
        Ok(s) => s,
        Err(e) => return RequestOutcome::Error(e),
    };
    let call = |endpoint: &DaemonEndpoint, action: &str| {
        let body = serde_json::json!({ "action": action, "enrollmentSecret": secret }).to_string();
        request(
            endpoint.addr,
            endpoint.cert_pem.as_deref(),
            "POST",
            "/v1/daemon/lifecycle",
            &[],
            Some(&body),
            timeout,
        )
    };
    let resp = match call(&endpoint, kind.as_str()) {
        Ok(r) => r,
        Err(RequestError::Refused(e)) => {
            return match refused_means_gone(data_dir, &e) {
                Ok(()) => RequestOutcome::NotRunning,
                Err(e) => RequestOutcome::Error(e),
            }
        }
        Err(e) => return RequestOutcome::Error(e.to_string()),
    };
    let reply = resp.json();
    if resp.status != 200 {
        let msg = reply
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unexpected response");
        return RequestOutcome::Error(format!("daemon answered {}: {msg}", resp.status));
    }
    match reply.get("outcome").and_then(|v| v.as_str()) {
        Some("deferred") => RequestOutcome::Deferred {
            busy: reply
                .get("busy")
                .and_then(|b| b.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
        },
        Some("exiting") => {
            let instance = reply
                .get("instanceId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if instance.is_empty() {
                return RequestOutcome::Error("the daemon's answer has no instance id".into());
            }
            let deadline = Instant::now() + wait;
            loop {
                let lock_free = crate::lifecycle::InstanceLock::probe(data_dir)
                    == crate::lifecycle::InstanceProbe::Free;
                // A restarted daemon may listen on another port, so the endpoint is read again.
                let current = DaemonEndpoint::discover(data_dir)
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| endpoint.clone());
                let answered = match call(&current, "status") {
                    Ok(r) if r.status == 200 => r
                        .json()
                        .get("instanceId")
                        .and_then(|v| v.as_str())
                        .filter(|id| !id.is_empty())
                        .map(String::from),
                    _ => None,
                };
                match (kind, answered) {
                    (ExitKind::Restart, Some(id)) if id != instance => return RequestOutcome::Done,
                    (ExitKind::Stop, Some(id)) if id != instance => {
                        return RequestOutcome::Error(format!(
                            "the daemon stopped, but another daemon ({id}) is running for this data directory"
                        ))
                    }
                    (_, None) if lock_free => return RequestOutcome::Done,
                    _ => {}
                }
                if Instant::now() >= deadline {
                    return RequestOutcome::Error(format!(
                        "the daemon accepted the request but wasn't confirmed gone after {}s",
                        wait.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        other => RequestOutcome::Error(format!("unexpected outcome {other:?}")),
    }
}
