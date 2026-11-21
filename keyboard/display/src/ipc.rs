#[cfg(unix)]
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;

const MAX_IPC_LINE_BYTES: usize = 64 * 1024;

pub struct LineStreamBuffer {
    buf: Vec<u8>,
    skip_until_newline: bool,
}

impl Default for LineStreamBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl LineStreamBuffer {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(4096),
            skip_until_newline: false,
        }
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.skip_until_newline = false;
    }

    /// Feeds raw bytes (including partial records or split multi-byte UTF-8 sequences) and returns
    /// every complete `\n`-terminated line, enforcing `MAX_IPC_LINE_BYTES` per record.
    pub fn push_bytes(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for part in chunk.split_inclusive(|&b| b == b'\n') {
            let ends_nl = part.ends_with(b"\n");
            if self.skip_until_newline {
                if ends_nl {
                    self.skip_until_newline = false;
                }
                continue;
            }
            let record_len = if ends_nl { part.len() - 1 } else { part.len() };
            if self.buf.len() + record_len > MAX_IPC_LINE_BYTES {
                self.buf.clear();
                if !ends_nl {
                    self.skip_until_newline = true;
                }
                continue;
            }
            if ends_nl {
                let trimmed = if self.buf.is_empty() {
                    String::from_utf8_lossy(&part[..record_len])
                        .trim()
                        .to_string()
                } else {
                    self.buf.extend_from_slice(&part[..record_len]);
                    let s = String::from_utf8_lossy(&self.buf).trim().to_string();
                    self.buf.clear();
                    s
                };
                if !trimmed.is_empty() {
                    lines.push(trimmed);
                }
            } else {
                self.buf.extend_from_slice(part);
            }
        }
        lines
    }
}

#[cfg(unix)]
pub struct BridgeIpcClient {
    stream: Option<UnixStream>,
    line_buf: LineStreamBuffer,
}

#[cfg(unix)]
impl Default for BridgeIpcClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(unix)]
impl BridgeIpcClient {
    pub fn new() -> Self {
        Self {
            stream: None,
            line_buf: LineStreamBuffer::new(),
        }
    }

    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    pub fn raw_fd(&self) -> Option<i32> {
        use std::os::unix::io::AsRawFd;
        self.stream.as_ref().map(|s| s.as_raw_fd())
    }

    pub fn connect(&mut self, sock_path: &str) -> bool {
        if self.stream.is_some() {
            return true;
        }
        if let Ok(mut s) = UnixStream::connect(sock_path) {
            let _ = s.set_nonblocking(true);
            let _ = s.write_all(b"{\"cmd\":\"get_state\"}\n{\"cmd\":\"set_analog\",\"enabled\":true}\n");
            self.line_buf.clear();
            self.stream = Some(s);
            true
        } else {
            false
        }
    }

    pub fn poll_lines(&mut self) -> Vec<String> {
        let Some(ref mut s) = self.stream else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut tmp = [0u8; 4096];
        let mut disconnected = false;
        loop {
            match s.read(&mut tmp) {
                Ok(0) => {
                    disconnected = true;
                    break;
                }
                Ok(n) => {
                    out.extend(self.line_buf.push_bytes(&tmp[..n]));
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if disconnected {
            self.stream = None;
            self.line_buf.clear();
        }
        out
    }
}
