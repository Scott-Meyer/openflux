//! RP2040 UF2 parsing, the LittleFS guard, and PICOBOOT flashing.
//!
//! A UF2 image is accepted only if it:
//!   - uses 256-byte payloads at 256-byte-aligned addresses,
//!   - is a complete sequence of unique blocks (block_no == idx, num_blocks == total, no overlaps),
//!   - writes nothing at or above 0x10F00000 (the LittleFS calibration and keymap region).
//!
//! An accepted image becomes a flash plan of whole 4 KiB sectors.

use std::collections::BTreeMap;

/// A boot loader device and its USB (bus, port path).
type PicobootCandidate = (rusb::Device<rusb::Context>, (u8, Vec<u8>));

pub const RP2040_FLASH_BASE: u32 = 0x1000_0000;
pub const RP2040_LFS_OFFSET: u32 = 0x00F0_0000;
pub const RP2040_LFS_START: u32 = RP2040_FLASH_BASE + RP2040_LFS_OFFSET;
pub const RP2040_LFS_SIZE: u32 = 1024 * 1024;
pub const RP2040_FAMILY_ID: u32 = 0xE48B_FF56;
/// The error a flash returns when the calibration region's contents differ after the write. Only
/// this error means the calibration may have changed; every other one leaves it as it was.
pub const CALIBRATION_CHANGED: &str = "The LittleFS calibration partition changed during the flash";

const UF2_MAGIC_START0: u32 = 0x0A32_4655;
const UF2_MAGIC_START1: u32 = 0x9E5D_5157;
const UF2_MAGIC_END: u32 = 0x0AB1_6F30;

pub const PICOBOOT_MAGIC: u32 = 0x431F_D10B;
pub const PC_EXCLUSIVE_ACCESS: u8 = 0x01;
pub const PC_REBOOT: u8 = 0x02;
pub const PC_FLASH_ERASE: u8 = 0x03;
pub const PC_READ: u8 = 0x84;
pub const PC_WRITE: u8 = 0x05;
pub const PC_EXIT_XIP: u8 = 0x06;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uf2FlashPlan {
    pub flash_offset: u32,
    pub image: Vec<u8>,
}

