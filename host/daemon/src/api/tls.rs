//! Self-signed TLS for the local API (`0600` private key), Keyglass's own discovery file
//! (`keys.json` in the data directory) and the connection-bounded HTTPS/WSS acceptor.

use axum::Router;
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder as HttpServerBuilder;
use rustls::pki_types::pem::{self, PemObject};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;

pub const MAX_CONCURRENT_TLS_CONNS: usize = 64;
pub const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

pub struct TlsCertBundle {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub cert_pem: String,
    pub cert_sha256: String,
    pub acceptor: TlsAcceptor,
}

pub fn write_owner_only_file(path: &Path, content: &[u8], secret: bool) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Missing parent dir".to_string())?;
    let mut rand_bytes = [0u8; 8];
    getrandom::getrandom(&mut rand_bytes).map_err(|e| format!("CSPRNG error: {e}"))?;
    let suffix: String = rand_bytes.iter().map(|b| format!("{b:02x}")).collect();
    let tmp = parent.join(format!(
        ".tmp_{}_{}",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("tls"),
        suffix
    ));

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mode = if secret { 0o600 } else { 0o644 };
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)
            .map_err(|e| format!("open {}: {e}", tmp.display()))?;
        f.write_all(content).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("write {}: {e}", tmp.display())
        })?;
        f.set_permissions(std::fs::Permissions::from_mode(mode))
            .map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                format!("chmod {}: {e}", tmp.display())
            })?;
    }
    #[cfg(windows)]
    {
        write_windows_secret_atomic(&tmp, content, secret)?;
    }
    #[cfg(not(any(unix, windows)))]
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| format!("open {}: {e}", tmp.display()))?;
        f.write_all(content)
            .map_err(|e| format!("write {}: {e}", tmp.display()))?;
    }

    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("rename {}: {e}", path.display())
    })
}

#[cfg(windows)]
mod win_sec {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use std::path::Path;

    #[repr(C)]
    struct SecurityAttributes {
        n_length: u32,
        lp_security_descriptor: *mut c_void,
        b_inherit_handle: i32,
    }

    const GENERIC_WRITE: u32 = 0x4000_0000;
    const CREATE_NEW: u32 = 1;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
    const INVALID_HANDLE_VALUE: *mut c_void = -1isize as *mut c_void;
    const SDDL_REVISION_1: u32 = 1;
    const DACL_SECURITY_INFORMATION: u32 = 0x0000_0004;
    const PROTECTED_DACL_SECURITY_INFORMATION: u32 = 0x8000_0000;
    const SE_FILE_OBJECT: u32 = 1;

