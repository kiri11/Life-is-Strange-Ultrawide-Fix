//! The ultrawide camera part: the loader library next to the game
//! executable, and under Proton its registration with Wine.
//!
//! The camera fix is applied to the game's code in memory, at every launch,
//! by a small library the game loads by itself. It is installed as
//! `winhttp.dll` next to the game executable: the game imports a DLL of that
//! name, and Windows looks for it in the game's own folder first. The
//! executable on disk is never modified, so Steam's Verify Integrity, game
//! updates and reinstalls leave the fix in place, and there is nothing to
//! back up or restore.
//!
//! The library reports what it did in `LiSUltrawideCamera.log` next to
//! itself. That is the only place the installer can learn whether the
//! game's build is one the fix knows: the signatures live in the loader.

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::games::Game;
use crate::report::{InstallError, Report, Result, replace_file, replace_file_from, write_failure};
use crate::wine;

/// In the loader's version resource: how the fix's DLL is told from
/// another mod's winhttp.dll.
pub const DLL_MARKER: &str = "LiSUltrawideCamera";
pub const CAMERA_INI: &str = "LiSUltrawideCamera.ini";
pub const CAMERA_LOG: &str = "LiSUltrawideCamera.log";

pub struct CameraPaths {
    pub dll: PathBuf,
    pub ini: PathBuf,
    pub log: PathBuf,
}

/// The installed loader, its ini and its log, all next to the game executable.
pub fn camera_paths(game: &dyn Game, exe: &Path) -> CameraPaths {
    let win64 = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    CameraPaths { dll: win64.join(game.proxy_dlls()[0]), ini: win64.join(CAMERA_INI), log: win64.join(CAMERA_LOG) }
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

/// Does this file carry the loader's name in its version resource?
///
/// A winhttp.dll next to the game could also be some other mod's loader, and
/// the fix must neither overwrite nor delete that one.
pub fn is_our_dll(path: &Path) -> bool {
    std::fs::read(path).map(|d| find(&d, &utf16(DLL_MARKER)).is_some()).unwrap_or(false)
}

/// What a DLL's version resource says it is - "Ultimate ASI Loader
/// (ThirteenAG)" - or None when it says nothing. Each string in the resource
/// is UTF-16 and sits right after its key, padded to four bytes, so this
/// needs no walk of the resource tree.
pub fn describe_dll(path: &Path) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    let string = |key: &str| -> Option<String> {
        let mut needle = utf16(key);
        needle.extend([0, 0]);
        let mut at = find(&data, &needle)? + needle.len();
        while data.get(at..at + 2) == Some(&[0, 0]) {
            at += 2; // alignment padding
        }
        let mut units = Vec::new();
        while units.len() < 100 {
            let Some(pair) = data.get(at..at + 2) else { break };
            if pair == [0, 0] {
                break;
            }
            units.push(u16::from_le_bytes([pair[0], pair[1]]));
            at += 2;
        }
        let text = String::from_utf16_lossy(&units).trim().to_string();
        (!text.is_empty() && !text.chars().any(char::is_control)).then_some(text)
    };
    let what = string("FileDescription").or_else(|| string("ProductName"));
    let company = string("CompanyName");
    match (what, company) {
        (Some(w), Some(c)) if !w.contains(&c) => Some(format!("{w} ({c})")),
        (Some(w), _) => Some(w),
        (None, c) => c,
    }
}

/// Is the file exactly these bytes?
pub fn file_equals(path: &Path, bytes: &[u8]) -> bool {
    let Ok(meta) = std::fs::metadata(path) else { return false };
    if meta.len() != bytes.len() as u64 {
        return false;
    }
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let mut buf = vec![0u8; 1 << 20];
    let mut at = 0;
    loop {
        let Ok(n) = f.read(&mut buf) else { return false };
        if n == 0 {
            return at == bytes.len();
        }
        if bytes.get(at..at + n) != Some(&buf[..n]) {
            return false;
        }
        at += n;
    }
}