pub fn parse_uf2_image(uf2_bytes: &[u8]) -> Result<Uf2FlashPlan, String> {
    if uf2_bytes.is_empty() || !uf2_bytes.len().is_multiple_of(512) {
        return Err(format!(
            "UF2 size {} is not a positive multiple of 512",
            uf2_bytes.len()
        ));
    }
    let nblocks = uf2_bytes.len() / 512;
    let mut pages: BTreeMap<u32, [u8; 256]> = BTreeMap::new();

    for (idx, blk) in uf2_bytes.chunks_exact(512).enumerate() {
        let m0 = u32::from_le_bytes([blk[0], blk[1], blk[2], blk[3]]);
        let m1 = u32::from_le_bytes([blk[4], blk[5], blk[6], blk[7]]);
        let flags = u32::from_le_bytes([blk[8], blk[9], blk[10], blk[11]]);
        let target_addr = u32::from_le_bytes([blk[12], blk[13], blk[14], blk[15]]);
        let payload_sz = u32::from_le_bytes([blk[16], blk[17], blk[18], blk[19]]);
        let block_no = u32::from_le_bytes([blk[20], blk[21], blk[22], blk[23]]);
        let num_blocks = u32::from_le_bytes([blk[24], blk[25], blk[26], blk[27]]);
        let family_id = u32::from_le_bytes([blk[28], blk[29], blk[30], blk[31]]);
        let mend = u32::from_le_bytes([blk[508], blk[509], blk[510], blk[511]]);

        if m0 != UF2_MAGIC_START0 || m1 != UF2_MAGIC_START1 || mend != UF2_MAGIC_END {
            return Err(format!("Invalid UF2 magic in block {idx}"));
        }
        if (flags & 0x0000_0001) != 0 {
            return Err(format!("Unexpected non-flash UF2 flag in block {idx}"));
        }
        if (flags & 0x0000_2000) != 0 && family_id != RP2040_FAMILY_ID {
            return Err(format!(
                "Unexpected UF2 family_id 0x{family_id:08x} in block {idx}"
            ));
        }
        if payload_sz != 256 || (target_addr % 256) != 0 {
            return Err(format!(
                "Unexpected UF2 page size/align in block {idx}: size={payload_sz}, addr=0x{target_addr:08x}"
            ));
        }
        if block_no as usize != idx || num_blocks as usize != nblocks {
            return Err(format!(
                "Incomplete/out-of-sequence UF2 block {block_no}/{num_blocks} at index {idx}/{nblocks}"
            ));
        }
        let end_addr = target_addr
            .checked_add(256)
            .ok_or_else(|| "UF2 address overflow".to_string())?;
        if target_addr < RP2040_FLASH_BASE {
            return Err(format!(
                "UF2 block {idx} [0x{target_addr:08x}..0x{end_addr:08x}) is outside flash, which starts at 0x{RP2040_FLASH_BASE:08x}"
            ));
        }
        if end_addr > RP2040_LFS_START {
            return Err(format!(
                "UF2 block {idx} [0x{target_addr:08x}..0x{end_addr:08x}) overlaps protected LittleFS calibration/keymap region >= 0x{RP2040_LFS_START:08x}"
            ));
        }
        let off = target_addr - RP2040_FLASH_BASE;
        if pages.contains_key(&off) {
            return Err(format!(
                "Duplicate/overlapping UF2 target address 0x{target_addr:08x} at block {idx}"
            ));
        }
        let mut page = [0u8; 256];
        page.copy_from_slice(&blk[32..32 + 256]);
        pages.insert(off, page);
    }

    if pages.is_empty() {
        return Err("UF2 contains zero flashable blocks".into());
    }

    let min_off = pages.keys().next().copied().unwrap() & !0xFFF;
    let max_end = (pages.keys().next_back().copied().unwrap() + 256 + 0xFFF) & !0xFFF;
    if max_end > RP2040_LFS_OFFSET {
        return Err(format!(
            "Sector-aligned flash plan end 0x{max_end:08x} overlaps LittleFS >= 0x{RP2040_LFS_OFFSET:08x}"
        ));
    }

    let mut buf = vec![0xFFu8; (max_end - min_off) as usize];
    for (off, pdata) in pages {
        let pos = (off - min_off) as usize;
        buf[pos..pos + 256].copy_from_slice(&pdata);
    }
    Ok(Uf2FlashPlan {
        flash_offset: min_off,
        image: buf,
    })
}

pub fn build_picoboot_cmd(cmd_id: u8, token: u32, transfer_len: u32, args: &[u8]) -> [u8; 32] {
    let mut pkt = [0u8; 32];
    pkt[0..4].copy_from_slice(&PICOBOOT_MAGIC.to_le_bytes());
    pkt[4..8].copy_from_slice(&token.to_le_bytes());
    pkt[8] = cmd_id;
    pkt[9] = args.len().min(16) as u8;
    pkt[12..16].copy_from_slice(&transfer_len.to_le_bytes());
    let n = args.len().min(16);
    pkt[16..16 + n].copy_from_slice(&args[..n]);
    pkt
}

pub const RP2040_PICOBOOT_VID: u16 = 0x2E8A;
pub const RP2040_PICOBOOT_PID: u16 = 0x0003;

struct PicobootSession {
    handle: rusb::DeviceHandle<rusb::Context>,
    ep_out: u8,
    ep_in: u8,
    intf: u8,
    token: u32,
}