    #[link(name = "advapi32")]
    extern "system" {
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            string_sd: *const u16,
            string_sd_revision: u32,
            security_descriptor: *mut *mut c_void,
            security_descriptor_size: *mut u32,
        ) -> i32;
        fn GetSecurityDescriptorDacl(
            p_security_descriptor: *mut c_void,
            lpb_dacl_present: *mut i32,
            p_dacl: *mut *mut c_void,
            lpb_dacl_defaulted: *mut i32,
        ) -> i32;
        fn SetNamedSecurityInfoW(
            p_object_name: *const u16,
            object_type: u32,
            security_info: u32,
            psid_owner: *mut c_void,
            psid_group: *mut c_void,
            p_dacl: *mut c_void,
            p_sacl: *mut c_void,
        ) -> u32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateFileW(
            lp_file_name: *const u16,
            dw_desired_access: u32,
            dw_share_mode: u32,
            lp_security_attributes: *const SecurityAttributes,
            dw_creation_disposition: u32,
            dw_flags_and_attributes: u32,
            h_template_file: *mut c_void,
        ) -> *mut c_void;
        fn LocalFree(h_mem: *mut c_void) -> *mut c_void;
    }

    fn current_user_sid_string() -> Result<String, String> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let who = std::process::Command::new("whoami")
            .arg("/user")
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| format!("whoami /user failed: {e}"))?;
        String::from_utf8_lossy(&who.stdout)
            .split_whitespace()
            .find(|tok| tok.starts_with("S-1-5-"))
            .map(|s| s.to_string())
            .ok_or_else(|| "Could not determine current Windows user SID".to_string())
    }

    fn to_wide_nul(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn protected_owner_sddl() -> Result<Vec<u16>, String> {
        let sid = current_user_sid_string()?;
        // Protected DACL ("D:P") with single Generic-All ACE for current user SID only.
        let sddl = format!("D:P(A;;GA;;;{sid})");
        Ok(to_wide_nul(std::ffi::OsStr::new(&sddl)))
    }

    pub fn write_secret_atomic(path: &Path, content: &[u8], secret: bool) -> Result<(), String> {
        if !secret {
            return std::fs::write(path, content)
                .map_err(|e| format!("write {}: {e}", path.display()));
        }
        let sddl = protected_owner_sddl()?;
        let path_w = to_wide_nul(path.as_os_str());
        unsafe {
            let mut sd: *mut c_void = std::ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                std::ptr::null_mut(),
            ) == 0
            {
                return Err("ConvertStringSecurityDescriptorToSecurityDescriptorW failed".into());
            }
            let sa = SecurityAttributes {
                n_length: std::mem::size_of::<SecurityAttributes>() as u32,
                lp_security_descriptor: sd,
                b_inherit_handle: 0,
            };
            // CREATE_NEW with no sharing: the file exists only with the owner-only DACL, from its first byte.
            let handle = CreateFileW(
                path_w.as_ptr(),
                GENERIC_WRITE,
                0,
                &sa,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            );
            LocalFree(sd);
            if handle == INVALID_HANDLE_VALUE {
                return Err(format!("CreateFileW({}) failed", path.display()));
            }
            let mut file =
                std::fs::File::from_raw_handle(handle as std::os::windows::io::RawHandle);
            use std::io::Write;
            file.write_all(content)
                .map_err(|e| format!("write {}: {e}", path.display()))
        }
    }

    pub fn replace_dacl_owner_only(path: &Path) -> Result<(), String> {
        let sddl = protected_owner_sddl()?;
        let path_w = to_wide_nul(path.as_os_str());
        unsafe {
            let mut sd: *mut c_void = std::ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                std::ptr::null_mut(),
            ) == 0
            {
                return Err("ConvertStringSecurityDescriptorToSecurityDescriptorW failed".into());
            }
            let mut present = 0i32;
            let mut dacl: *mut c_void = std::ptr::null_mut();
            let mut defaulted = 0i32;
            let ok = GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted);
            if ok == 0 || present == 0 || dacl.is_null() {
                LocalFree(sd);
                return Err("GetSecurityDescriptorDacl failed".into());
            }
            // One call replaces the DACL with the protected owner-only one, so the file never
            // passes through a state with inherited permissions.
            let rc = SetNamedSecurityInfoW(
                path_w.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                dacl,
                std::ptr::null_mut(),
            );
            LocalFree(sd);
            if rc != 0 {
                return Err(format!(
                    "SetNamedSecurityInfoW({}) error {rc}",
                    path.display()
                ));
            }
            Ok(())
        }
    }
}

#[cfg(windows)]
fn write_windows_secret_atomic(path: &Path, content: &[u8], secret: bool) -> Result<(), String> {
    win_sec::write_secret_atomic(path, content, secret)
}

#[cfg(windows)]
fn restrict_windows_owner_only(path: &Path) -> Result<(), String> {
    win_sec::replace_dacl_owner_only(path)
}