/// The loader's verdict from the last launch, or None if it has not run.
pub fn last_launch(log: &Path) -> Option<String> {
    let text = std::fs::read(log).ok()?;
    let text = String::from_utf8_lossy(&text);
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines
        .iter()
        .rev()
        .find(|l| l.starts_with("applied") || l.starts_with("not applied"))
        .or(lines.last())
        .map(|l| l.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraStatus {
    /// the loader next to the game is the one shipped here
    Installed,
    /// it is this fix's loader, but not the version shipped here
    Outdated,
    /// some other program's winhttp.dll is there; never touched
    Foreign,
    /// not installed
    None,
    /// the path is not the game's executable
    NotGame,
    /// nothing readable at that path
    Missing,
}

impl CameraStatus {
    /// The word the Windows front-end reads on the `status:` line.
    pub fn as_str(self) -> &'static str {
        match self {
            CameraStatus::Installed => "installed",
            CameraStatus::Outdated => "outdated",
            CameraStatus::Foreign => "foreign",
            CameraStatus::None => "none",
            CameraStatus::NotGame => "notgame",
            CameraStatus::Missing => "missing",
        }
    }
}

/// Classify the camera part. `shipped` is the loader this installer carries.
pub fn check_camera(game: &dyn Game, exe: &Path, shipped: Option<&[u8]>) -> (CameraStatus, String) {
    if !exe.is_file() {
        return (CameraStatus::Missing, "there is no file at that path".into());
    }
    if !exe.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(game.exe_name())) {
        // the caller may have fallen back to the first game for an executable
        // no game claims, so name every one the fix knows
        let names: Vec<&str> = crate::games::GAMES.iter().map(|g| g.exe_name()).collect();
        return (CameraStatus::NotGame, format!("that is not {} - select the game's own executable", names.join(" or ")));
    }
    let paths = camera_paths(game, exe);
    let dll_name = game.proxy_dlls()[0];
    if !paths.dll.is_file() {
        return (CameraStatus::None, "not installed".into());
    }
    if !is_our_dll(&paths.dll) {
        let what = describe_dll(&paths.dll).map(|w| format!(" ({w})")).unwrap_or_default();
        return (
            CameraStatus::Foreign,
            format!("another program's {dll_name}{what} is next to the game, and the fix will not replace it"),
        );
    }
    let tail = match last_launch(&paths.log) {
        Some(l) => format!(" - last launch: {l}"),
        None => " - the game has not been started since".to_string(),
    };
    if shipped.is_some_and(|s| !file_equals(&paths.dll, s)) {
        return (CameraStatus::Outdated, format!("a different version of the loader is installed{tail}"));
    }
    (CameraStatus::Installed, format!("loader installed{tail}"))
}

/// (file size, PE timestamp, image size) - or None if it is not a PE file.
///
/// What tells one build of the game from another: the old in-place patch
/// kept both, so it says whether a backup belongs to the executable next to it.
pub fn build_identity(path: &Path) -> Option<(u64, u32, u32)> {
    let size = std::fs::metadata(path).ok()?.len();
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = vec![0u8; 0x400];
    let n = f.read(&mut head).ok()?;
    head.truncate(n);
    if head.len() < 0x40 || &head[..2] != b"MZ" {
        return None;
    }
    let pe = u32::from_le_bytes(head[0x3C..0x40].try_into().unwrap()) as usize;
    if pe + 0x60 > head.len() || &head[pe..pe + 4] != b"PE\0\0" {
        return None;
    }
    let at = |o: usize| u32::from_le_bytes(head[o..o + 4].try_into().unwrap());
    Some((size, at(pe + 8), at(pe + 24 + 56)))
}

