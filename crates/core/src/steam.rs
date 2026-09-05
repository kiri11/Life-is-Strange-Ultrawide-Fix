//! Where Steam is, and where it keeps games: the installations named in the
//! registry and the usual folders, every library folder they have
//! registered in `libraryfolders.vdf`, the install folder a game's
//! `appmanifest_<id>.acf` names, and the Proton prefix of a game.
//!
//! The VDF reader is deliberately small: both files are flat `"key" "value"`
//! pairs, and only a few keys matter.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// `"key" "value"` pairs, in order, with nothing but whitespace between the
/// two strings: what `libraryfolders.vdf` and `appmanifest_*.acf` are made of.
pub fn vdf_pairs(text: &str) -> Vec<(String, String)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let quoted = |mut i: usize| -> Option<(String, usize)> {
        if b.get(i) != Some(&b'"') {
            return None;
        }
        i += 1;
        let start = i;
        while i < b.len() && b[i] != b'"' {
            if b[i] == b'\\' {
                i += 1;
            }
            i += 1;
        }
        let raw = String::from_utf8_lossy(&b[start..i.min(b.len())]).replace("\\\\", "\\").replace("\\\"", "\"");
        Some((raw, i + 1))
    };
    while i < b.len() {
        if let Some((key, mut j)) = quoted(i) {
            while j < b.len() && matches!(b[j], b' ' | b'\t') {
                j += 1;
            }
            if let Some((value, k)) = quoted(j) {
                out.push((key, value));
                i = k;
                continue;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// The first value of `key`.
pub fn vdf_value(text: &str, key: &str) -> Option<String> {
    vdf_pairs(text).into_iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// The library paths a `libraryfolders.vdf` names: the current format keys
/// each entry `"path"`; the pre-2021 one used numbers.
pub fn library_folders(text: &str) -> Vec<String> {
    vdf_pairs(text)
        .into_iter()
        .filter(|(k, v)| k == "path" || (!k.is_empty() && k.bytes().all(|c| c.is_ascii_digit()) && v.len() >= 3))
        .map(|(_, v)| v)
        .collect()
}

/// A comparison key for paths: case-folded and separator-normalised on
/// Windows, as they are on Linux.
pub fn normcase(p: &Path) -> String {
    let s = p.to_string_lossy();
    if cfg!(windows) { s.replace('/', "\\").to_lowercase() } else { s.into_owned() }
}

/// Identity for discovery, resolving symlinks and (on Unix) bind-mount aliases.
/// Keep the original path for use: its Steam library ancestry locates Proton.
#[derive(Debug, PartialEq, Eq, Hash)]
pub enum PathKey {
    #[cfg(unix)]
    File(u64, u64),
    Path(String),
}

pub fn path_key(path: &Path) -> PathKey {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(meta) = std::fs::metadata(path) {
            return PathKey::File(meta.dev(), meta.ino());
        }
    }
    let resolved = std::fs::canonicalize(path).or_else(|_| std::path::absolute(path)).unwrap_or_else(|_| path.to_path_buf());
    PathKey::Path(normcase(&resolved))
}

/// `parent/name`, matched case-insensitively (Linux has `SteamApps` and `steamapps`).
pub fn child(parent: &Path, name: &str) -> PathBuf {
    let direct = parent.join(name);
    if direct.exists() {
        return direct;
    }
    if let Ok(entries) = std::fs::read_dir(parent) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().eq_ignore_ascii_case(name) {
                return e.path();
            }
        }
    }
    direct
}

pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()))
        .map(PathBuf::from)
}

/// Every fixed drive's root on Windows; `/` elsewhere.
pub fn fixed_drives() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let mut drives = Vec::new();
        let mask = unsafe { win::GetLogicalDrives() };
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let root = format!("{}:\\", (b'A' + i as u8) as char);
            let wide: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
            if unsafe { win::GetDriveTypeW(wide.as_ptr()) } == win::DRIVE_FIXED {
                drives.push(PathBuf::from(root));
            }
        }
        if drives.is_empty() {
            drives.push(PathBuf::from("C:\\"));
        }
        drives
    }
    #[cfg(not(windows))]
    {
        vec![PathBuf::from("/")]
    }
}

#[cfg(windows)]
mod win {
    use core::ffi::c_void;

    pub const DRIVE_FIXED: u32 = 3;
    pub const HKEY_CURRENT_USER: usize = 0x8000_0001;
    pub const HKEY_LOCAL_MACHINE: usize = 0x8000_0002;
    pub const KEY_READ: u32 = 0x2_0019;
    pub const REG_SZ: u32 = 1;

    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        pub fn GetLogicalDrives() -> u32;
        pub fn GetDriveTypeW(root: *const u16) -> u32;
    }

    #[link(name = "advapi32", kind = "raw-dylib")]
    unsafe extern "system" {
        pub fn RegOpenKeyExW(key: usize, sub: *const u16, options: u32, access: u32, out: *mut usize) -> i32;
        pub fn RegQueryValueExW(key: usize, name: *const u16, reserved: *mut u32, kind: *mut u32, data: *mut u8, len: *mut u32) -> i32;
        pub fn RegCloseKey(key: usize) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// A string value from the registry, or None.
    pub fn registry_string(hive: usize, key: &str, name: &str) -> Option<String> {
        let mut h: usize = 0;
        if unsafe { RegOpenKeyExW(hive, wide(key).as_ptr(), 0, KEY_READ, &mut h) } != 0 {
            return None;
        }
        let mut kind = 0u32;
        let mut len = 0u32;
        let vname = wide(name);
        let mut out = None;
        if unsafe { RegQueryValueExW(h, vname.as_ptr(), core::ptr::null_mut(), &mut kind, core::ptr::null_mut(), &mut len) } == 0
            && (kind == REG_SZ || kind == 2)
            && len > 0
        {
            let mut buf = vec![0u8; len as usize + 2];
            if unsafe { RegQueryValueExW(h, vname.as_ptr(), core::ptr::null_mut(), &mut kind, buf.as_mut_ptr(), &mut len) } == 0 {
                let units: Vec<u16> = buf[..len as usize].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
                out = Some(String::from_utf16_lossy(&units[..end]));
            }
        }
        unsafe { RegCloseKey(h) };
        out.filter(|s| !s.is_empty())
    }

    #[allow(dead_code)]
    fn _unused(_: *mut c_void) {}
}

