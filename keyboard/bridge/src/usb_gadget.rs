#[cfg(target_os = "linux")]
use crate::device_paths::{FACTORY_BRIDGE_FFS_MOUNT, FACTORY_FFS_FUNCTION_NAME, FFS_FUNCTION_NAME};
#[cfg(target_os = "linux")]
use crate::protocol::{
    USB_CONFIG_STR, USB_MFR_STR, USB_PRODUCT_ID_STR, USB_PROD_STR, USB_SERIAL_STR, USB_UDC_NAME,
    USB_VENDOR_ID_STR,
};
use std::os::unix::io::RawFd;

#[cfg(target_os = "linux")]
const GADGET_PATH: &str = "/sys/kernel/config/usb_gadget/g1";

#[cfg(any(target_os = "linux", test))]
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct UsbIntfDesc {
    b_length: u8,
    b_descriptor_type: u8,
    b_interface_number: u8,
    b_alternate_setting: u8,
    b_num_endpoints: u8,
    b_interface_class: u8,
    b_interface_sub_class: u8,
    b_interface_protocol: u8,
    i_interface: u8,
}

#[cfg(any(target_os = "linux", test))]
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct UsbEpDescNoAudio {
    b_length: u8,
    b_descriptor_type: u8,
    b_endpoint_address: u8,
    bm_attributes: u8,
    w_max_packet_size: u16,
    b_interval: u8,
}

#[cfg(any(target_os = "linux", test))]
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct FsHsGroup {
    intf: UsbIntfDesc,
    sink: UsbEpDescNoAudio,
    source: UsbEpDescNoAudio,
}

#[cfg(any(target_os = "linux", test))]
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct UsbOsDescHeader {
    interface: u8,
    dw_length: u32,
    bcd_version: u16,
    w_index: u16,
    b_count: u8,
    reserved: u8,
}

#[cfg(any(target_os = "linux", test))]
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct UsbExtCompatDesc {
    b_first_interface_number: u8,
    reserved1: u8,
    compatible_id: [u8; 8],
    sub_compatible_id: [u8; 8],
    reserved2: [u8; 6],
}

/// FunctionFS MS OS 1.0 Extended Property descriptor registering `DeviceInterfaceGUID` so
/// Windows `libusb` (WinUSB backend) can discover and open the bulk interface without an INF.
/// Linux `f_fs.c` expects UTF-8 NUL-terminated strings here and doubles their lengths when
/// converting to UTF-16LE for the host.
#[cfg(any(target_os = "linux", test))]
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct UsbExtPropGuidDesc {
    dw_size: u32,
    dw_property_data_type: u32,
    w_property_name_length: u16,
    b_property_name: [u8; 20], // "DeviceInterfaceGUID\0"
    dw_property_data_length: u32,
    b_property_data: [u8; 39], // "{9F4C2E18-7B3A-4D51-A60E-8C2F1D4B7A93}\0"
}

#[cfg(any(target_os = "linux", test))]
#[repr(C, packed)]
struct BridgeFfsDescs {
    magic: u32,
    length: u32,
    flags: u32,
    fs_count: u32,
    hs_count: u32,
    os_count: u32,
    fs_descs: FsHsGroup,
    hs_descs: FsHsGroup,
    os_compat_header: UsbOsDescHeader,
    os_compat: UsbExtCompatDesc,
    os_prop_header: UsbOsDescHeader,
    os_prop: UsbExtPropGuidDesc,
}

#[cfg(target_os = "linux")]
#[repr(C, packed)]
struct BridgeFfsDescsCompatOnly {
    magic: u32,
    length: u32,
    flags: u32,
    fs_count: u32,
    hs_count: u32,
    os_count: u32,
    fs_descs: FsHsGroup,
    hs_descs: FsHsGroup,
    os_compat_header: UsbOsDescHeader,
    os_compat: UsbExtCompatDesc,
}

#[cfg(target_os = "linux")]
#[repr(C, packed)]
struct BridgeFfsDescsLegacy {
    magic: u32,
    length: u32,
    flags: u32,
    fs_count: u32,
    hs_count: u32,
    fs_descs: FsHsGroup,
    hs_descs: FsHsGroup,
}

