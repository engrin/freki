//! Reading a ROM from disk: plain or zipped, with a soft patch applied.
//!
//! A `.zip` holds the first `.gb`/`.gbc`/`.sgb` file inside it. A patch next to the ROM
//! with the same name (`game.bps` or `game.ips` beside `game.gb` or `game.zip`) is applied
//! in memory; the ROM file itself is never changed. Battery saves and save states stay
//! named after the ROM file, so they are shared with the unpatched game.

use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

/// Extensions of ROM files picked from a zip.
const ROM_EXTENSIONS: [&str; 3] = ["gb", "gbc", "sgb"];
/// Patch extensions looked for next to a ROM, in order of preference.
const PATCH_EXTENSIONS: [&str; 2] = ["bps", "ips"];

pub struct Rom {
    pub data: Vec<u8>,
    /// The file inside a zip the ROM came from.
    pub zip_entry: Option<String>,
    /// The patch applied.
    pub patch: Option<PathBuf>,
}

impl Rom {
    /// What was loaded beyond the plain file, for status messages: e.g.
    /// ` (game.gb from the zip, patched with game.bps)`.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(entry) = &self.zip_entry {
            parts.push(format!("{entry} from the zip"));
        }
        if let Some(patch) = &self.patch {
            parts.push(format!("patched with {}", crate::session::file_name(patch)));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!(" ({})", parts.join(", "))
        }
    }
}

/// Reads the ROM at `path` (unzipping it if needed) and applies `patch`, or else a patch
/// found next to it.
pub fn load(path: &Path, patch: Option<&Path>) -> Result<Rom, String> {
    let bytes = read(path)?;
    let (mut data, zip_entry) = if bytes.starts_with(b"PK\x03\x04") {
        let (name, data) = unzip(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        (data, Some(name))
    } else {
        (bytes, None)
    };
    let patch = match patch {
        Some(p) => Some(p.to_path_buf()),
        None => PATCH_EXTENSIONS
            .iter()
            .map(|ext| path.with_extension(ext))
            .find(|p| p.is_file()),
    };
    if let Some(patch) = &patch {
        data = apply_patch(&data, &read(patch)?)
            .map_err(|e| format!("cannot apply {}: {e}", patch.display()))?;
    }
    Ok(Rom {
        data,
        zip_entry,
        patch,
    })
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// The first Game Boy ROM in a zip archive: its name and contents.
fn unzip(bytes: &[u8]) -> Result<(String, Vec<u8>), String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let index = (0..archive.len())
        .find(|&i| {
            archive.name_for_index(i).is_some_and(|name| {
                name.is_ok_and(|name| {
                    Path::new(name.as_ref()).extension().is_some_and(|ext| {
                        ROM_EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e))
                    })
                })
            })
        })
        .ok_or("no .gb or .gbc file in the zip")?;
    let mut file = archive.by_index(index).map_err(|e| e.to_string())?;
    let name = file.name().map_err(|e| e.to_string())?.into_owned();
    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .map_err(|e| format!("{name}: {e}"))?;
    Ok((name, data))
}

/// Applies an IPS or BPS patch to `rom`.
pub fn apply_patch(rom: &[u8], patch: &[u8]) -> Result<Vec<u8>, String> {
    if patch.starts_with(b"PATCH") {
        apply_ips(rom, patch)
    } else if patch.starts_with(b"BPS1") {
        apply_bps(rom, patch)
    } else {
        Err("not an IPS or BPS patch".into())
    }
}