fn add(list: &mut Vec<PathBuf>, seen: &mut HashSet<PathKey>, path: PathBuf) {
    if path.as_os_str().is_empty() || !path.is_dir() {
        return;
    }
    if seen.insert(path_key(&path)) {
        list.push(path);
    }
}

/// Every Steam installation this machine knows about.
pub fn installs() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut seen = HashSet::new();
    #[cfg(windows)]
    {
        for (hive, key) in [
            (win::HKEY_CURRENT_USER, "Software\\Valve\\Steam"),
            (win::HKEY_LOCAL_MACHINE, "SOFTWARE\\WOW6432Node\\Valve\\Steam"),
            (win::HKEY_LOCAL_MACHINE, "SOFTWARE\\Valve\\Steam"),
        ] {
            for value in ["SteamPath", "InstallPath"] {
                if let Some(p) = win::registry_string(hive, key, value) {
                    add(&mut roots, &mut seen, PathBuf::from(p));
                }
            }
        }
    }
    if let Some(home) = home() {
        for rel in [
            ".steam/steam",
            ".steam/root",
            ".local/share/Steam",
            ".var/app/com.valvesoftware.Steam/data/Steam",
            "snap/steam/common/.local/share/Steam",
            "Library/Application Support/Steam",
        ] {
            add(&mut roots, &mut seen, home.join(rel));
        }
    }
    for drive in fixed_drives() {
        for rel in ["Program Files (x86)/Steam", "Program Files/Steam", "Steam"] {
            add(&mut roots, &mut seen, drive.join(rel));
        }
    }
    roots
}

/// Steam installations plus every library folder they have registered.
pub fn libraries() -> Vec<PathBuf> {
    let mut libraries = installs();
    let mut seen: HashSet<PathKey> = libraries.iter().map(|p| path_key(p)).collect();
    for install in libraries.clone() {
        let vdf = child(&install, "steamapps").join("libraryfolders.vdf");
        let Ok(text) = std::fs::read_to_string(&vdf) else { continue };
        for raw in library_folders(&text) {
            add(&mut libraries, &mut seen, PathBuf::from(raw));
        }
    }
    libraries
}

/// `<library>/steamapps/common/<game>/.../x.exe -> <library>`, or None.
pub fn library_of(exe: &Path) -> Option<PathBuf> {
    let mut path = exe.parent()?;
    loop {
        let parent = path.parent()?;
        if path.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("steamapps")) {
            return Some(parent.to_path_buf());
        }
        path = parent;
    }
}

/// Where Steam keeps a game's Proton prefix for this library.
pub fn proton_prefix(library: &Path, appid: u32) -> PathBuf {
    child(library, "steamapps").join("compatdata").join(appid.to_string()).join("pfx")
}

/// The folder name `appmanifest_<appid>.acf` says the game is installed in.
pub fn install_dir_from_manifest(steamapps: &Path, appid: u32) -> Option<String> {
    let text = std::fs::read_to_string(steamapps.join(format!("appmanifest_{appid}.acf"))).ok()?;
    vdf_value(&text, "installdir")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_library_folder_formats_and_manifests() {
        let modern = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t\t\"label\"\t\t\"\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"1874000\"\t\t\"123\"\n\t\t}\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}\n";
        let folders = library_folders(modern);
        assert!(folders.contains(&"C:\\Program Files (x86)\\Steam".to_string()));
        assert!(folders.contains(&"D:\\SteamLibrary".to_string()));
        // the numeric app key with a short value is not a library path;
        // one with a long value is a candidate, as the old format used
        assert!(folders.contains(&"123".to_string()));
        let old = "\"LibraryFolders\"\n{\n\t\"TimeNextStatsReport\"\t\t\"1\"\n\t\"1\"\t\t\"/home/deck/Games\"\n}\n";
        assert_eq!(library_folders(old), vec!["/home/deck/Games".to_string()]);
        let manifest = "\"AppState\"\n{\n\t\"appid\"\t\t\"1874000\"\n\t\"installdir\"\t\t\"LifeIsStrangeDoubleExposure\"\n}\n";
        assert_eq!(vdf_value(manifest, "installdir").as_deref(), Some("LifeIsStrangeDoubleExposure"));
        assert_eq!(vdf_value(manifest, "nope"), None);
    }

    #[test]
    fn library_of_walks_up_to_steamapps() {
        let exe = Path::new("/lib/SteamApps/common/Game/Chronos/Binaries/Win64/x.exe");
        assert_eq!(library_of(exe), Some(PathBuf::from("/lib")));
        assert_eq!(library_of(Path::new("/elsewhere/x.exe")), None);
    }
}
