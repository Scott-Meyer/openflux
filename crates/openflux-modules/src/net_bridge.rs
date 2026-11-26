//! Host-only opt-in sandboxed network bridge (`net.http`, `net.tcp_*`) for Lua modules that
//! explicitly declare `[permissions] network = true` in `module.toml`.
//!
//! Strictly compiled only when the `host-net` feature is enabled (in `openfluxd`, NEVER on
//! `openflux-display`) and bound ONLY when `ModuleRuntime::with_host_network(true)` is set AND
//! the module manifest declares `permissions.network = true`.

#[cfg(feature = "host-net")]
use base64::Engine;
#[cfg(feature = "host-net")]
use mlua::{Lua, MultiValue, Table, Value as LuaValue};
#[cfg(feature = "host-net")]
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
#[cfg(feature = "host-net")]
use std::collections::HashMap;
#[cfg(feature = "host-net")]
use std::io::{self, Read, Write};
#[cfg(feature = "host-net")]
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
#[cfg(feature = "host-net")]
use std::sync::{Arc, Mutex};
#[cfg(feature = "host-net")]
use std::time::{Duration, Instant};

#[cfg(feature = "host-net")]
const MAX_TCP_CONNS_PER_MODULE: usize = 16;
#[cfg(feature = "host-net")]
const MAX_HTTP_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[cfg(feature = "host-net")]
enum ManagedSocket {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

#[cfg(feature = "host-net")]
impl ManagedSocket {
    fn shutdown_cleanly(&mut self) {
        match self {
            ManagedSocket::Plain(s) => {
                let _ = s.shutdown(Shutdown::Both);
            }
            ManagedSocket::Tls(tls) => {
                tls.conn.send_close_notify();
                let _ = tls.flush();
                let _ = tls.sock.shutdown(Shutdown::Both);
            }
        }
    }

    fn write_all_bounded(&mut self, data: &[u8], timeout: Duration) -> io::Result<usize> {
        let deadline = Instant::now() + timeout;
        let mut written = 0usize;
        while written < data.len() {
            let step = match self {
                ManagedSocket::Plain(s) => s.write(&data[written..]),
                ManagedSocket::Tls(s) => s.write(&data[written..]),
            };
            match step {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "connection closed")),
                Ok(n) => written += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "send timed out"));
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(e) => return Err(e),
            }
        }
        let _ = match self {
            ManagedSocket::Plain(s) => s.flush(),
            ManagedSocket::Tls(s) => s.flush(),
        };
        Ok(written)
    }

    fn read_nonblocking(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            ManagedSocket::Plain(s) => s.read(buf),
            ManagedSocket::Tls(s) => s.read(buf),
        }
    }
}

/// Owns all open sockets for one loaded Lua module; `Drop` cleanly shuts down every socket so
/// broker Last-Will messages fire immediately when a module unloads, reloads, or is disabled.
#[derive(Default)]
pub struct ModuleNetState {
    #[cfg(feature = "host-net")]
    next_id: u64,
    #[cfg(feature = "host-net")]
    sockets: HashMap<u64, ManagedSocket>,
}

impl ModuleNetState {
    pub fn close_all(&mut self) {
        #[cfg(feature = "host-net")]
        {
            for (_, mut sock) in self.sockets.drain() {
                sock.shutdown_cleanly();
            }
        }
    }
}

impl Drop for ModuleNetState {
    fn drop(&mut self) {
        self.close_all();
    }
}

#[cfg(feature = "host-net")]
#[derive(Debug)]
struct InsecureNoCertVerifier(Arc<rustls::crypto::CryptoProvider>);

