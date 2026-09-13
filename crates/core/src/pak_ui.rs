use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::report::{InstallError, Report, Result, replace_file, write_failure};
use crate::ui_layout::{UiStatus, design_space, sha256_file};
use crate::{hash, json, pak, to_hex};

pub struct FloatEdit {
    pub label: &'static str,
    pub offsets: &'static [usize],
    pub old: f32,
    /// Fractions of the additional design width and height to add.
    pub grow: (f64, f64),
}

pub struct PackageEdit {
    pub path: &'static str,
    pub header_sha256: &'static str,
    pub payload_sha256: &'static str,
    pub edits: &'static [FloatEdit],
}

pub struct PakUiFix {
    pub source: &'static str,
    pub mod_name: &'static str,
    pub design: (f64, f64),
    pub packages: &'static [PackageEdit],
}

pub fn paths(paks: &Path, fix: &PakUiFix) -> (PathBuf, PathBuf) {
    let base = paks.join("Mods").join(fix.mod_name);
    (base.with_extension("pak"), base.with_extension("json"))
}

pub fn build_mod(paks: &Path, fix: &PakUiFix, width: u32, height: u32, r: &mut dyn Report) -> Result<(Vec<u8>, String)> {
    if width == 0 || height == 0 {
        return Err("display dimensions must be positive".into());
    }
    let (w, h) = design_space(width, height, fix.design);
    let mut source = pak::Pak::open(&paks.join(fix.source))?;
    if source.mount != "../../../" {
        return Err("unexpected game pak mount point".into());
    }
    let mut files = BTreeMap::new();
    for package in fix.packages {
        let header_name = format!("{}.uasset", package.path);
        let payload_name = format!("{}.uexp", package.path);
        let header = source.read(&header_name)?;
        let mut payload = source.read(&payload_name)?;
        if to_hex(&hash::sha256(&header)) != package.header_sha256 || to_hex(&hash::sha256(&payload)) != package.payload_sha256 {
            return Err(format!(
                "{}: this UI package has changed; a new version of the fix is needed. The game's own files are untouched.",
                package.path
            )
            .into());
        }
        for edit in package.edits {
            let value = (edit.old as f64 + (w - fix.design.0) * edit.grow.0 + (h - fix.design.1) * edit.grow.1) as f32;
            for &at in edit.offsets {
                let bytes = payload.get_mut(at..at + 4).ok_or("UI edit exceeds the asset")?;
                if bytes != edit.old.to_le_bytes() {
                    return Err(format!("{}: unexpected {} value", package.path, edit.label).into());
                }
                bytes.copy_from_slice(&value.to_le_bytes());
            }
            r.line(&format!("{}: {} {} -> {} ({} copies)", package.path, edit.label, edit.old, value, edit.offsets.len()));
        }
        files.insert(header_name, header);
        files.insert(payload_name, payload);
    }
    Ok((pak::build("../../../", &files), source.fingerprint))
}

pub fn check_ui(paks: Option<&Path>, fix: &PakUiFix) -> (UiStatus, String) {
    let Some(paks) = paks else { return (UiStatus::None, "game data folder not found".into()) };
    let (mod_pak, record) = paths(paks, fix);
    if !mod_pak.exists() && !record.exists() {
        return (UiStatus::None, "not installed".into());
    }
    let check = || -> std::result::Result<String, String> {
        let text = std::fs::read_to_string(&record).map_err(|e| e.to_string())?;
        let v = json::parse(&text).map_err(|e| e.to_string())?;
        if v.get("version").and_then(|v| v.as_u64()) != Some(1) {
            return Err("unknown UI record version".into());
        }
        let source = pak::Pak::open(&paks.join(fix.source))?;
        if v.get("source_sha256").and_then(|v| v.as_str()) != Some(&source.fingerprint) {
            return Err("game data changed since installation".into());
        }
        let sha = sha256_file(&mod_pak, None).map_err(|e| e.to_string())?;
        if v.get("pak_sha256").and_then(|v| v.as_str()) != Some(&sha) {
            return Err("UI mod is missing or has changed".into());
        }
        Ok("full-width UI pak installed and current".into())
    };
    match check() {
        Ok(s) => (UiStatus::Current, s),
        Err(s) => (UiStatus::Stale, format!("{s} - install again")),
    }
}

pub fn install_ui(paks: &Path, fix: &PakUiFix, width: u32, height: u32, r: &mut dyn Report) -> Result<()> {
    let (bytes, fingerprint) = build_mod(paks, fix, width, height, r)?;
    let record = format!(
        "{{\"version\":1,\"source_sha256\":{},\"pak_sha256\":{},\"display\":[{},{}]}}\n",
        json::quote(&fingerprint),
        json::quote(&to_hex(&hash::sha256(&bytes))),
        width,
        height
    );
    let (mod_pak, sidecar) = paths(paks, fix);
    std::fs::create_dir_all(mod_pak.parent().unwrap()).map_err(|e| InstallError(write_failure(&mod_pak, &e)))?;
    replace_file(&mod_pak, &bytes)?;
    replace_file(&sidecar, record.as_bytes())?;
    r.line(&format!("installed {} packages in {}", fix.packages.len(), mod_pak.display()));
    Ok(())
}

pub fn restore_ui(paks: &Path, fix: &PakUiFix, r: &mut dyn Report) -> Result<()> {
    let (mod_pak, sidecar) = paths(paks, fix);
    for path in [mod_pak, sidecar] {
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| InstallError(write_failure(&path, &e)))?;
            r.line(&format!("removed {}", path.display()));
        }
    }
    Ok(())
}
