//! Extracts files from, or builds, the 1 MiB littlefs image in the keyboard's RP2040 flash.

use core::ffi::{c_int, c_void, CStr};
use littlefs2::ll;
use std::env;
use std::fs;
use std::path::Path;
use std::process;

const BLOCK_SIZE: usize = 4096;
const BLOCK_COUNT: usize = 256;
/// A file system formatted with one block fewer also mounts.
const SHORT_BLOCK_COUNT: usize = 255;
const IMG_SIZE: usize = BLOCK_SIZE * BLOCK_COUNT;
const PROG_SIZE: usize = 256;
const LOOKAHEAD_SIZE: usize = 16;

struct MemFlash {
    img: Vec<u8>,
    max_blocks: usize,
}

unsafe extern "C" fn cb_read(
    c: *const ll::lfs_config,
    block: ll::lfs_block_t,
    off: ll::lfs_off_t,
    buffer: *mut c_void,
    size: ll::lfs_size_t,
) -> c_int {
    let mem = &*((*c).context as *const MemFlash);
    let b = block as usize;
    let start = b.saturating_mul(BLOCK_SIZE).saturating_add(off as usize);
    let end = start.saturating_add(size as usize);
    if b >= mem.max_blocks || end > mem.img.len() {
        return ll::lfs_error_LFS_ERR_IO;
    }
    core::ptr::copy_nonoverlapping(
        mem.img[start..end].as_ptr(),
        buffer as *mut u8,
        size as usize,
    );
    0
}

unsafe extern "C" fn cb_prog(
    c: *const ll::lfs_config,
    block: ll::lfs_block_t,
    off: ll::lfs_off_t,
    buffer: *const c_void,
    size: ll::lfs_size_t,
) -> c_int {
    let mem = &mut *((*c).context as *mut MemFlash);
    let b = block as usize;
    let start = b.saturating_mul(BLOCK_SIZE).saturating_add(off as usize);
    let end = start.saturating_add(size as usize);
    if b >= mem.max_blocks || end > mem.img.len() {
        return ll::lfs_error_LFS_ERR_IO;
    }
    core::ptr::copy_nonoverlapping(
        buffer as *const u8,
        mem.img[start..end].as_mut_ptr(),
        size as usize,
    );
    0
}

unsafe extern "C" fn cb_erase(c: *const ll::lfs_config, block: ll::lfs_block_t) -> c_int {
    let mem = &mut *((*c).context as *mut MemFlash);
    let b = block as usize;
    let start = b.saturating_mul(BLOCK_SIZE);
    let end = start.saturating_add(BLOCK_SIZE);
    if b >= mem.max_blocks || end > mem.img.len() {
        return ll::lfs_error_LFS_ERR_IO;
    }
    mem.img[start..end].fill(0xFF);
    0
}

unsafe extern "C" fn cb_sync(_c: *const ll::lfs_config) -> c_int {
    0
}

struct LfsBufs {
    read_buf: Vec<u8>,
    prog_buf: Vec<u8>,
    lookahead_buf: Vec<u8>,
}

impl LfsBufs {
    fn new() -> Self {
        Self {
            read_buf: vec![0u8; BLOCK_SIZE],
            prog_buf: vec![0u8; BLOCK_SIZE],
            lookahead_buf: vec![0u8; LOOKAHEAD_SIZE],
        }
    }

    fn config(&mut self, mem: &mut MemFlash, block_count: u32) -> ll::lfs_config {
        ll::lfs_config {
            context: (mem as *mut MemFlash).cast::<c_void>(),
            read: Some(cb_read),
            prog: Some(cb_prog),
            erase: Some(cb_erase),
            sync: Some(cb_sync),
            read_size: 1,
            prog_size: PROG_SIZE as u32,
            block_size: BLOCK_SIZE as u32,
            block_count,
            block_cycles: 500,
            cache_size: BLOCK_SIZE as u32,
            lookahead_size: LOOKAHEAD_SIZE as u32,
            read_buffer: self.read_buf.as_mut_ptr().cast::<c_void>(),
            prog_buffer: self.prog_buf.as_mut_ptr().cast::<c_void>(),
            lookahead_buffer: self.lookahead_buf.as_mut_ptr().cast::<c_void>(),
            name_max: 0,
            file_max: 0,
            attr_max: 0,
            compact_thresh: 0,
            metadata_max: 0,
            inline_max: 0,
            disk_version: 0,
        }
    }
}