/// IPS: records of (3-byte offset, 2-byte length, data), or run-length records with
/// length 0; then "EOF" and optionally a 3-byte size to truncate to.
fn apply_ips(rom: &[u8], patch: &[u8]) -> Result<Vec<u8>, String> {
    let truncated = || "patch is cut short".to_string();
    let mut out = rom.to_vec();
    let mut pos = 5;
    let mut take = |n: usize| -> Result<&[u8], String> {
        let bytes = patch.get(pos..pos + n).ok_or_else(truncated)?;
        pos += n;
        Ok(bytes)
    };
    let be = |b: &[u8]| b.iter().fold(0usize, |acc, &x| acc << 8 | x as usize);
    loop {
        let head = take(3)?;
        if head == b"EOF" {
            if let Ok(size) = take(3) {
                out.truncate(be(size));
            }
            return Ok(out);
        }
        let offset = be(head);
        let len = be(take(2)?);
        let data: Vec<u8> = if len == 0 {
            let run = be(take(2)?);
            vec![take(1)?[0]; run]
        } else {
            take(len)?.to_vec()
        };
        if out.len() < offset + data.len() {
            out.resize(offset + data.len(), 0);
        }
        out[offset..offset + data.len()].copy_from_slice(&data);
    }
}

/// BPS (beat): copies from the source, the patch or the output so far, with CRC32s of
/// source, target and patch in the 12-byte footer.
fn apply_bps(rom: &[u8], patch: &[u8]) -> Result<Vec<u8>, String> {
    if patch.len() < 4 + 12 {
        return Err("patch is cut short".into());
    }
    let footer = patch.len() - 12;
    let crc = |at: usize| u32::from_le_bytes(patch[at..at + 4].try_into().unwrap());
    if crc32fast::hash(&patch[..footer + 8]) != crc(footer + 8) {
        return Err("patch file is damaged (checksum mismatch)".into());
    }
    if crc32fast::hash(rom) != crc(footer) {
        return Err("patch is for a different ROM (checksum mismatch)".into());
    }
    let bad = || "patch is damaged or for a different ROM".to_string();
    // Variable-length numbers: 7 bits per byte, the last byte has bit 7 set.
    let read_number = |pos: &mut usize| -> Result<usize, String> {
        let mut value = 0usize;
        let mut shift = 1usize;
        loop {
            let byte = *patch[..footer].get(*pos).ok_or_else(bad)?;
            *pos += 1;
            value = value
                .checked_add((byte as usize & 0x7F) * shift)
                .ok_or_else(bad)?;
            if byte & 0x80 != 0 {
                return Ok(value);
            }
            shift = shift.checked_shl(7).ok_or_else(bad)?;
            value = value.checked_add(shift).ok_or_else(bad)?;
        }
    };
    let mut pos = 4;
    let source_size = read_number(&mut pos)?;
    let target_size = read_number(&mut pos)?;
    let metadata_size = read_number(&mut pos)?;
    if source_size != rom.len() {
        return Err("patch is for a different ROM (size mismatch)".into());
    }
    if target_size > 64 << 20 {
        return Err("patched ROM would be too large".into());
    }
    pos = pos.checked_add(metadata_size).ok_or_else(bad)?;
    let mut out = Vec::with_capacity(target_size);
    let (mut source_rel, mut target_rel) = (0usize, 0usize);
    let signed = |n: usize| -> isize {
        let magnitude = (n >> 1) as isize;
        if n & 1 != 0 { -magnitude } else { magnitude }
    };
    while pos < footer {
        let command = read_number(&mut pos)?;
        let len = (command >> 2) + 1;
        if out.len() + len > target_size {
            return Err(bad());
        }
        match command & 3 {
            0 => {
                // SourceRead: the source bytes at the same offset.
                let at = out.len();
                out.extend_from_slice(rom.get(at..at + len).ok_or_else(bad)?);
            }
            1 => {
                // TargetRead: bytes from the patch.
                out.extend_from_slice(patch.get(pos..pos + len).ok_or_else(bad)?);
                pos += len;
            }
            2 => {
                // SourceCopy: from a moving offset in the source.
                let offset = signed(read_number(&mut pos)?);
                source_rel = source_rel.checked_add_signed(offset).ok_or_else(bad)?;
                out.extend_from_slice(rom.get(source_rel..source_rel + len).ok_or_else(bad)?);
                source_rel += len;
            }
            _ => {
                // TargetCopy: from the output so far, byte by byte (runs may overlap).
                let offset = signed(read_number(&mut pos)?);
                target_rel = target_rel.checked_add_signed(offset).ok_or_else(bad)?;
                for _ in 0..len {
                    let byte = *out.get(target_rel).ok_or_else(bad)?;
                    out.push(byte);
                    target_rel += 1;
                }
            }
        }
    }
    if out.len() != target_size || crc32fast::hash(&out) != crc(footer + 4) {
        return Err("patched ROM has the wrong checksum".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ips_writes_records_runs_and_grows() {
        let rom = vec![0u8; 8];
        let mut patch = b"PATCH".to_vec();
        patch.extend([0, 0, 2, 0, 2, 0xAA, 0xBB]); // 2 bytes at 2
        patch.extend([0, 0, 9, 0, 0, 0, 3, 0x11]); // run of 3 × $11 at 9
        patch.extend(b"EOF");
        let out = apply_patch(&rom, &patch).unwrap();
        assert_eq!(out, [0, 0, 0xAA, 0xBB, 0, 0, 0, 0, 0, 0x11, 0x11, 0x11]);
        patch.extend([0, 0, 4]); // truncate to 4
        assert_eq!(apply_patch(&rom, &patch).unwrap(), [0, 0, 0xAA, 0xBB]);
        assert!(apply_patch(&rom, b"PATCH\x00\x00").is_err());
    }

    fn number(mut n: usize, out: &mut Vec<u8>) {
        loop {
            let x = (n & 0x7F) as u8;
            n >>= 7;
            if n == 0 {
                out.push(0x80 | x);
                return;
            }
            out.push(x);
            n -= 1;
        }
    }

    /// Builds a BPS patch from `actions` (already encoded) with correct checksums.
    fn bps(source: &[u8], target: &[u8], actions: &[u8]) -> Vec<u8> {
        let mut p = b"BPS1".to_vec();
        number(source.len(), &mut p);
        number(target.len(), &mut p);
        number(0, &mut p);
        p.extend_from_slice(actions);
        p.extend(crc32fast::hash(source).to_le_bytes());
        p.extend(crc32fast::hash(target).to_le_bytes());
        let crc = crc32fast::hash(&p);
        p.extend(crc.to_le_bytes());
        p
    }

    #[test]
    fn bps_applies_all_four_actions() {
        let source = b"ABCDEFGH";
        let target = b"ABxyFGHxyFGHGHG";
        let mut a = Vec::new();
        number((2 - 1) << 2, &mut a); // SourceRead 2: "AB"
        number((2 - 1) << 2 | 1, &mut a); // TargetRead 2: "xy"
        a.extend(b"xy");
        number((3 - 1) << 2 | 2, &mut a); // SourceCopy 3 from +5: "FGH"
        number(5 << 1, &mut a);
        number((5 - 1) << 2 | 3, &mut a); // TargetCopy 5 from +2: "xyFGH"
        number(2 << 1, &mut a);
        number((3 - 1) << 2 | 3, &mut a); // TargetCopy 3 from +3 (overlapping): "GHG"
        number(3 << 1, &mut a);
        let patch = bps(source, target, &a);
        assert_eq!(apply_patch(source, &patch).unwrap(), target);
        assert!(
            apply_patch(b"ABCDEFGX", &patch).is_err(),
            "wrong source ROM"
        );
    }

    #[test]
    fn loads_zipped_rom_and_patch_beside_it() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("freki-romfile-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("game.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&zip_path).unwrap());
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("readme.txt", opts).unwrap();
        zip.write_all(b"hello").unwrap();
        zip.start_file("Game (Europe).gb", opts).unwrap();
        zip.write_all(&[1, 2, 3, 4]).unwrap();
        zip.finish().unwrap();

        let rom = load(&zip_path, None).unwrap();
        assert_eq!(rom.data, [1, 2, 3, 4]);
        assert_eq!(rom.zip_entry.as_deref(), Some("Game (Europe).gb"));
        assert!(rom.patch.is_none());

        fs::write(dir.join("game.ips"), b"PATCH\x00\x00\x01\x00\x01\x09EOF").unwrap();
        let rom = load(&zip_path, None).unwrap();
        assert_eq!(rom.data, [1, 9, 3, 4]);
        assert!(rom.describe().contains("patched with game.ips"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