pub fn ensure_tls_bundle(tls_dir: &Path) -> Result<TlsCertBundle, String> {
    std::fs::create_dir_all(tls_dir).map_err(|e| format!("mkdir tls_dir: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tls_dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("chmod tls_dir: {e}"))?;
    }
    #[cfg(windows)]
    restrict_windows_owner_only(tls_dir)?;

    let cert_path = tls_dir.join("cert.pem");
    let key_path = tls_dir.join("key.pem");

    let (cert_pem, key_pem) = if cert_path.is_file() && key_path.is_file() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&key_path).map_err(|e| format!("stat key.pem: {e}"))?;
            if (meta.permissions().mode() & 0o077) != 0 {
                std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
                    .map_err(|e| format!("restrict key.pem: {e}"))?;
            }
        }
        #[cfg(windows)]
        restrict_windows_owner_only(&key_path)?;
        (
            std::fs::read_to_string(&cert_path).map_err(|e| format!("read cert.pem: {e}"))?,
            std::fs::read_to_string(&key_path).map_err(|e| format!("read key.pem: {e}"))?,
        )
    } else {
        let subject_alt_names = vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "::1".to_string(),
        ];
        let certified = rcgen::generate_simple_self_signed(subject_alt_names)
            .map_err(|e| format!("rcgen error: {e}"))?;
        let cpem = certified.cert.pem();
        let kpem = certified.key_pair.serialize_pem();
        write_owner_only_file(&key_path, kpem.as_bytes(), true)?;
        write_owner_only_file(&cert_path, cpem.as_bytes(), false)?;
        (cpem, kpem)
    };

    let certs: Vec<_> = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("parse cert.pem: {e}"))?;

    let mut hasher = Sha256::new();
    if let Some(first) = certs.first() {
        hasher.update(first.as_ref());
    } else {
        hasher.update(cert_pem.as_bytes());
    }
    let cert_sha256 = format!("{:x}", hasher.finalize());

    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes()).map_err(|e| match e {
        pem::Error::NoItemsFound => "No private key found in key.pem".to_string(),
        e => format!("parse key.pem: {e}"),
    })?;

    let _ = rustls::crypto::ring::default_provider().install_default();
    let cfg = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("rustls ServerConfig: {e}"))?;

    Ok(TlsCertBundle {
        cert_path,
        key_path,
        cert_pem,
        cert_sha256,
        acceptor: TlsAcceptor::from(Arc::new(cfg)),
    })
}

pub fn write_keys_discovery_json(
    config_dir: &Path,
    bound_addr: SocketAddr,
    cert_path: &Path,
    cert_sha256: &str,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(config_dir).map_err(|e| format!("mkdir config_dir: {e}"))?;
    let keys_path = config_dir.join("keys.json");
    // `publicKeyLocation` is the certificate file. (The third-party discovery files, written by
    // `third_party`, use a directory there instead.)
    let doc = serde_json::json!({
        "openPort": bound_addr.port(),
        "host": bound_addr.ip().to_string(),
        "publicKeyLocation": cert_path.to_string_lossy(),
        "certHash": cert_sha256,
    });
    let txt = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    write_owner_only_file(&keys_path, txt.as_bytes(), true)?;
    Ok(keys_path)
}

pub async fn serve_tls_router(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    app: Router,
    running: Arc<AtomicBool>,
) {
    serve_tls_router_with_limits(
        listener,
        acceptor,
        app,
        running,
        MAX_CONCURRENT_TLS_CONNS,
        TLS_HANDSHAKE_TIMEOUT,
    )
    .await;
}

pub async fn serve_tls_router_with_limits(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    app: Router,
    running: Arc<AtomicBool>,
    max_conns: usize,
    handshake_timeout: Duration,
) {
    let conn_sem = Arc::new(Semaphore::new(max_conns));
    while running.load(Ordering::Relaxed) {
        let accept_res = tokio::time::timeout(Duration::from_millis(100), listener.accept()).await;
        let Ok(Ok((stream, _peer))) = accept_res else {
            continue;
        };
        let Ok(permit) = conn_sem.clone().try_acquire_owned() else {
            drop(stream);
            continue;
        };
        let acc = acceptor.clone();
        let svc_app = app.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let Ok(Ok(tls_stream)) =
                tokio::time::timeout(handshake_timeout, acc.accept(stream)).await
            else {
                return;
            };
            let io = TokioIo::new(tls_stream);
            let mut builder = HttpServerBuilder::new(TokioExecutor::new());
            builder
                .http1()
                .timer(TokioTimer::new())
                .header_read_timeout(handshake_timeout);
            let hyper_svc = hyper::service::service_fn(move |req: hyper::Request<Incoming>| {
                svc_app.clone().oneshot(req)
            });
            let _ = builder.serve_connection_with_upgrades(io, hyper_svc).await;
        });
    }
}
