//! Gives freki.exe its icon on Windows: turns `assets/freki.ico` into a compiled resource
//! file (.res) and hands it to the MSVC linker, which takes .res files as they are. No
//! resource compiler or extra crate needed. Other targets get their icons elsewhere (the
//! macOS app bundle, the Linux .desktop file) and skip this.

use std::env;
use std::fs;
use std::path::PathBuf;

const ICO: &str = "../../assets/freki.ico";
const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const LANGUAGE: u16 = 0x0409;

fn main() {
    println!("cargo:rerun-if-changed={ICO}");
    let windows = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let msvc = env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !(windows && msvc) {
        return;
    }
    let ico = fs::read(ICO).expect("assets/freki.ico");
    let res = icon_resources(&ico).expect("assets/freki.ico is not a valid icon file");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("freki.res");
    fs::write(&out, res).expect("writing freki.res");
    println!("cargo:rustc-link-arg-bins={}", out.display());
}

/// A .res file with each image of the .ico as an icon (ids 1, 2, …) and an icon group
/// (id 1, the one Explorer shows) listing them.
fn icon_resources(ico: &[u8]) -> Option<Vec<u8>> {
    let word = |at: usize| Some(u16::from_le_bytes(ico.get(at..at + 2)?.try_into().ok()?));
    let dword = |at: usize| Some(u32::from_le_bytes(ico.get(at..at + 4)?.try_into().ok()?));
    if word(0)? != 0 || word(2)? != 1 {
        return None;
    }
    let count = word(4)?;
    let mut res = Vec::new();
    // A .res file starts with an empty entry.
    resource(&mut res, 0, 0, 0, &[]);
    let mut group = Vec::new();
    group.extend([0u16, 1, count].iter().flat_map(|w| w.to_le_bytes()));
    for index in 0..count {
        // ICONDIRENTRY: width, height, colours, reserved, planes, bits, size, offset.
        let entry = 6 + 16 * usize::from(index);
        let size = dword(entry + 8)?;
        let offset = dword(entry + 12)? as usize;
        let image = ico.get(offset..offset + size as usize)?;
        let id = index + 1;
        resource(&mut res, RT_ICON, id, 0x1010, image);
        // GRPICONDIRENTRY: the same, with the resource id in place of the file offset.
        group.extend_from_slice(ico.get(entry..entry + 12)?);
        group.extend_from_slice(&id.to_le_bytes());
    }
    resource(&mut res, RT_GROUP_ICON, 1, 0x1030, &group);
    Some(res)
}

/// Appends one resource with a numeric type and name.
fn resource(res: &mut Vec<u8>, kind: u16, name: u16, flags: u16, data: &[u8]) {
    let size = u32::try_from(data.len()).expect("resource too big");
    res.extend(size.to_le_bytes());
    res.extend(32u32.to_le_bytes()); // header size
    for id in [kind, name] {
        res.extend(0xFFFFu16.to_le_bytes());
        res.extend(id.to_le_bytes());
    }
    res.extend(0u32.to_le_bytes()); // data version
    res.extend(flags.to_le_bytes());
    res.extend(if kind == 0 { 0 } else { LANGUAGE }.to_le_bytes());
    res.extend(0u32.to_le_bytes()); // version
    res.extend(0u32.to_le_bytes()); // characteristics
    res.extend_from_slice(data);
    res.resize(res.len().next_multiple_of(4), 0);
}