pub fn select_picoboot_for_firmware_path(
    firmware_path: (u8, &[u8]),
    picoboot_candidates: &[(u8, Vec<u8>)],
) -> Result<usize, String> {
    if firmware_path.1.is_empty() {
        return Err(
            "USB port path of the keyboard firmware is unavailable; refusing to flash".into(),
        );
    }
    let matches: Vec<usize> = picoboot_candidates
        .iter()
        .enumerate()
        .filter(|(_, (bus, ports))| {
            !ports.is_empty() && *bus == firmware_path.0 && ports.as_slice() == firmware_path.1
        })
        .map(|(idx, _)| idx)
        .collect();
    match matches.len() {
        1 => Ok(matches[0]),
        0 => Err(format!(
            "No RP2040 boot loader (VID 0x{RP2040_PICOBOOT_VID:04x}, PID 0x{RP2040_PICOBOOT_PID:04x}) at the keyboard's USB path bus={} ports={:?}; other BOOTSEL devices are left alone",
            firmware_path.0, firmware_path.1
        )),
        _ => Err(format!(
            "More than one RP2040 boot loader at the keyboard's USB path bus={} ports={:?}",
            firmware_path.0, firmware_path.1
        )),
    }
}

pub fn parse_picoboot_status(buf: &[u8], expected_token: u32) -> Result<(), String> {
    if buf.len() < 16 {
        return Err(format!("Short PICOBOOT status response ({} B)", buf.len()));
    }
    let tok = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let status = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
    let cmd_id = buf[8];
    let in_progress = buf[9];
    if tok != expected_token {
        return Err(format!(
            "PICOBOOT status token mismatch: got 0x{tok:08x}, expected 0x{expected_token:08x}"
        ));
    }
    if status != 0 {
        return Err(format!(
            "PICOBOOT ROM command 0x{cmd_id:02x} failed with status={status} (in_progress={in_progress})"
        ));
    }
    Ok(())
}

impl PicobootSession {
    fn open_at_path(ctx: &rusb::Context, fw_bus: u8, fw_ports: &[u8]) -> Result<Self, String> {
        use rusb::UsbContext;
        if fw_ports.is_empty() {
            return Err(
                "USB port path of the keyboard firmware is unavailable; refusing to flash".into(),
            );
        }
        let devs = ctx.devices().map_err(|e| format!("usb list: {e}"))?;
        let mut candidates: Vec<PicobootCandidate> = Vec::new();
        for dev in devs.iter() {
            let Ok(desc) = dev.device_descriptor() else {
                continue;
            };
            if desc.vendor_id() == RP2040_PICOBOOT_VID && desc.product_id() == RP2040_PICOBOOT_PID {
                let Ok(ports) = dev.port_numbers() else {
                    continue;
                };
                if ports.is_empty() {
                    continue;
                }
                let bus = dev.bus_number();
                candidates.push((dev, (bus, ports)));
            }
        }
        let paths: Vec<(u8, Vec<u8>)> = candidates.iter().map(|(_, p)| p.clone()).collect();
        let chosen_idx = select_picoboot_for_firmware_path((fw_bus, fw_ports), &paths)?;
        let dev = &candidates[chosen_idx].0;
        let cfg = dev
            .active_config_descriptor()
            .map_err(|e| format!("picoboot cfg: {e}"))?;
        for intf in cfg.interfaces() {
            for idesc in intf.descriptors() {
                if idesc.class_code() == 0xFF {
                    let mut ep_out = None;
                    let mut ep_in = None;
                    for ep in idesc.endpoint_descriptors() {
                        if ep.transfer_type() == rusb::TransferType::Bulk {
                            match ep.direction() {
                                rusb::Direction::Out => ep_out = Some(ep.address()),
                                rusb::Direction::In => ep_in = Some(ep.address()),
                            }
                        }
                    }
                    if let (Some(eo), Some(ei)) = (ep_out, ep_in) {
                        let handle = dev.open().map_err(|e| format!("picoboot open: {e}"))?;
                        let _ = handle.set_auto_detach_kernel_driver(true);
                        handle
                            .claim_interface(idesc.interface_number())
                            .map_err(|e| format!("picoboot claim: {e}"))?;
                        return Ok(Self {
                            handle,
                            ep_out: eo,
                            ep_in: ei,
                            intf: idesc.interface_number(),
                            token: 1,
                        });
                    }
                }
            }
        }
        Err("RP2040 PICOBOOT vendor interface not found on bound USB path".into())
    }

