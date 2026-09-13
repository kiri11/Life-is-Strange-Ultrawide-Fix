use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lis_ultrawide_core::games::Game;
use lis_ultrawide_core::games::true_colors::TRUE_COLORS;
use lis_ultrawide_core::pak::{Pak, build};
use lis_ultrawide_core::pak_ui::{build_mod, check_ui, install_ui, paths, restore_ui};

use lis_ultrawide_core::ui_layout::UiStatus;

fn fixture_dir() -> PathBuf {
    std::env::temp_dir().join(format!("lis-true-colors-pak-ui-{}", std::process::id()))
}

fn package_files(source: &mut Pak, fix: &lis_ultrawide_core::pak_ui::PakUiFix) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    for package in fix.packages {
        for ext in ["uasset", "uexp"] {
            let name = format!("{}.{}", package.path, ext);
            files.insert(name.clone(), source.read(&name).unwrap());
        }
    }
    files
}

fn write_source(dir: &Path, files: &BTreeMap<String, Vec<u8>>) {
    std::fs::write(dir.join("pakchunk0-WindowsNoEditor.pak"), build("../../../", files)).unwrap();
}

fn assert_variant(dir: &Path, fix: &lis_ultrawide_core::pak_ui::PakUiFix, files: &BTreeMap<String, Vec<u8>>, width: u32, height: u32) {
    let mut report = Vec::new();
    let (bytes, _) = build_mod(dir, fix, width, height, &mut report).unwrap();
    let path = dir.join(format!("variant-{width}-{height}.pak"));
    std::fs::write(&path, bytes).unwrap();
    let mut output = Pak::open(&path).unwrap();
    assert_eq!(output.entries.len(), files.len());
    for package in fix.packages {
        let header = format!("{}.uasset", package.path);
        let payload = format!("{}.uexp", package.path);
        assert_eq!(output.read(&header).unwrap(), files[&header]);
        let got = output.read(&payload).unwrap();
        let source = &files[&payload];
        assert_eq!(got.len(), source.len());
        let mut changed = Vec::new();
        for (at, (&a, &b)) in source.iter().zip(&got).enumerate() {
            if a != b {
                changed.push(at);
            }
        }
        let expected: Vec<usize> = package.edits.iter().flat_map(|e| e.offsets.iter().flat_map(|&at| at..at + 4)).collect();
        assert!(changed.iter().all(|at| expected.contains(at)), "{payload}: unexpected changes at {changed:?}");
        let (dw, dh) = lis_ultrawide_core::ui_layout::design_space(width, height, fix.design);
        for edit in package.edits {
            let value = (edit.old as f64 + (dw - fix.design.0) * edit.grow.0 + (dh - fix.design.1) * edit.grow.1) as f32;
            for &at in edit.offsets {
                assert_eq!(&got[at..at + 4], value.to_le_bytes().as_slice(), "{payload} at {at}");
            }
        }
    }
    let root = output.read("Siren/Content/UI/BP/Managers/UIWindowManager_BP.uexp").unwrap();
    let dimensions: (f32, f32) = match (width, height) {
        (5120, 2160) => (2560.0, 1080.0),
        (5120, 1440) => (3840.0, 1080.0),
        (1920, 1200) => (1920.0, 1200.0),
        _ => panic!("unexpected test display"),
    };
    assert_eq!(&root[15247..15251], &dimensions.0.to_le_bytes());
    assert_eq!(&root[15276..15280], &dimensions.1.to_le_bytes());
    drop(output);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn builds_variants_and_preserves_install_lifecycle() {
    let installed = PathBuf::from(r"D:\Games\Life is Strange True Colors\Siren\Content\Paks\pakchunk0-WindowsNoEditor.pak");
    if !installed.is_file() {
        eprintln!("skipped: no True Colors PAK at {}", installed.display());
        return;
    }
    let fix = TRUE_COLORS.pak_ui().unwrap();
    let mut source = Pak::open(&installed).unwrap();
    let files = package_files(&mut source, fix);
    let dir = fixture_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    write_source(&dir, &files);
    assert_variant(&dir, fix, &files, 5120, 2160);
    assert_variant(&dir, fix, &files, 5120, 1440);
    assert_variant(&dir, fix, &files, 1920, 1200);

    let mut report = Vec::new();
    install_ui(&dir, fix, 5120, 2160, &mut report).unwrap();
    let (pak_path, record) = paths(&dir, fix);
    assert_eq!(check_ui(Some(&dir), fix).0, UiStatus::Current);
    let before = std::fs::read(&pak_path).unwrap();
    let unrelated = dir.join("Mods/unrelated.pak");
    std::fs::write(&unrelated, b"keep").unwrap();

    let mut changed = files.clone();
    changed.insert("unrelated.bin".into(), b"index changed".to_vec());
    write_source(&dir, &changed);
    assert_eq!(check_ui(Some(&dir), fix).0, UiStatus::Stale);
    install_ui(&dir, fix, 5120, 1440, &mut report).unwrap();
    let installed_bytes = std::fs::read(&pak_path).unwrap();
    assert_ne!(before, installed_bytes, "reinstall must rebuild for the new display");
    assert_eq!(check_ui(Some(&dir), fix).0, UiStatus::Current);
    let mut corrupted = files.clone();
    let key = format!("{}.uexp", fix.packages[0].path);
    corrupted.get_mut(&key).unwrap()[15247] ^= 1;
    write_source(&dir, &corrupted);
    assert!(install_ui(&dir, fix, 5120, 2160, &mut report).is_err());
    assert_eq!(std::fs::read(&pak_path).unwrap(), installed_bytes);

    std::fs::remove_file(dir.join(fix.source)).unwrap();
    restore_ui(&dir, fix, &mut report).unwrap();
    assert!(!pak_path.exists() && !record.exists());
    assert_eq!(std::fs::read(&unrelated).unwrap(), b"keep");
    assert_eq!(check_ui(Some(&dir), fix).0, UiStatus::None);
    let _ = std::fs::remove_dir_all(&dir);
}