fn c_path(s: &str) -> Option<Vec<u8>> {
    if s.as_bytes().contains(&0) {
        return None;
    }
    let mut b = Vec::with_capacity(s.len() + 1);
    b.extend_from_slice(s.as_bytes());
    b.push(0);
    Some(b)
}

fn file_open_cfg(file_buf: &mut [u8]) -> ll::lfs_file_config {
    ll::lfs_file_config {
        buffer: file_buf.as_mut_ptr().cast::<c_void>(),
        attrs: core::ptr::null_mut(),
        attr_count: 0,
    }
}

fn mount_explicit<'a>(
    mem: &'a mut MemFlash,
    bufs: &'a mut LfsBufs,
    cfg_slot: &'a mut Option<ll::lfs_config>,
    lfs: &mut ll::lfs_t,
    blocks: usize,
) -> bool {
    if mem.img.len() < blocks * BLOCK_SIZE {
        return false;
    }
    mem.max_blocks = blocks;
    *cfg_slot = Some(bufs.config(mem, blocks as u32));
    let cfg_ptr = cfg_slot.as_ref().unwrap() as *const ll::lfs_config;
    unsafe { ll::lfs_mount(lfs, cfg_ptr) == 0 }
}

fn cmd_extract(img_path: &Path, out_dir: &Path) -> i32 {
    let Ok(img) = fs::read(img_path) else {
        eprintln!("read {}: cannot open the image", img_path.display());
        return 1;
    };
    if img.len() != IMG_SIZE && img.len() != BLOCK_SIZE * SHORT_BLOCK_COUNT {
        eprintln!(
            "expected {IMG_SIZE} or {} bytes in {}, got {}",
            BLOCK_SIZE * SHORT_BLOCK_COUNT,
            img_path.display(),
            img.len()
        );
        return 1;
    }
    let mut mem = MemFlash { img, max_blocks: 0 };
    let mut bufs = LfsBufs::new();
    let mut cfg_slot: Option<ll::lfs_config> = None;
    let mut lfs: ll::lfs_t = unsafe { core::mem::zeroed() };

    let mounted = mount_explicit(&mut mem, &mut bufs, &mut cfg_slot, &mut lfs, BLOCK_COUNT)
        || mount_explicit(
            &mut mem,
            &mut bufs,
            &mut cfg_slot,
            &mut lfs,
            SHORT_BLOCK_COUNT,
        );
    if !mounted {
        eprintln!("lfs_mount failed");
        return 2;
    }

    if let Err(e) = fs::create_dir_all(out_dir) {
        unsafe { ll::lfs_unmount(&mut lfs) };
        eprintln!("mkdir {}: {e}", out_dir.display());
        return 1;
    }

    let mut dir: ll::lfs_dir_t = unsafe { core::mem::zeroed() };
    if unsafe { ll::lfs_dir_open(&mut lfs, &mut dir, c"/".as_ptr().cast()) } != 0 {
        unsafe { ll::lfs_unmount(&mut lfs) };
        eprintln!("lfs_dir_open / failed");
        return 2;
    }

    let mut file_buf = vec![0u8; BLOCK_SIZE];
    let mut failed = false;
    loop {
        let mut info: ll::lfs_info = unsafe { core::mem::zeroed() };
        let rc = unsafe { ll::lfs_dir_read(&mut lfs, &mut dir, &mut info) };
        if rc < 0 {
            eprintln!("lfs_dir_read / failed: {rc}");
            failed = true;
            break;
        }
        if rc == 0 {
            break;
        }
        if info.type_ as ll::lfs_type != ll::lfs_type_LFS_TYPE_REG {
            continue;
        }
        let name = unsafe { CStr::from_ptr(info.name.as_ptr().cast()) }
            .to_string_lossy()
            .into_owned();
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            continue;
        }
        let Some(cpath) = c_path(&format!("/{name}")) else {
            eprintln!("invalid file name: {name}");
            failed = true;
            break;
        };
        let mut f: ll::lfs_file_t = unsafe { core::mem::zeroed() };
        let fcfg = file_open_cfg(&mut file_buf);
        if unsafe {
            ll::lfs_file_opencfg(
                &mut lfs,
                &mut f,
                cpath.as_ptr().cast(),
                ll::lfs_open_flags_LFS_O_RDONLY as i32,
                &fcfg,
            )
        } != 0
        {
            eprintln!("lfs_file_open /{name} failed");
            failed = true;
            break;
        }
        let mut buf = Vec::new();
        let mut chunk = [0u8; BLOCK_SIZE];
        loop {
            let n = unsafe {
                ll::lfs_file_read(
                    &mut lfs,
                    &mut f,
                    chunk.as_mut_ptr().cast(),
                    chunk.len() as u32,
                )
            };
            if n < 0 {
                eprintln!("lfs_file_read /{name} failed: {n}");
                failed = true;
                break;
            }
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n as usize]);
        }
        unsafe { ll::lfs_file_close(&mut lfs, &mut f) };
        if failed {
            break;
        }
        let out_path = out_dir.join(&name);
        if let Err(e) = fs::write(&out_path, &buf) {
            eprintln!("write {}: {e}", out_path.display());
            failed = true;
            break;
        }
        println!("{name}\t{}", buf.len());
    }

    unsafe {
        ll::lfs_dir_close(&mut lfs, &mut dir);
        ll::lfs_unmount(&mut lfs);
    }
    if failed {
        2
    } else {
        0
    }
}

