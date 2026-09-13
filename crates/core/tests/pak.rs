use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lis_ultrawide_core::hash;
use lis_ultrawide_core::pak::{Pak, build};

fn temp_pak(name: &str, bytes: &[u8]) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("lis-pak-{name}-{}-{}.pak", std::process::id(), std::thread::current().name().unwrap_or("test")));
    std::fs::write(&path, bytes).unwrap();
    path
}

fn with_pak<T>(name: &str, bytes: &[u8], f: impl FnOnce(&Path) -> T) -> T {
    let path = temp_pak(name, bytes);
    let result = f(&path);
    let _ = std::fs::remove_file(path);
    result
}

fn index_offset(pak: &[u8]) -> usize {
    let footer = pak.len() - 222;
    u64::from_le_bytes(pak[footer + 25..footer + 33].try_into().unwrap()) as usize
}

fn update_index_digest(pak: &mut [u8]) {
    let at = index_offset(pak);
    let footer = pak.len() - 222;
    let size = u64::from_le_bytes(pak[footer + 33..footer + 41].try_into().unwrap()) as usize;
    let digest = hash::sha1(&pak[at..at + size]);
    let footer = pak.len() - 222;
    pak[footer + 41..footer + 61].copy_from_slice(&digest);
}

#[test]
fn builder_roundtrips_entries_and_fingerprint() {
    let mut files = BTreeMap::new();
    files.insert("Content/UI/a.uasset".into(), b"asset bytes".to_vec());
    files.insert("Content/UI/a.uexp".into(), (0..=255).collect());
    let bytes = build("/Game", &files);

    with_pak("roundtrip", &bytes, |path| {
        let mut pak = Pak::open(path).unwrap();
        assert_eq!(pak.mount, "/Game");
        assert_eq!(pak.entries.len(), files.len());
        assert!(!pak.fingerprint.is_empty());
        for (name, expected) in files {
            assert_eq!(pak.read(&name).unwrap(), expected);
        }
        assert!(pak.read("missing").unwrap_err().contains("entry missing"));
    });
}

#[test]
fn rejects_corrupt_entry_checksum() {
    let mut files = BTreeMap::new();
    files.insert("asset.bin".into(), b"checksum me".to_vec());
    let mut bytes = build("/Game", &files);
    bytes[53] ^= 1;
    with_pak("checksum", &bytes, |path| {
        let mut pak = Pak::open(path).unwrap();
        assert!(pak.read("asset.bin").unwrap_err().contains("checksum mismatch"));
    });
}

#[test]
fn rejects_invalid_uncompressed_length_and_method() {
    let mut files = BTreeMap::new();
    files.insert("asset.bin".into(), b"payload".to_vec());
    let original = build("/Game", &files);
    let idx = index_offset(&original);
    let entry = idx + 4 + "/Game".len() + 1 + 4 + 4 + "asset.bin".len() + 1;

    let mut bad_size = original.clone();
    bad_size[entry + 16..entry + 24].copy_from_slice(&8u64.to_le_bytes());
    update_index_digest(&mut bad_size);
    with_pak("length", &bad_size, |path| {
        let mut pak = Pak::open(path).unwrap();
        assert!(pak.read("asset.bin").unwrap_err().contains("invalid uncompressed pak size"));
    });

    let mut bad_method = original;
    bad_method[entry + 24..entry + 28].copy_from_slice(&2u32.to_le_bytes());
    update_index_digest(&mut bad_method);
    with_pak("method", &bad_method, |path| {
        let error = match Pak::open(path) {
            Ok(_) => panic!("unsupported method accepted"),
            Err(error) => error,
        };
        assert!(error.contains("truncated pak index") || error.contains("invalid pak block count"));
    });
}

#[test]
fn reads_true_colors_ui_window_manager_when_installed() {
    let path = PathBuf::from(r"D:\Games\Life is Strange True Colors\Siren\Content\Paks\pakchunk0-WindowsNoEditor.pak");
    if !path.is_file() {
        eprintln!("skipped: no True Colors PAK at {}", path.display());
        return;
    }
    let mut pak = Pak::open(&path).unwrap();
    for name in ["Siren/Content/UI/BP/Managers/UIWindowManager_BP.uasset", "Siren/Content/UI/BP/Managers/UIWindowManager_BP.uexp"] {
        let data = pak.read(name).unwrap();
        assert!(!data.is_empty(), "{name}");
    }
}