    fn reset_endpoints(&self) {
        let _ = self.handle.clear_halt(self.ep_out);
        let _ = self.handle.clear_halt(self.ep_in);
        let _ = self.handle.write_control(
            0x41,
            0x41,
            0,
            self.intf as u16,
            &[],
            std::time::Duration::from_secs(2),
        );
    }

    fn cmd(
        &mut self,
        cmd_id: u8,
        args: &[u8],
        data_out: &[u8],
        data_in_len: usize,
    ) -> Result<Vec<u8>, String> {
        self.token = self.token.wrapping_add(1);
        let xfer_len = if !data_out.is_empty() {
            data_out.len() as u32
        } else {
            data_in_len as u32
        };
        let pkt = build_picoboot_cmd(cmd_id, self.token, xfer_len, args);
        let tmo = std::time::Duration::from_secs(10);
        self.handle
            .write_bulk(self.ep_out, &pkt, tmo)
            .map_err(|e| format!("picoboot cmd 0x{cmd_id:02x} write: {e}"))?;
        let mut out = Vec::new();
        if !data_out.is_empty() {
            let mut off = 0;
            while off < data_out.len() {
                let n = self
                    .handle
                    .write_bulk(self.ep_out, &data_out[off..], tmo)
                    .map_err(|e| format!("picoboot data_out 0x{cmd_id:02x}: {e}"))?;
                if n == 0 {
                    return Err("picoboot zero-byte bulk write".into());
                }
                off += n;
            }
            let mut zlp = [0u8; 64];
            let _ = self
                .handle
                .read_bulk(self.ep_in, &mut zlp, tmo)
                .map_err(|e| format!("picoboot zlp in: {e}"))?;
        } else if data_in_len > 0 {
            out.resize(data_in_len, 0);
            let mut off = 0;
            while off < data_in_len {
                let n = self
                    .handle
                    .read_bulk(self.ep_in, &mut out[off..], tmo)
                    .map_err(|e| format!("picoboot data_in 0x{cmd_id:02x}: {e}"))?;
                if n == 0 {
                    return Err("picoboot short read".into());
                }
                off += n;
            }
            let _ = self
                .handle
                .write_bulk(self.ep_out, &[], tmo)
                .map_err(|e| format!("picoboot zlp out: {e}"))?;
        } else if (cmd_id & 0x80) != 0 {
            let _ = self
                .handle
                .write_bulk(self.ep_out, &[], tmo)
                .map_err(|e| format!("picoboot zlp out: {e}"))?;
        } else {
            let mut zlp = [0u8; 64];
            let _ = self
                .handle
                .read_bulk(self.ep_in, &mut zlp, tmo)
                .map_err(|e| format!("picoboot zlp in: {e}"))?;
        }
        if cmd_id != PC_REBOOT {
            let mut st_buf = [0u8; 16];
            let n = self
                .handle
                .read_control(0xC1, 0x42, 0, self.intf as u16, &mut st_buf, tmo)
                .map_err(|e| format!("picoboot get_command_status(0x{cmd_id:02x}): {e}"))?;
            parse_picoboot_status(&st_buf[..n], self.token)?;
        }
        Ok(out)
    }

    fn read_flash(&mut self, addr: u32, size: u32) -> Result<Vec<u8>, String> {
        let mut out = Vec::with_capacity(size as usize);
        let mut off = 0u32;
        while off < size {
            let chunk = (size - off).min(65_536);
            let mut args = [0u8; 8];
            args[0..4].copy_from_slice(&(addr + off).to_le_bytes());
            args[4..8].copy_from_slice(&chunk.to_le_bytes());
            out.extend_from_slice(&self.cmd(PC_READ, &args, &[], chunk as usize)?);
            off += chunk;
        }
        Ok(out)
    }

    fn flash_erase(&mut self, addr: u32, size: u32) -> Result<(), String> {
        let mut args = [0u8; 8];
        args[0..4].copy_from_slice(&addr.to_le_bytes());
        args[4..8].copy_from_slice(&size.to_le_bytes());
        let _ = self.cmd(PC_FLASH_ERASE, &args, &[], 0)?;
        Ok(())
    }