fn cmd_create(img_path: &Path, in_dir: &Path) -> i32 {
    let mut mem = MemFlash {
        img: vec![0xFFu8; IMG_SIZE],
        max_blocks: BLOCK_COUNT,
    };
    let mut bufs = LfsBufs::new();
    let cfg = bufs.config(&mut mem, BLOCK_COUNT as u32);
    let mut lfs: ll::lfs_t = unsafe { core::mem::zeroed() };
    if unsafe { ll::lfs_format(&mut lfs, &cfg) } != 0 {
        eprintln!("lfs_format failed");
        return 2;
    }
    if unsafe { ll::lfs_mount(&mut lfs, &cfg) } != 0 {
        eprintln!("lfs_mount failed");
        return 2;
    }

    let rd = match fs::read_dir(in_dir) {
        Ok(rd) => rd,
        Err(e) => {
            unsafe { ll::lfs_unmount(&mut lfs) };
            eprintln!("read {}: {e}", in_dir.display());
            return 1;
        }
    };
    let mut file_buf = vec![0u8; BLOCK_SIZE];
    for ent_res in rd {
        let ent = match ent_res {
            Ok(ent) => ent,
            Err(e) => {
                unsafe { ll::lfs_unmount(&mut lfs) };
                eprintln!("read {}: {e}", in_dir.display());
                return 1;
            }
        };
        let name_os = ent.file_name();
        let name = name_os.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let p = ent.path();
        let meta = match fs::metadata(&p) {
            Ok(meta) => meta,
            Err(e) => {
                unsafe { ll::lfs_unmount(&mut lfs) };
                eprintln!("stat {}: {e}", p.display());
                return 1;
            }
        };
        if !meta.is_file() {
            continue;
        }
        let bytes = match fs::read(&p) {
            Ok(bytes) => bytes,
            Err(e) => {
                unsafe { ll::lfs_unmount(&mut lfs) };
                eprintln!("read {}: {e}", p.display());
                return 1;
            }
        };
        let Some(cpath) = c_path(name.as_ref()) else {
            unsafe { ll::lfs_unmount(&mut lfs) };
            eprintln!("invalid file name: {name}");
            return 2;
        };
        let mut f: ll::lfs_file_t = unsafe { core::mem::zeroed() };
        let fcfg = file_open_cfg(&mut file_buf);
        let flags = (ll::lfs_open_flags_LFS_O_WRONLY
            | ll::lfs_open_flags_LFS_O_CREAT
            | ll::lfs_open_flags_LFS_O_TRUNC) as i32;
        if unsafe { ll::lfs_file_opencfg(&mut lfs, &mut f, cpath.as_ptr().cast(), flags, &fcfg) }
            != 0
        {
            unsafe { ll::lfs_unmount(&mut lfs) };
            eprintln!("lfs_file_open {name} failed");
            return 2;
        }
        let wr = unsafe {
            ll::lfs_file_write(&mut lfs, &mut f, bytes.as_ptr().cast(), bytes.len() as u32)
        };
        let cl = unsafe { ll::lfs_file_close(&mut lfs, &mut f) };
        if wr != bytes.len() as i32 || cl != 0 {
            unsafe { ll::lfs_unmount(&mut lfs) };
            eprintln!("lfs_file_write {name} failed");
            return 2;
        }
    }
    unsafe { ll::lfs_unmount(&mut lfs) };
    if let Err(e) = fs::write(img_path, &mem.img) {
        eprintln!("write {}: {e}", img_path.display());
        return 1;
    }
    0
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() == 4 && args[1] == "extract" {
        process::exit(cmd_extract(Path::new(&args[2]), Path::new(&args[3])));
    }
    if args.len() == 4 && args[1] == "create" {
        process::exit(cmd_create(Path::new(&args[2]), Path::new(&args[3])));
    }
    eprintln!(
        "usage: {} <extract|create> <image.bin> <dir>",
        args.first().map(|s| s.as_str()).unwrap_or("lfs_tool")
    );
    process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_rejects_corrupt_truncated_and_overcapacity_create() {
        let tmp = std::env::temp_dir().join(format!("openflux_lfs_tool_t_{}", process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let in_dir = tmp.join("in");
        let out_dir = tmp.join("out");
        fs::create_dir_all(&in_dir).unwrap();
        fs::write(in_dir.join("hello.bin"), b"openflux-cal-123").unwrap();
        let img = tmp.join("ok.bin");
        assert_eq!(cmd_create(&img, &in_dir), 0);
        assert_eq!(cmd_extract(&img, &out_dir), 0);
        assert_eq!(
            fs::read(out_dir.join("hello.bin")).unwrap(),
            b"openflux-cal-123"
        );

        // A 256-block image cut to 255 blocks fails to extract.
        let trunc_img = tmp.join("trunc_255.bin");
        let raw = fs::read(&img).unwrap();
        fs::write(&trunc_img, &raw[..BLOCK_SIZE * 255]).unwrap();
        assert_ne!(cmd_extract(&trunc_img, &tmp.join("out_trunc")), 0);

        // An image formatted with 255 blocks mounts and extracts.
        let img_255 = tmp.join("ok_255.bin");
        let mut mem255 = MemFlash {
            img: vec![0xFFu8; BLOCK_SIZE * 255],
            max_blocks: 255,
        };
        let mut bufs = LfsBufs::new();
        let cfg255 = bufs.config(&mut mem255, 255);
        let mut lfs255: ll::lfs_t = unsafe { core::mem::zeroed() };
        assert_eq!(unsafe { ll::lfs_format(&mut lfs255, &cfg255) }, 0);
        assert_eq!(unsafe { ll::lfs_mount(&mut lfs255, &cfg255) }, 0);
        let mut f255: ll::lfs_file_t = unsafe { core::mem::zeroed() };
        let mut fbuf = vec![0u8; BLOCK_SIZE];
        let fcfg = file_open_cfg(&mut fbuf);
        let flags = (ll::lfs_open_flags_LFS_O_WRONLY | ll::lfs_open_flags_LFS_O_CREAT) as i32;
        assert_eq!(
            unsafe {
                ll::lfs_file_opencfg(
                    &mut lfs255,
                    &mut f255,
                    c"cal.bin".as_ptr().cast(),
                    flags,
                    &fcfg,
                )
            },
            0
        );
        assert_eq!(
            unsafe { ll::lfs_file_write(&mut lfs255, &mut f255, b"ok255".as_ptr().cast(), 5) },
            5
        );
        assert_eq!(unsafe { ll::lfs_file_close(&mut lfs255, &mut f255) }, 0);
        assert_eq!(unsafe { ll::lfs_unmount(&mut lfs255) }, 0);
        fs::write(&img_255, &mem255.img).unwrap();
        let out_255 = tmp.join("out_255");
        assert_eq!(cmd_extract(&img_255, &out_255), 0);
        assert_eq!(fs::read(out_255.join("cal.bin")).unwrap(), b"ok255");

        // An all-zero image fails without hanging.
        let bad_img = tmp.join("corrupt.bin");
        fs::write(&bad_img, vec![0u8; IMG_SIZE]).unwrap();
        assert_ne!(cmd_extract(&bad_img, &tmp.join("out_bad")), 0);

        // An over-capacity create fails and writes no output image.
        let big_dir = tmp.join("big");
        fs::create_dir_all(&big_dir).unwrap();
        fs::write(big_dir.join("too_big.bin"), vec![0xAAu8; IMG_SIZE + 4096]).unwrap();
        let partial_img = tmp.join("should_not_exist.bin");
        assert_ne!(cmd_create(&partial_img, &big_dir), 0);
        assert!(
            !partial_img.exists(),
            "failed create must not publish partial image"
        );

        let _ = fs::remove_dir_all(&tmp);
    }
}
