pub const SIMPLE_HDLC_START_BYTE: u8 = 0x7E;
pub const SIMPLE_HDLC_ESCAPE_BYTE: u8 = 0x7D;
pub const SIMPLE_HDLC_ESCAPE_XOR: u8 = 0x20;
pub const SIMPLE_HDLC_MAX_PAYLOAD: usize = 8192;

const fn build_reverse_table() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut rev = 0u8;
        let mut bit = 0u8;
        while bit < 8 {
            rev = (rev << 1) | (((i as u8) >> bit) & 1);
            bit += 1;
        }
        t[i] = rev;
        i += 1;
    }
    t
}

const fn build_crc32_table() -> [u32; 256] {
    let poly: u32 = 0x04C1_1DB7;
    let mut t = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = (i as u32) << 24;
        let mut b = 0;
        while b < 8 {
            if (crc & 0x8000_0000) != 0 {
                crc = (crc << 1) ^ poly;
            } else {
                crc <<= 1;
            }
            b += 1;
        }
        t[i] = crc;
        i += 1;
    }
    t
}

static REVERSE_TABLE: [u8; 256] = build_reverse_table();
static CRC32_TABLE: [u32; 256] = build_crc32_table();

/// The frame check: CRC-32 with polynomial 0x04C11DB7, initial value and final XOR 0xFFFFFFFF,
/// each input byte bit-reversed and the result not reflected. This matches neither common
/// CRC-32 variant (CRC-32 reflects both, CRC-32/BZIP2 neither); the firmware uses the same one.
pub fn simple_hdlc_crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        let idx = (((crc >> 24) as u8) ^ REVERSE_TABLE[b as usize]) as usize;
        crc = CRC32_TABLE[idx] ^ (crc << 8);
    }
    !crc
}

/// Frames `payload`; refuses payloads longer than [`SIMPLE_HDLC_MAX_PAYLOAD`]. An empty payload
/// is framed, but [`SimpleHdlcDecoder`] discards zero-length frames.
#[allow(clippy::result_unit_err)]
pub fn simple_hdlc_encode(payload: &[u8]) -> Result<Vec<u8>, ()> {
    if payload.len() > SIMPLE_HDLC_MAX_PAYLOAD {
        return Err(());
    }
    let mut out = Vec::with_capacity(payload.len() + 16);
    out.push(SIMPLE_HDLC_START_BYTE);
    let push_esc = |o: &mut Vec<u8>, b: u8| {
        if b == SIMPLE_HDLC_START_BYTE || b == SIMPLE_HDLC_ESCAPE_BYTE {
            o.push(SIMPLE_HDLC_ESCAPE_BYTE);
            o.push(b ^ SIMPLE_HDLC_ESCAPE_XOR);
        } else {
            o.push(b);
        }
    };
    let lb = (payload.len() as u16).to_be_bytes();
    push_esc(&mut out, lb[0]);
    push_esc(&mut out, lb[1]);
    for &b in payload {
        push_esc(&mut out, b);
    }
    for b in simple_hdlc_crc32(payload).to_be_bytes() {
        push_esc(&mut out, b);
    }
    Ok(out)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RxState {
    WaitStart,
    LenHigh,
    LenLow,
    PayloadAndCrc,
}

pub struct SimpleHdlcDecoder {
    state: RxState,
    escape_next: bool,
    frame_len: usize,
    buf: Vec<u8>,
}

impl Default for SimpleHdlcDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SimpleHdlcDecoder {
    pub fn new() -> Self {
        Self {
            state: RxState::WaitStart,
            escape_next: false,
            frame_len: 0,
            buf: Vec::with_capacity(512),
        }
    }

    pub fn feed<F: FnMut(&[u8])>(&mut self, data: &[u8], mut on_frame: F) {
        for &raw_b in data {
            if raw_b == SIMPLE_HDLC_START_BYTE {
                self.state = RxState::LenHigh;
                self.escape_next = false;
                self.frame_len = 0;
                self.buf.clear();
                continue;
            }
            if self.state == RxState::WaitStart {
                continue;
            }
            let b = if self.escape_next {
                self.escape_next = false;
                raw_b ^ SIMPLE_HDLC_ESCAPE_XOR
            } else if raw_b == SIMPLE_HDLC_ESCAPE_BYTE {
                self.escape_next = true;
                continue;
            } else {
                raw_b
            };

            match self.state {
                RxState::WaitStart => {}
                RxState::LenHigh => {
                    self.frame_len = (b as usize) << 8;
                    self.state = RxState::LenLow;
                }
                RxState::LenLow => {
                    self.frame_len |= b as usize;
                    if self.frame_len == 0 || self.frame_len > SIMPLE_HDLC_MAX_PAYLOAD {
                        self.state = RxState::WaitStart;
                    } else {
                        self.buf.clear();
                        self.state = RxState::PayloadAndCrc;
                    }
                }
                RxState::PayloadAndCrc => {
                    self.buf.push(b);
                    if self.buf.len() == self.frame_len + 4 {
                        let payload = &self.buf[..self.frame_len];
                        let crc_b = &self.buf[self.frame_len..];
                        let got = u32::from_be_bytes([crc_b[0], crc_b[1], crc_b[2], crc_b[3]]);
                        if got == simple_hdlc_crc32(payload) {
                            on_frame(payload);
                        }
                        self.state = RxState::WaitStart;
                        self.buf.clear();
                    }
                }
            }
        }
    }
}