    fn write_flash(&mut self, addr: u32, data: &[u8]) -> Result<(), String> {
        let mut off = 0usize;
        while off < data.len() {
            let chunk = (data.len() - off).min(4096);
            let mut args = [0u8; 8];
            args[0..4].copy_from_slice(&(addr + off as u32).to_le_bytes());
            args[4..8].copy_from_slice(&(chunk as u32).to_le_bytes());
            let _ = self.cmd(PC_WRITE, &args, &data[off..off + chunk], 0)?;
            off += chunk;
        }
        Ok(())
    }
}

impl Drop for PicobootSession {
    fn drop(&mut self) {
        let _ = self.handle.release_interface(self.intf);
    }
}

pub fn flash_uf2_live(
    uf2_bytes: &[u8],
    enter_bootloader: impl FnOnce() -> Result<(u8, Vec<u8>), String>,
) -> Result<String, String> {
    use rusb::UsbContext;
    use sha2::{Digest, Sha256};

    let plan = parse_uf2_image(uf2_bytes)?;
    let expected_sha = format!("{:x}", Sha256::digest(&plan.image));
    let ctx = rusb::Context::new().map_err(|e| format!("libusb init: {e}"))?;

    // Snapshot any pre-existing RPI-RP2 volumes BEFORE rebooting the keyboard into BOOTSEL so
    // the Windows mass-storage fallback NEVER flashes an unrelated Raspberry Pi Pico!
    #[cfg(target_os = "windows")]
    let pre_existing_rp2 = snapshot_windows_rp2_volumes();

    // The keyboard firmware (3761:4015) is rebooted into its boot loader and flashed at the same USB
    // bus and port path, so any other RP2040 in BOOTSEL mode is never touched.
    let (fw_bus, fw_ports) = enter_bootloader()?;
    let is_bound_picoboot_present = |c: &rusb::Context| -> bool {
        let Ok(devs) = c.devices() else { return false };
        let paths: Vec<(u8, Vec<u8>)> = devs
            .iter()
            .filter_map(|d| {
                let dd = d.device_descriptor().ok()?;
                if dd.vendor_id() == RP2040_PICOBOOT_VID && dd.product_id() == RP2040_PICOBOOT_PID {
                    Some((d.bus_number(), d.port_numbers().unwrap_or_default()))
                } else {
                    None
                }
            })
            .collect();
        select_picoboot_for_firmware_path((fw_bus, &fw_ports), &paths).is_ok()
    };

    let boot_deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
    while std::time::Instant::now() < boot_deadline && !is_bound_picoboot_present(&ctx) {
        std::thread::sleep(std::time::Duration::from_millis(120));
    }

    let write_addr = RP2040_FLASH_BASE + plan.flash_offset;
    let lfs_sha = match PicobootSession::open_at_path(&ctx, fw_bus, &fw_ports) {
        Ok(mut pb) => {
            let res = (|| -> Result<String, String> {
                let _ = pb.cmd(PC_EXCLUSIVE_ACCESS, &[2], &[], 0)?;
                let _ = pb.cmd(PC_EXIT_XIP, &[], &[], 0)?;
                let lfs_before = format!(
                    "{:x}",
                    Sha256::digest(pb.read_flash(RP2040_LFS_START, RP2040_LFS_SIZE)?)
                );
                pb.flash_erase(write_addr, plan.image.len() as u32)?;
                pb.write_flash(write_addr, &plan.image)?;
                let verify_sha = format!(
                    "{:x}",
                    Sha256::digest(pb.read_flash(write_addr, plan.image.len() as u32)?)
                );
                if verify_sha != expected_sha {
                    return Err("PICOBOOT read-back after the write doesn't match the image".into());
                }
                let lfs_after = format!(
                    "{:x}",
                    Sha256::digest(pb.read_flash(RP2040_LFS_START, RP2040_LFS_SIZE)?)
                );
                if lfs_after != lfs_before {
                    return Err(CALIBRATION_CHANGED.into());
                }
                let mut rb_args = [0u8; 12];
                rb_args[8..12].copy_from_slice(&100u32.to_le_bytes());
                let _ = pb.cmd(PC_REBOOT, &rb_args, &[], 0);
                Ok(lfs_after)
            })();
            let sha = match res {
                Ok(s) => s,
                Err(e) => {
                    pb.reset_endpoints();
                    return Err(e);
                }
            };
            drop(pb);
            sha
        }
        Err(picoboot_err) => {
            #[cfg(target_os = "windows")]
            {
                flash_uf2_via_windows_rp2_volume(uf2_bytes, &picoboot_err, &pre_existing_rp2)?
            }
            #[cfg(not(target_os = "windows"))]
            {
                return Err(picoboot_err);
            }
        }
    };

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while std::time::Instant::now() < deadline {
        if let Ok(devs) = ctx.devices() {
            if devs.iter().any(|d| {
                d.device_descriptor()
                    .map(|dd| {
                        dd.vendor_id() == crate::usb::protocol::KEYBOARD_VID
                            && dd.product_id() == crate::usb::protocol::FIRMWARE_PID
                            && d.bus_number() == fw_bus
                            && d.port_numbers().unwrap_or_default() == fw_ports
                    })
                    .unwrap_or(false)
            }) {
                return Ok(format!(
                    "Flashed {} B at 0x{write_addr:08x} on bus {fw_bus} ports {fw_ports:?} (sha256={expected_sha}, LittleFS preserved sha256={lfs_sha})",
                    plan.image.len()
                ));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    Err(format!(
        "The keyboard firmware did not come back on bus {fw_bus} ports {fw_ports:?} after flashing"
    ))
}

#[cfg(target_os = "windows")]
fn snapshot_windows_rp2_volumes() -> std::collections::HashSet<char> {
    let mut found = std::collections::HashSet::new();
    for letter in b'D'..=b'Z' {
        let ch = letter as char;
        let info_path = std::path::PathBuf::from(format!("{ch}:\\INFO_UF2.TXT"));
        if let Ok(txt) = std::fs::read_to_string(&info_path) {
            if txt.contains("RP2040") || txt.contains("RPI-RP2") {
                found.insert(ch);
            }
        }
    }
    found
}

#[cfg(target_os = "windows")]
fn flash_uf2_via_windows_rp2_volume(
    uf2_bytes: &[u8],
    picoboot_err: &str,
    pre_existing: &std::collections::HashSet<char>,
) -> Result<String, String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let now_vols = snapshot_windows_rp2_volumes();
        let newly_mounted: Vec<char> = now_vols
            .into_iter()
            .filter(|ch| !pre_existing.contains(ch))
            .collect();
        if newly_mounted.len() > 1 {
            return Err(format!(
                "Multiple new RPI-RP2 drives mounted ({newly_mounted:?}); unplug other RP2040 boards before flashing"
            ));
        }
        if let Some(&ch) = newly_mounted.first() {
            let dst = std::path::PathBuf::from(format!("{ch}:\\keyglass-firmware.uf2"));
            std::fs::write(&dst, uf2_bytes).map_err(|e| {
                format!(
                    "PICOBOOT unavailable ({picoboot_err}); failed writing UF2 to {}: {e}",
                    dst.display()
                )
            })?;
            return Ok("uf2-mass-storage".to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Err(format!(
        "PICOBOOT unavailable ({picoboot_err}) and no newly mounted RPI-RP2 drive appeared on Windows"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_uf2_block(addr: u32, payload: &[u8], bno: u32, tot: u32) -> [u8; 512] {
        let mut b = [0u8; 512];
        b[0..4].copy_from_slice(&UF2_MAGIC_START0.to_le_bytes());
        b[4..8].copy_from_slice(&UF2_MAGIC_START1.to_le_bytes());
        b[8..12].copy_from_slice(&0x0000_2000u32.to_le_bytes());
        b[12..16].copy_from_slice(&addr.to_le_bytes());
        b[16..20].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        b[20..24].copy_from_slice(&bno.to_le_bytes());
        b[24..28].copy_from_slice(&tot.to_le_bytes());
        b[28..32].copy_from_slice(&RP2040_FAMILY_ID.to_le_bytes());
        b[32..32 + payload.len()].copy_from_slice(payload);
        b[508..512].copy_from_slice(&UF2_MAGIC_END.to_le_bytes());
        b
    }

    #[test]
    fn valid_uf2_sector_aligned_and_rejects_incomplete_duplicate_or_littlefs() {
        let mut ok = Vec::new();
        ok.extend_from_slice(&make_uf2_block(0x1000_0000, &[1u8; 256], 0, 2));
        ok.extend_from_slice(&make_uf2_block(0x1000_0100, &[2u8; 256], 1, 2));
        let plan = parse_uf2_image(&ok).unwrap();
        assert_eq!(plan.flash_offset, 0);
        assert_eq!(plan.image.len(), 4096);
        assert_eq!(&plan.image[0..256], &[1u8; 256]);
        assert_eq!(&plan.image[256..512], &[2u8; 256]);

        // Missing block (header claims 3 blocks, only 2 present)
        let mut trunc = Vec::new();
        trunc.extend_from_slice(&make_uf2_block(0x1000_0000, &[1u8; 256], 0, 3));
        trunc.extend_from_slice(&make_uf2_block(0x1000_0100, &[2u8; 256], 1, 3));
        assert!(parse_uf2_image(&trunc).unwrap_err().contains("Incomplete"));

        // Duplicate address
        let mut dup = Vec::new();
        dup.extend_from_slice(&make_uf2_block(0x1000_0000, &[1u8; 256], 0, 2));
        dup.extend_from_slice(&make_uf2_block(0x1000_0000, &[2u8; 256], 1, 2));
        assert!(parse_uf2_image(&dup).unwrap_err().contains("Duplicate"));

        // Unaligned payload
        let unal = make_uf2_block(0x1000_0080, &[1u8; 256], 0, 1);
        assert!(parse_uf2_image(&unal).unwrap_err().contains("align"));

        // LittleFS overlap
        let lfs = make_uf2_block(RP2040_LFS_START, &[0xFFu8; 256], 0, 1);
        assert!(parse_uf2_image(&lfs).unwrap_err().contains("LittleFS"));
    }

    #[test]
    fn picoboot_cmd_wire_layout() {
        let cmd = build_picoboot_cmd(PC_READ, 0x1122_3344, 4096, &[1, 2, 3, 4]);
        assert_eq!(&cmd[0..4], &PICOBOOT_MAGIC.to_le_bytes());
        assert_eq!(&cmd[4..8], &0x1122_3344u32.to_le_bytes());
        assert_eq!(cmd[8], PC_READ);
        assert_eq!(cmd[9], 4);
        assert_eq!(&cmd[12..16], &4096u32.to_le_bytes());
        assert_eq!(&cmd[16..20], &[1, 2, 3, 4]);
    }

    #[test]
    fn picoboot_selection_rejects_unrelated_bootsel_pico_and_binds_exact_usb_path() {
        let unrelated = vec![(1u8, vec![2u8, 1u8])];
        let err = select_picoboot_for_firmware_path((1, &[3u8, 4u8]), &unrelated).unwrap_err();
        assert!(
            err.contains("other BOOTSEL devices are left alone"),
            "got {err}"
        );

        let both = vec![(1u8, vec![2u8, 1u8]), (1u8, vec![3u8, 4u8])];
        assert_eq!(
            select_picoboot_for_firmware_path((1, &[3u8, 4u8]), &both).unwrap(),
            1
        );

        // A boot ROM status error is reported.
        let mut bad_st = [0u8; 16];
        bad_st[0..4].copy_from_slice(&7u32.to_le_bytes());
        bad_st[4..8].copy_from_slice(&2u32.to_le_bytes());
        bad_st[8] = PC_FLASH_ERASE;
        assert!(parse_picoboot_status(&bad_st, 7)
            .unwrap_err()
            .contains("status=2"));
        bad_st[4..8].copy_from_slice(&0u32.to_le_bytes());
        assert!(parse_picoboot_status(&bad_st, 7).is_ok());
    }
}
