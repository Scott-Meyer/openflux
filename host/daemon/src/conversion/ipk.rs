//! Reading files out of an opkg package: an `ar` archive whose `data.tar.gz` (or `data.tar`)
//! holds the installed files.

use std::collections::BTreeMap;
use std::io::Read;

/// The members of an `ar` archive, by name.
fn ar_members(data: &[u8]) -> Result<BTreeMap<String, &[u8]>, String> {
    let mut rest = data
        .strip_prefix(b"!<arch>\n")
        .ok_or("not an opkg package (no ar header)")?;
    let mut out = BTreeMap::new();
    while !rest.is_empty() {
        if rest.len() < 60 {
            return Err("truncated package".into());
        }
        let (header, body) = rest.split_at(60);
        let name = String::from_utf8_lossy(&header[..16])
            .trim()
            .trim_end_matches('/')
            .to_string();
        let size: usize = String::from_utf8_lossy(&header[48..58])
            .trim()
            .parse()
            .map_err(|_| "bad member size in package")?;
        if body.len() < size {
            return Err("truncated package".into());
        }
        out.insert(name, &body[..size]);
        let next = size + (size % 2);
        rest = body.get(next..).unwrap_or(&[]);
    }
    Ok(out)
}

/// Reads the files at `paths` (absolute install paths, such as
/// `/usr/lib/keyglass/bridge/keyglass-bridge`) from the package's data archive.
pub fn read_files(ipk: &[u8], paths: &[&str]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let members = ar_members(ipk)?;
    let reader: Box<dyn Read + '_> = if let Some(gz) = members.get("data.tar.gz") {
        Box::new(flate2::read::GzDecoder::new(*gz))
    } else if let Some(tar) = members.get("data.tar") {
        Box::new(*tar)
    } else {
        return Err("the package has no data.tar.gz".into());
    };
    let mut archive = tar::Archive::new(reader);
    let mut out = BTreeMap::new();
    for entry in archive
        .entries()
        .map_err(|e| format!("unreadable package data: {e}"))?
    {
        let mut entry = entry.map_err(|e| format!("unreadable package data: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .to_string();
        let abs = format!("/{}", path.trim_start_matches("./").trim_start_matches('/'));
        if paths.contains(&abs.as_str()) && entry.header().entry_type().is_file() {
            let mut data = Vec::new();
            entry.read_to_end(&mut data).map_err(|e| e.to_string())?;
            out.insert(abs, data);
        }
    }
    if let Some(missing) = paths.iter().find(|p| !out.contains_key(**p)) {
        return Err(format!("the package doesn't contain {missing}"));
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds a package with the given data files, the way the bridge's packaging does.
    pub fn build(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar_gz = Vec::new();
        {
            let gz = flate2::write::GzEncoder::new(&mut tar_gz, flate2::Compression::default());
            let mut tar = tar::Builder::new(gz);
            for (path, data) in files {
                let mut h = tar::Header::new_gnu();
                h.set_size(data.len() as u64);
                h.set_mode(0o755);
                h.set_cksum();
                tar.append_data(&mut h, format!(".{path}"), *data).unwrap();
            }
            tar.into_inner().unwrap().finish().unwrap();
        }
        let mut out = b"!<arch>\n".to_vec();
        for (name, payload) in [
            ("debian-binary", b"2.0\n".to_vec()),
            ("control.tar.gz", Vec::new()),
            ("data.tar.gz", tar_gz),
        ] {
            out.extend_from_slice(
                format!(
                    "{name:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
                    0,
                    0,
                    0,
                    100644,
                    payload.len()
                )
                .as_bytes(),
            );
            out.extend_from_slice(&payload);
            if payload.len() % 2 == 1 {
                out.push(b'\n');
            }
        }
        out
    }

    #[test]
    fn files_are_read_from_the_package_data() {
        let ipk = build(&[
            ("/usr/lib/keyglass/bridge/keyglass-bridge", b"ELF"),
            (
                "/usr/lib/keyglass/bridge/keyglass-autolaunch.sh",
                b"#!/bin/sh\n",
            ),
        ]);
        let files = read_files(
            &ipk,
            &[
                "/usr/lib/keyglass/bridge/keyglass-bridge",
                "/usr/lib/keyglass/bridge/keyglass-autolaunch.sh",
            ],
        )
        .unwrap();
        assert_eq!(files["/usr/lib/keyglass/bridge/keyglass-bridge"], b"ELF");
        assert!(read_files(&ipk, &["/usr/bin/missing"])
            .unwrap_err()
            .contains("doesn't contain"));
        assert!(read_files(b"garbage", &[]).is_err());
    }
}