#[cfg(any(target_os = "linux", test))]
fn build_bridge_ffs_descs() -> BridgeFfsDescs {
    let fs_descs = FsHsGroup {
        intf: UsbIntfDesc {
            b_length: 9,
            b_descriptor_type: 0x04,
            b_interface_number: 0,
            b_alternate_setting: 0,
            b_num_endpoints: 2,
            b_interface_class: 0xFF,
            b_interface_sub_class: 0,
            b_interface_protocol: 0,
            i_interface: 1,
        },
        sink: UsbEpDescNoAudio {
            b_length: 7,
            b_descriptor_type: 0x05,
            b_endpoint_address: 1 | 0x80,
            bm_attributes: 2,
            w_max_packet_size: 64u16.to_le(),
            b_interval: 0,
        },
        source: UsbEpDescNoAudio {
            b_length: 7,
            b_descriptor_type: 0x05,
            b_endpoint_address: 1,
            bm_attributes: 2,
            w_max_packet_size: 64u16.to_le(),
            b_interval: 0,
        },
    };
    let mut hs_descs = fs_descs;
    hs_descs.sink.w_max_packet_size = 512u16.to_le();
    hs_descs.source.w_max_packet_size = 512u16.to_le();

    let compat_len =
        (std::mem::size_of::<UsbOsDescHeader>() + std::mem::size_of::<UsbExtCompatDesc>()) as u32;
    let prop_size = std::mem::size_of::<UsbExtPropGuidDesc>() as u32;
    let prop_len = (std::mem::size_of::<UsbOsDescHeader>() + prop_size as usize) as u32;
    BridgeFfsDescs {
        magic: 3u32.to_le(), // FUNCTIONFS_DESCRIPTORS_MAGIC_V2
        length: (std::mem::size_of::<BridgeFfsDescs>() as u32).to_le(),
        flags: (1u32 | 2u32 | 8u32).to_le(), // HAS_FS_DESC | HAS_HS_DESC | HAS_MS_OS_DESC
        fs_count: 3u32.to_le(),
        hs_count: 3u32.to_le(),
        os_count: 2u32.to_le(),
        fs_descs,
        hs_descs,
        os_compat_header: UsbOsDescHeader {
            interface: 0,
            dw_length: compat_len.to_le(),
            // Linux FunctionFS (5.x and 6.x) accepts bcdVersion = 1 (5.x rejects 0x0100 with EINVAL).
            bcd_version: 1u16.to_le(),
            w_index: 0x0004u16.to_le(), // Extended Compat ID
            b_count: 1,
            reserved: 0,
        },
        os_compat: UsbExtCompatDesc {
            b_first_interface_number: 0,
            reserved1: 1,
            compatible_id: *b"WINUSB\0\0",
            sub_compatible_id: [0; 8],
            reserved2: [0; 6],
        },
        os_prop_header: UsbOsDescHeader {
            interface: 0,
            dw_length: prop_len.to_le(),
            bcd_version: 1u16.to_le(),
            w_index: 0x0005u16.to_le(), // Extended Properties
            b_count: 1,                 // wCount = 1 (LE: [1, 0])
            reserved: 0,
        },
        os_prop: UsbExtPropGuidDesc {
            dw_size: prop_size.to_le(),
            dw_property_data_type: 1u32.to_le(), // USB_EXT_PROP_UNICODE (REG_SZ)
            w_property_name_length: 20u16.to_le(),
            b_property_name: *b"DeviceInterfaceGUID\0",
            dw_property_data_length: 39u32.to_le(),
            b_property_data: *b"{9F4C2E18-7B3A-4D51-A60E-8C2F1D4B7A93}\0",
        },
    }
}

#[cfg(target_os = "linux")]
#[repr(C, packed)]
struct BridgeFfsStrings {
    magic: u32,
    length: u32,
    str_count: u32,
    lang_count: u32,
    code: u16,
    str1: [u8; 9], // "OpenFlux\0"
}

#[cfg(target_os = "linux")]
fn write_str(path: &str, val: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)?;
    f.write_all(val.as_bytes())
}