#[cfg(feature = "host-net")]
impl rustls::client::danger::ServerCertVerifier for InsecureNoCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(feature = "host-net")]
fn build_rustls_client_config(verify: bool) -> Result<Arc<rustls::ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    if !verify {
        let cfg = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| format!("tls config: {e}"))?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(InsecureNoCertVerifier(provider)))
            .with_no_client_auth();
        return Ok(Arc::new(cfg));
    }
    let mut roots = rustls::RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for cert in native.certs {
        let _ = roots.add(cert);
    }
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("tls config: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

#[cfg(feature = "host-net")]
fn lua_err_pair(lua: &Lua, msg: &str) -> mlua::Result<MultiValue> {
    Ok(MultiValue::from_vec(vec![
        LuaValue::Nil,
        LuaValue::String(lua.create_string(msg)?),
    ]))
}

#[cfg(feature = "host-net")]
pub fn install_net_table(
    lua: &Lua,
    net_state: Arc<Mutex<ModuleNetState>>,
) -> mlua::Result<()> {
    let net_tbl = lua.create_table()?;

    // `net.http({ method?, url, headers?, body?, timeout_ms?, verify? })`
    net_tbl.set(
        "http",
        lua.create_function(|lua, req: Table| {
            let url: String = req.get("url").unwrap_or_default();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                let res = lua.create_table()?;
                res.set("ok", false)?;
                res.set("status", 0)?;
                res.set("error", "only http:// and https:// URLs are permitted")?;
                return Ok(res);
            }
            let method_str: String = req
                .get::<Option<String>>("method")?
                .unwrap_or_else(|| "GET".into());
            let timeout_ms: u64 = req
                .get::<Option<u64>>("timeout_ms")?
                .unwrap_or(5000)
                .clamp(250, 30_000);
            let verify: bool = req.get::<Option<bool>>("verify")?.unwrap_or(true);
            let body_opt: Option<mlua::String> = req.get("body").ok();

            let mut builder = reqwest::blocking::Client::builder()
                .timeout(Duration::from_millis(timeout_ms));
            if !verify {
                builder = builder.danger_accept_invalid_certs(true);
            }
            let client = match builder.build() {
                Ok(c) => c,
                Err(e) => {
                    let res = lua.create_table()?;
                    res.set("ok", false)?;
                    res.set("status", 0)?;
                    res.set("error", format!("http client: {e}"))?;
                    return Ok(res);
                }
            };
            let method = reqwest::Method::from_bytes(method_str.trim().to_ascii_uppercase().as_bytes())
                .unwrap_or(reqwest::Method::GET);
            let mut rb = client.request(method, &url);
            if let Ok(hdrs) = req.get::<Table>("headers") {
                for pair in hdrs.pairs::<String, String>().flatten() {
                    rb = rb.header(pair.0, pair.1);
                }
            }
            if let Some(b) = body_opt {
                rb = rb.body(b.as_bytes().to_vec());
            }

            let out = lua.create_table()?;
            match rb.send() {
                Ok(mut resp) => {
                    let status = resp.status().as_u16();
                    let resp_hdrs = lua.create_table()?;
                    for (k, v) in resp.headers() {
                        if let Ok(vs) = v.to_str() {
                            let _ = resp_hdrs.set(k.as_str().to_ascii_lowercase(), vs);
                        }
                    }
                    let mut buf = Vec::new();
                    let mut limited = resp.by_ref().take((MAX_HTTP_RESPONSE_BYTES + 1) as u64);
                    if let Err(e) = limited.read_to_end(&mut buf) {
                        out.set("ok", false)?;
                        out.set("status", status)?;
                        out.set("error", format!("read body: {e}"))?;
                        return Ok(out);
                    }
                    if buf.len() > MAX_HTTP_RESPONSE_BYTES {
                        out.set("ok", false)?;
                        out.set("status", status)?;
                        out.set("error", "response exceeded 4 MB limit")?;
                        return Ok(out);
                    }
                    out.set("ok", (200..300).contains(&status))?;
                    out.set("status", status)?;
                    out.set("headers", resp_hdrs)?;
                    out.set("body", lua.create_string(&buf)?)?;
                    out.set(
                        "body_b64",
                        base64::engine::general_purpose::STANDARD.encode(&buf),
                    )?;
                }
                Err(e) => {
                    out.set("ok", false)?;
                    out.set("status", 0)?;
                    out.set("error", e.to_string())?;
                }
            }
            Ok(out)
        })?,
    )?;

    // `net.tcp_connect(host, port, opts?)` -> `id` OR `nil, err`
    {
        let st = net_state.clone();
        net_tbl.set(
            "tcp_connect",
            lua.create_function(move |lua, (host, port, opts): (String, u16, Option<Table>)| {
                let mut use_tls = false;
                let mut verify = true;
                let mut server_name: Option<String> = None;
                let mut timeout_ms: u64 = 5000;
                if let Some(o) = opts {
                    use_tls = o.get::<Option<bool>>("tls")?.unwrap_or(false);
                    if let Some(v) = o.get::<Option<bool>>("verify")? {
                        verify = v;
                    } else if o.get::<Option<bool>>("tls_insecure")?.unwrap_or(false) {
                        verify = false;
                    }
                    server_name = o
                        .get::<Option<String>>("server_name")?
                        .filter(|s| !s.trim().is_empty());
                    timeout_ms = o
                        .get::<Option<u64>>("timeout_ms")?
                        .unwrap_or(5000)
                        .clamp(250, 30_000);
                }

                {
                    let g = st.lock().unwrap();
                    if g.sockets.len() >= MAX_TCP_CONNS_PER_MODULE {
                        return lua_err_pair(lua, "too many open TCP connections for this module");
                    }
                }

                let addr_str = format!("{}:{}", host.trim(), port);
                let addrs: Vec<std::net::SocketAddr> = match addr_str.to_socket_addrs() {
                    Ok(it) => it.collect(),
                    Err(e) => return lua_err_pair(lua, &format!("resolve {addr_str}: {e}")),
                };
                let Some(first_addr) = addrs.first().copied() else {
                    return lua_err_pair(lua, &format!("no addresses resolved for {addr_str}"));
                };
                let timeout = Duration::from_millis(timeout_ms);
                let mut last_err = None;
                let mut connected_sock = None;
                for addr in addrs.iter().copied().chain(std::iter::once(first_addr)).take(addrs.len()) {
                    match TcpStream::connect_timeout(&addr, timeout) {
                        Ok(s) => {
                            connected_sock = Some(s);
                            break;
                        }
                        Err(e) => last_err = Some(e),
                    }
                }
                let Some(sock) = connected_sock else {
                    let msg = last_err
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "connection failed".into());
                    return lua_err_pair(lua, &msg);
                };
                let _ = sock.set_nodelay(true);

                let managed = if use_tls {
                    let _ = sock.set_read_timeout(Some(timeout));
                    let _ = sock.set_write_timeout(Some(timeout));
                    let cfg = match build_rustls_client_config(verify) {
                        Ok(c) => c,
                        Err(e) => return lua_err_pair(lua, &e),
                    };
                    let sni_raw = server_name.unwrap_or_else(|| host.trim().to_string());
                    let sni = match ServerName::try_from(sni_raw.clone()) {
                        Ok(s) => s,
                        Err(_) => return lua_err_pair(lua, &format!("tls: invalid server_name {sni_raw:?}")),
                    };
                    let conn = match rustls::ClientConnection::new(cfg, sni) {
                        Ok(c) => c,
                        Err(e) => return lua_err_pair(lua, &format!("tls: {e}")),
                    };
                    let mut stream = rustls::StreamOwned::new(conn, sock);
                    while stream.conn.is_handshaking() {
                        if let Err(e) = stream.conn.complete_io(&mut stream.sock) {
                            return lua_err_pair(lua, &format!("tls: {e}"));
                        }
                    }
                    let _ = stream.sock.set_read_timeout(None);
                    let _ = stream.sock.set_write_timeout(None);
                    let _ = stream.sock.set_nonblocking(true);
                    ManagedSocket::Tls(Box::new(stream))
                } else {
                    let _ = sock.set_nonblocking(true);
                    ManagedSocket::Plain(sock)
                };

                let mut g = st.lock().unwrap();
                g.next_id = g.next_id.wrapping_add(1).max(1);
                let id = g.next_id;
                g.sockets.insert(id, managed);
                Ok(MultiValue::from_vec(vec![LuaValue::Integer(id as i64)]))
            })?,
        )?;
    }

    // `net.tcp_send(id, bytes)` -> `bytes_written` OR `nil, err`
    {
        let st = net_state.clone();
        net_tbl.set(
            "tcp_send",
            lua.create_function(move |lua, (id, data): (u64, mlua::String)| {
                let mut g = st.lock().unwrap();
                let Some(sock) = g.sockets.get_mut(&id) else {
                    return lua_err_pair(lua, "closed");
                };
                match sock.write_all_bounded(&data.as_bytes(), Duration::from_millis(200)) {
                    Ok(n) => Ok(MultiValue::from_vec(vec![LuaValue::Integer(n as i64)])),
                    Err(e) => {
                        let msg = e.to_string();
                        if let Some(mut dead) = g.sockets.remove(&id) {
                            dead.shutdown_cleanly();
                        }
                        lua_err_pair(lua, &msg)
                    }
                }
            })?,
        )?;
    }

    // `net.tcp_recv(id, max_bytes?)` -> `bytes` | `nil` (nothing yet) | `nil, "closed"` | `nil, err`
    {
        let st = net_state.clone();
        net_tbl.set(
            "tcp_recv",
            lua.create_function(move |lua, (id, max_bytes): (u64, Option<usize>)| {
                let cap = max_bytes.unwrap_or(8192).clamp(64, 65536);
                let mut buf = vec![0u8; cap];
                let mut g = st.lock().unwrap();
                let Some(sock) = g.sockets.get_mut(&id) else {
                    return lua_err_pair(lua, "closed");
                };
                match sock.read_nonblocking(&mut buf) {
                    Ok(0) => {
                        if let Some(mut dead) = g.sockets.remove(&id) {
                            dead.shutdown_cleanly();
                        }
                        lua_err_pair(lua, "closed")
                    }
                    Ok(n) => Ok(MultiValue::from_vec(vec![LuaValue::String(
                        lua.create_string(&buf[..n])?,
                    )])),
                    Err(e)
                        if e.kind() == io::ErrorKind::WouldBlock
                            || e.kind() == io::ErrorKind::Interrupted =>
                    {
                        Ok(MultiValue::from_vec(vec![LuaValue::Nil]))
                    }
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                        if let Some(mut dead) = g.sockets.remove(&id) {
                            dead.shutdown_cleanly();
                        }
                        lua_err_pair(lua, "closed")
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        if let Some(mut dead) = g.sockets.remove(&id) {
                            dead.shutdown_cleanly();
                        }
                        lua_err_pair(lua, &msg)
                    }
                }
            })?,
        )?;
    }

    // `net.tcp_close(id)`
    {
        let st = net_state.clone();
        net_tbl.set(
            "tcp_close",
            lua.create_function(move |_, id: u64| {
                let mut g = st.lock().unwrap();
                if let Some(mut sock) = g.sockets.remove(&id) {
                    sock.shutdown_cleanly();
                }
                Ok(())
            })?,
        )?;
    }

    lua.globals().set("net", net_tbl)?;
    Ok(())
}