/// Two files with the same bytes, compared a megabyte at a time: one of
/// them is the game's executable.
fn same_file_bytes(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else { return false };
    if ma.len() != mb.len() {
        return false;
    }
    let (Ok(mut fa), Ok(mut fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else { return false };
    let mut x = vec![0u8; 1 << 20];
    let mut y = vec![0u8; 1 << 20];
    loop {
        let Ok(n) = fa.read(&mut x) else { return false };
        if n == 0 {
            return true;
        }
        if fb.read_exact(&mut y[..n]).is_err() || x[..n] != y[..n] {
            return false;
        }
    }
}

/// Undo what versions before 2026.09 did to the executable itself.
///
/// Those versions edited the executable in place and kept the stock file
/// next to it as `.exe.original`. The stock file goes back and the backup
/// goes, because the loader patches the game in memory and refuses an
/// executable that is already patched on disk.
pub fn retire_exe_patch(exe: &Path, r: &mut dyn Report) -> Result<()> {
    let mut backup = exe.as_os_str().to_owned();
    backup.push(".original");
    let backup = PathBuf::from(backup);
    if !backup.is_file() {
        return Ok(());
    }
    let name = backup.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let theirs = build_identity(&backup);
    if theirs.is_none() || theirs != build_identity(exe) {
        r.line(&format!("  note: {name} belongs to a different build of the game; it is not needed any more and can be deleted"));
        return Ok(());
    }
    if same_file_bytes(exe, &backup) {
        r.line("  the executable is already the stock one");
    } else {
        r.line(&format!("  an older version of this fix edited the executable - putting the stock file back from {name}"));
        replace_file_from(exe, &backup)?;
    }
    match std::fs::remove_file(&backup) {
        Ok(()) => r.line(&format!("  removed {name} - the loader needs no backup")),
        Err(e) => r.line(&format!("  note: could not remove {name} ({e}) - it can be deleted by hand")),
    }
    Ok(())
}

/// SUWSF would re-apply in-memory aspect patches on top of the fix and
/// poison its Hor+ maths; if it is there, it is switched off.
pub fn disable_suwsf(exe: &Path, r: &mut dyn Report) {
    let Some(dir) = exe.parent() else { return };
    let ini = dir.join("SUWSF.ini");
    if !ini.is_file() {
        return;
    }
    let attempt = (|| -> std::io::Result<bool> {
        let content = String::from_utf8_lossy(&std::fs::read(&ini)?).into_owned();
        if !content.contains("Enabled=true") {
            return Ok(false);
        }
        std::fs::write(&ini, content.replace("Enabled=true", "Enabled=false"))?;
        Ok(true)
    })();
    match attempt {
        Ok(true) => r.line("  disabled conflicting SUWSF.ini in-memory patches"),
        Ok(false) => {}
        Err(e) => r.line(&format!("  note: could not disable SUWSF.ini ({e}) - if you have that tool, turn it off by hand.")),
    }
}

/// Put the loader next to the game and, under Proton, register it with Wine.
///
/// `explicit` is the resolution when it was chosen by hand rather than
/// detected: then it is written to the loader's ini, otherwise the loader
/// reads the primary display itself at every launch, so a new monitor needs
/// no reinstall.
pub fn install_camera(
    game: &dyn Game,
    exe: &Path,
    shipped: Option<&[u8]>,
    explicit: Option<(u32, u32)>,
    engine_ini: Option<&Path>,
    r: &mut dyn Report,
) -> Result<()> {
    let Some(shipped) = shipped else {
        return Err(InstallError(
            "this build of the installer carries no loader library - build the loader first \
             (cargo build --release -p lis-ultrawide-loader), then the installer"
                .into(),
        ));
    };
    retire_exe_patch(exe, r)?;
    let paths = camera_paths(game, exe);
    let dll_name = game.proxy_dlls()[0];
    if paths.dll.is_file() && !is_our_dll(&paths.dll) {
        let what = match describe_dll(&paths.dll) {
            Some(w) => format!(" - it says it is {w}"),
            None => " - another mod's loader, probably".to_string(),
        };
        return Err(InstallError(format!(
            "there is already a {dll_name} next to the game that is not this fix's{what}. The fix needs that name: \
             move the other file away and run this again, and please report which mod it belongs to."
        )));
    }
    if paths.dll.is_file() && file_equals(&paths.dll, shipped) {
        r.line(&format!("  loader already in place: {}", paths.dll.display()));
    } else {
        replace_file(&paths.dll, shipped)?;
        r.line(&format!("  installed the loader as {}", paths.dll.display()));
    }
    if let Some((w, h)) = explicit {
        replace_file(
            &paths.ini,
            format!(
                "; Written by the LiS Ultrawide Fix installer. Without this file the\n\
                 ; loader reads the primary display's resolution at every launch.\n\
                 Width={w}\nHeight={h}\n"
            )
            .as_bytes(),
        )?;
        r.line(&format!("  {CAMERA_INI}: {w}x{h}"));
    } else if paths.ini.is_file() {
        std::fs::remove_file(&paths.ini).map_err(|e| InstallError(write_failure(&paths.ini, &e)))?;
        r.line(&format!("  removed {CAMERA_INI} - the loader reads the display at launch"));
    }
    if paths.log.is_file() {
        let _ = std::fs::remove_file(&paths.log); // the next launch writes a fresh one
    }
    if !cfg!(windows) {
        wine::apply_dll_override(game, exe, dll_name, engine_ini, false, r)?;
    }
    disable_suwsf(exe, r);
    Ok(())
}

pub fn remove_camera(game: &dyn Game, exe: &Path, engine_ini: Option<&Path>, r: &mut dyn Report) -> Result<()> {
    retire_exe_patch(exe, r)?;
    let paths = camera_paths(game, exe);
    let dll_name = game.proxy_dlls()[0];
    if paths.dll.is_file() {
        if is_our_dll(&paths.dll) {
            std::fs::remove_file(&paths.dll).map_err(|e| InstallError(write_failure(&paths.dll, &e)))?;
            r.line(&format!("  removed the loader {}", paths.dll.display()));
        } else {
            r.line(&format!("  left alone: the {dll_name} next to the game is not this fix's"));
        }
    }
    for p in [&paths.ini, &paths.log] {
        if p.is_file() {
            let _ = std::fs::remove_file(p);
        }
    }
    if !cfg!(windows) {
        wine::apply_dll_override(game, exe, dll_name, engine_ini, true, r)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::games::double_exposure::DOUBLE_EXPOSURE;

    #[test]
    fn loader_status_without_a_game_or_a_dll() {
        let game: &dyn Game = &DOUBLE_EXPOSURE;
        let tmp = std::env::temp_dir().join(format!("lis-camera-test-{}", std::process::id()));
        let win64 = tmp.join("Chronos").join("Binaries").join("Win64");
        std::fs::create_dir_all(&win64).unwrap();
        let exe = win64.join(game.exe_name());
        assert_eq!(check_camera(game, &exe, None).0, CameraStatus::Missing, "no executable: missing");
        std::fs::write(&exe, b"MZ").unwrap();
        assert_eq!(check_camera(game, &win64.join("other.exe"), None).0, CameraStatus::Missing);
        let other = win64.join("Chronos.exe");
        std::fs::write(&other, b"MZ").unwrap();
        assert_eq!(check_camera(game, &other, None).0, CameraStatus::NotGame, "another executable: notgame");
        assert_eq!(check_camera(game, &exe, None).0, CameraStatus::None, "no loader: none");

        let paths = camera_paths(game, &exe);
        std::fs::write(&paths.dll, b"MZ some other mod").unwrap();
        assert!(!is_our_dll(&paths.dll), "a foreign winhttp.dll is not ours");
        assert_eq!(check_camera(game, &exe, None).0, CameraStatus::Foreign);
        assert_eq!(describe_dll(&paths.dll), None, "no version resource: nothing to describe");
        // the strings of a version resource: key, terminator, padding to 4, value
        let mut res = b"MZ".to_vec();
        res.extend(utf16("FileDescription"));
        res.extend([0, 0, 0, 0]);
        res.extend(utf16("Some Loader"));
        res.extend([0, 0]);
        res.extend(utf16("CompanyName"));
        res.extend([0, 0]);
        res.extend(utf16("Someone"));
        res.extend([0, 0]);
        std::fs::write(&paths.dll, &res).unwrap();
        assert_eq!(describe_dll(&paths.dll).as_deref(), Some("Some Loader (Someone)"));
        assert!(check_camera(game, &exe, None).1.contains("Some Loader"), "the status names the other program");
        let mut ours = b"MZ".to_vec();
        ours.extend(utf16(DLL_MARKER));
        ours.push(b'!');
        std::fs::write(&paths.dll, &ours).unwrap();
        assert!(is_our_dll(&paths.dll), "the marker identifies our loader");
        let (status, detail) = check_camera(game, &exe, None);
        assert_eq!(status, CameraStatus::Installed);
        assert!(detail.contains("not been started"), "{detail}");
        assert_eq!(check_camera(game, &exe, Some(b"different")).0, CameraStatus::Outdated);
        assert_eq!(check_camera(game, &exe, Some(&ours)).0, CameraStatus::Installed);
        std::fs::write(&paths.log, "LiS Ultrawide Fix camera loader dev - now\n  note\napplied 6 writes - the fix is active\n").unwrap();
        assert_eq!(last_launch(&paths.log).as_deref(), Some("applied 6 writes - the fix is active"));
        assert!(check_camera(game, &exe, None).1.contains("last launch: applied 6 writes"));

        // the old in-place patch: a matching backup is restored and dropped
        let mut header = vec![0u8; 0x200];
        header[..2].copy_from_slice(b"MZ");
        header[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        header[0x80..0x84].copy_from_slice(b"PE\0\0");
        header[0x88..0x8C].copy_from_slice(&12345u32.to_le_bytes()); // timestamp
        header[0x80 + 24 + 56..0x80 + 24 + 60].copy_from_slice(&0x1000u32.to_le_bytes()); // image size
        let mut stock = header.clone();
        stock.extend(b"stock");
        let mut patched = header.clone();
        patched.extend(b"patch");
        let backup = win64.join(format!("{}.original", game.exe_name()));
        std::fs::write(&backup, &stock).unwrap();
        std::fs::write(&exe, &patched).unwrap();
        assert_eq!(build_identity(&exe), build_identity(&backup), "same size and header: same build");
        let mut r = Vec::new();
        retire_exe_patch(&exe, &mut r).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), stock, "the stock executable came back from the backup");
        assert!(!backup.exists(), "the backup is gone");
        // a backup of another build is left alone
        let mut longer = stock.clone();
        longer.extend(b"longer");
        std::fs::write(&backup, &longer).unwrap();
        retire_exe_patch(&exe, &mut r).unwrap();
        assert!(std::fs::read(&exe).unwrap() == stock && backup.exists(), "a backup of a different build is neither restored nor removed");

        // install and remove, with the embedded loader stand-in
        std::fs::remove_file(&backup).unwrap();
        let mut r = Vec::new();
        // Keep Wine lookup inside the fixture even on a machine with the game installed.
        let engine_ini = tmp.join("Engine.ini");
        let engine_ini = Some(engine_ini.as_path());
        install_camera(game, &exe, Some(&ours), Some((5120, 2160)), engine_ini, &mut r).unwrap();
        assert!(file_equals(&paths.dll, &ours));
        assert!(std::fs::read_to_string(&paths.ini).unwrap().contains("Width=5120\nHeight=2160\n"));
        install_camera(game, &exe, Some(&ours), None, engine_ini, &mut r).unwrap();
        assert!(!paths.ini.exists(), "a detected resolution removes the ini");
        assert!(install_camera(game, &exe, None, None, engine_ini, &mut r).is_err(), "no embedded loader: refused");
        remove_camera(game, &exe, engine_ini, &mut r).unwrap();
        assert!(!paths.dll.exists());
        std::fs::write(&paths.dll, b"MZ foreign").unwrap();
        assert!(install_camera(game, &exe, Some(&ours), None, engine_ini, &mut r).unwrap_err().0.contains("not this fix's"));
        remove_camera(game, &exe, engine_ini, &mut r).unwrap();
        assert!(paths.dll.exists(), "a foreign loader is left alone");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