#[cfg(target_os = "linux")]
fn report(result: std::io::Result<()>, path: &str) {
    match result {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => eprintln!("usb gadget: {path}: {e}"),
    }
}

#[cfg(target_os = "linux")]
fn write_attr(path: &str, val: &str) {
    report(write_str(path, val), path);
}

#[cfg(target_os = "linux")]
fn make_dir(path: &str) {
    report(std::fs::create_dir_all(path), path);
}

#[cfg(target_os = "linux")]
fn make_link(target: &str, link: &str) {
    report(std::os::unix::fs::symlink(target, link), link);
}

#[cfg(target_os = "linux")]
pub fn bridge_setup_gadget(mount_point: &str) -> Result<(), String> {
    use std::ffi::CString;
    let _ = write_str(&format!("{GADGET_PATH}/UDC"), "\n");
    for fn_name in [FFS_FUNCTION_NAME, FACTORY_FFS_FUNCTION_NAME] {
        let _ = std::fs::remove_file(format!("{GADGET_PATH}/configs/c.1/{fn_name}"));
    }
    let _ = std::fs::remove_file(format!("{GADGET_PATH}/os_desc/c.1"));
    let _ = std::fs::remove_dir(format!("{GADGET_PATH}/configs/c.1/strings/0x409"));
    let _ = std::fs::remove_dir(format!("{GADGET_PATH}/configs/c.1"));
    for fn_name in [FFS_FUNCTION_NAME, FACTORY_FFS_FUNCTION_NAME] {
        let _ = std::fs::remove_dir(format!("{GADGET_PATH}/functions/{fn_name}"));
    }
    let _ = std::fs::remove_dir(format!("{GADGET_PATH}/strings/0x409"));
    let _ = std::fs::remove_dir(GADGET_PATH);
    for mnt in [mount_point, FACTORY_BRIDGE_FFS_MOUNT] {
        if let Ok(cm) = CString::new(mnt) {
            unsafe { libc::umount2(cm.as_ptr(), libc::MNT_DETACH) };
        }
    }

    // Configuration steps keep going after a failure so one rejected attribute doesn't take the
    // whole USB link down; each failure is logged with its path, so a later mount or bind error
    // can be traced to its cause.
    make_dir(mount_point);
    make_dir(GADGET_PATH);
    for (attr, val) in [
        ("idVendor", USB_VENDOR_ID_STR),
        ("idProduct", USB_PRODUCT_ID_STR),
        ("bcdDevice", "0x0102"),
        ("bcdUSB", "0x0200"),
    ] {
        write_attr(&format!("{GADGET_PATH}/{attr}"), val);
    }

    make_dir(&format!("{GADGET_PATH}/strings/0x409"));
    for (attr, val) in [
        ("serialnumber", USB_SERIAL_STR),
        ("manufacturer", USB_MFR_STR),
        ("product", USB_PROD_STR),
    ] {
        write_attr(&format!("{GADGET_PATH}/strings/0x409/{attr}"), val);
    }

    make_dir(&format!("{GADGET_PATH}/configs/c.1/strings/0x409"));
    write_attr(
        &format!("{GADGET_PATH}/configs/c.1/strings/0x409/configuration"),
        USB_CONFIG_STR,
    );
    write_attr(&format!("{GADGET_PATH}/configs/c.1/MaxPower"), "120");

    make_dir(&format!("{GADGET_PATH}/functions/{FFS_FUNCTION_NAME}"));
    make_link(
        &format!("{GADGET_PATH}/functions/{FFS_FUNCTION_NAME}"),
        &format!("{GADGET_PATH}/configs/c.1/{FFS_FUNCTION_NAME}"),
    );

    write_attr(&format!("{GADGET_PATH}/os_desc/use"), "1");
    write_attr(&format!("{GADGET_PATH}/os_desc/b_vendor_code"), "0x01");
    write_attr(&format!("{GADGET_PATH}/os_desc/qw_sign"), "MSFT100");
    make_link(
        &format!("{GADGET_PATH}/configs/c.1"),
        &format!("{GADGET_PATH}/os_desc/c.1"),
    );

    let c_src = CString::new("openflux").unwrap();
    let c_tgt = CString::new(mount_point).map_err(|e| e.to_string())?;
    let c_fs = CString::new("functionfs").unwrap();
    let rc = unsafe {
        libc::mount(
            c_src.as_ptr(),
            c_tgt.as_ptr(),
            c_fs.as_ptr(),
            libc::MS_NOEXEC,
            std::ptr::null(),
        )
    };
    if rc != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EBUSY) {
        return Err(format!(
            "mount functionfs: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn bridge_open_ffs(mount_point: &str) -> Result<(RawFd, RawFd, RawFd), String> {
    use std::ffi::CString;
    let p0 = CString::new(format!("{mount_point}/ep0")).map_err(|e| e.to_string())?;
    let p1 = CString::new(format!("{mount_point}/ep1")).map_err(|e| e.to_string())?;
    let p2 = CString::new(format!("{mount_point}/ep2")).map_err(|e| e.to_string())?;

    let ep0 = unsafe { libc::open(p0.as_ptr(), libc::O_RDWR) };
    if ep0 < 0 {
        return Err(format!("open ep0: {}", std::io::Error::last_os_error()));
    }

    let descs = build_bridge_ffs_descs();
    let d_bytes = unsafe {
        std::slice::from_raw_parts(
            &descs as *const BridgeFfsDescs as *const u8,
            std::mem::size_of::<BridgeFfsDescs>(),
        )
    };
    let mut ep0_ok =
        unsafe { libc::write(ep0, d_bytes.as_ptr() as *const libc::c_void, d_bytes.len()) }
            == d_bytes.len() as isize;
    if !ep0_ok {
        let e1 = std::io::Error::last_os_error();
        eprintln!(
            "usb gadget: write ep0 full MS OS descs ({} B) failed: {e1}",
            d_bytes.len()
        );
        let compat_only = BridgeFfsDescsCompatOnly {
            magic: descs.magic,
            length: (std::mem::size_of::<BridgeFfsDescsCompatOnly>() as u32).to_le(),
            flags: descs.flags,
            fs_count: descs.fs_count,
            hs_count: descs.hs_count,
            os_count: 1u32.to_le(),
            fs_descs: descs.fs_descs,
            hs_descs: descs.hs_descs,
            os_compat_header: descs.os_compat_header,
            os_compat: descs.os_compat,
        };
        let c_bytes = unsafe {
            std::slice::from_raw_parts(
                &compat_only as *const BridgeFfsDescsCompatOnly as *const u8,
                std::mem::size_of::<BridgeFfsDescsCompatOnly>(),
            )
        };
        ep0_ok =
            unsafe { libc::write(ep0, c_bytes.as_ptr() as *const libc::c_void, c_bytes.len()) }
                == c_bytes.len() as isize;
        if !ep0_ok {
            let e2 = std::io::Error::last_os_error();
            eprintln!(
                "usb gadget: write ep0 compat-only MS OS desc ({} B) failed: {e2}",
                c_bytes.len()
            );
            let legacy = BridgeFfsDescsLegacy {
                magic: descs.magic,
                length: (std::mem::size_of::<BridgeFfsDescsLegacy>() as u32).to_le(),
                flags: (1u32 | 2u32).to_le(),
                fs_count: descs.fs_count,
                hs_count: descs.hs_count,
                fs_descs: descs.fs_descs,
                hs_descs: descs.hs_descs,
            };
            let l_bytes = unsafe {
                std::slice::from_raw_parts(
                    &legacy as *const BridgeFfsDescsLegacy as *const u8,
                    std::mem::size_of::<BridgeFfsDescsLegacy>(),
                )
            };
            ep0_ok =
                unsafe { libc::write(ep0, l_bytes.as_ptr() as *const libc::c_void, l_bytes.len()) }
                    == l_bytes.len() as isize;
            if !ep0_ok {
                let e3 = std::io::Error::last_os_error();
                eprintln!("usb gadget: write ep0 legacy descs failed: {e3}");
            }
        }
    }
    if !ep0_ok {
        unsafe { libc::close(ep0) };
        return Err("write ep0 descriptors failed".into());
    }

    let strs = BridgeFfsStrings {
        magic: 2u32.to_le(), // FUNCTIONFS_STRINGS_MAGIC
        length: (std::mem::size_of::<BridgeFfsStrings>() as u32).to_le(),
        str_count: 1u32.to_le(),
        lang_count: 1u32.to_le(),
        code: 0x0409u16.to_le(),
        str1: *b"OpenFlux\0",
    };
    let s_bytes = unsafe {
        std::slice::from_raw_parts(
            &strs as *const BridgeFfsStrings as *const u8,
            std::mem::size_of::<BridgeFfsStrings>(),
        )
    };
    if unsafe { libc::write(ep0, s_bytes.as_ptr() as *const libc::c_void, s_bytes.len()) }
        != s_bytes.len() as isize
    {
        unsafe { libc::close(ep0) };
        return Err("write ep0 strings failed".into());
    }

    if let Err(e) = write_str(&format!("{GADGET_PATH}/UDC"), USB_UDC_NAME) {
        unsafe { libc::close(ep0) };
        return Err(format!("bind UDC {USB_UDC_NAME}: {e}"));
    }

    let ep1 = unsafe { libc::open(p1.as_ptr(), libc::O_RDWR) };
    let ep2 = unsafe { libc::open(p2.as_ptr(), libc::O_RDWR) };
    if ep1 < 0 || ep2 < 0 {
        unsafe {
            if ep1 >= 0 {
                libc::close(ep1);
            }
            if ep2 >= 0 {
                libc::close(ep2);
            }
            libc::close(ep0);
        }
        return Err("open ep1/ep2 failed".into());
    }
    Ok((ep0, ep1, ep2))
}

#[cfg(not(target_os = "linux"))]
pub fn bridge_setup_gadget(_mount_point: &str) -> Result<(), String> {
    Err("USB FunctionFS gadget only supported on Linux".into())
}

#[cfg(not(target_os = "linux"))]
pub fn bridge_open_ffs(_mount_point: &str) -> Result<(RawFd, RawFd, RawFd), String> {
    Err("USB FunctionFS gadget only supported on Linux".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffs_descriptors_advertise_winusb_ms_os_desc() {
        let d = build_bridge_ffs_descs();
        assert_eq!(std::mem::size_of::<UsbOsDescHeader>(), 11);
        assert_eq!(std::mem::size_of::<UsbExtCompatDesc>(), 24);
        assert_eq!(std::mem::size_of::<UsbExtPropGuidDesc>(), 14 + 20 + 39);
        assert_eq!(u32::from_le(d.magic), 3);
        assert_eq!(u32::from_le(d.flags), 1 | 2 | 8);
        assert_eq!(u32::from_le(d.os_count), 2);
        assert_eq!(
            u32::from_le(d.length) as usize,
            std::mem::size_of::<BridgeFfsDescs>()
        );
        assert_eq!(u32::from_le(d.os_compat_header.dw_length), 35);
        assert_eq!(u16::from_le(d.os_compat_header.bcd_version), 1);
        assert_eq!(u16::from_le(d.os_compat_header.w_index), 4);
        assert_eq!(d.os_compat_header.b_count, 1);
        assert_eq!(d.os_compat.reserved1, 1);
        assert_eq!(d.os_compat.compatible_id, *b"WINUSB\0\0");
        assert_eq!(u32::from_le(d.os_prop_header.dw_length), 11 + 73);
        assert_eq!(u16::from_le(d.os_prop_header.bcd_version), 1);
        assert_eq!(u16::from_le(d.os_prop_header.w_index), 5);
        assert_eq!(d.os_prop_header.b_count, 1);
        assert_eq!(u32::from_le(d.os_prop.dw_size), 73);
        assert_eq!(u32::from_le(d.os_prop.dw_property_data_type), 1);
        assert_eq!(u16::from_le(d.os_prop.w_property_name_length), 20);
        assert_eq!(u32::from_le(d.os_prop.dw_property_data_length), 39);
        assert_eq!(d.os_prop.b_property_name, *b"DeviceInterfaceGUID\0");
    }
}
