//! The full-width UI fix: widen the game's 16:9 UI layout to the real UMG
//! design space, and ship the result as a mod container.
//!
//! Root cause (RESEARCH.md 9c): `BP_UIWindowManager`'s `WindowParent`, the
//! panel every game window is reparented into, is a fixed 3840x2160 box
//! centred in the viewport, so all UI is clipped to 16:9 on an ultrawide
//! display. The first edit widens it; the rest repair the handful of
//! elements the audit (9c-2) found positioned by absolute coordinates on
//! the 3840 canvas, which would otherwise shift left.
//!
//! Every edit rewrites an *existing* float in place, so package sizes never
//! change. The edited packages are published as their own small IoStore
//! container in `Content/Paks/Mods/`, which the engine mounts after
//! `pakchunk0` and which therefore shadows the copies in it. `pakchunk0` is
//! only ever read, so Steam's Verify Integrity has nothing to repair, a game
//! update cannot half-overwrite the fix, and restoring is three file
//! deletions.
//!
//! The record next to the container (`<mod>.json`) is an on-disk contract
//! with existing installs: it says which build of the game the container
//! was made from, and the Python version of the fix wrote the same fields.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::hash;
use crate::iostore::{
    self, CHUNK_CONTAINER_HEADER, Chunk, ReadError, StoreEntry, Toc, build_container, container_header_chunk_id,
    container_id_for, load_script_objects, package_id_of, parse_container_header, stub_pak,
};
use crate::json::{self, Value};
use crate::report::{InstallError, Report, Result, replace_file, write_failure};
use crate::unver::{Slot, decode_slot};
use crate::zen::{ScriptObjects, Summary, ZenPackage};

pub const MOD_DIR: &str = "Mods";
pub const RECORD_VERSION: u64 = 1;
/// Bytes of the source `.ucas` hashed as its fingerprint.
const HEAD: u64 = 1 << 20;

/// Which `Offsets` field of a `UCanvasPanelSlot` an edit changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Left = 0,
    Top = 1,
    Right = 2,
    Bottom = 3,
}

impl Field {
    pub fn name(self) -> &'static str {
        match self {
            Field::Left => "Left",
            Field::Top => "Top",
            Field::Right => "Right",
            Field::Bottom => "Bottom",
        }
    }
}

/// The new value of a slot field, as a function of the design space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NewValue {
    /// The design width.
    Width,
    /// Half the design width.
    HalfWidth,
    /// Left-anchored: keep the element where the 16:9 box used to put it.
    Inset(f64),
    /// Right-anchored: the same, mirrored.
    Outset(f64),
}

impl NewValue {
    pub fn apply(self, design_w: f64, authored: (f64, f64)) -> f64 {
        match self {
            NewValue::Width => design_w,
            NewValue::HalfWidth => design_w / 2.0,
            NewValue::Inset(v) => v + (design_w - authored.0) / 2.0,
            NewValue::Outset(v) => v - (design_w - authored.0) / 2.0,
        }
    }
}

/// One float rewritten in place in one package.
#[derive(Debug)]
pub struct Edit {
    /// Below [`UiFix::ui_prefix`].
    pub package: &'static str,
    /// The widget the slot holds (its `Content`).
    pub widget: &'static str,
    pub field: Field,
    pub old: f32,
    pub new: NewValue,
}

/// A game's UI fix: the source container, the edits, and the container
/// format versions to write.
pub struct UiFix {
    /// `"pakchunk0-Windows"`: the container the packages come from.
    pub source: &'static str,
    /// `"Chronos/Content/"`: what the directory index paths start with.
    pub content_prefix: &'static str,
    /// `"Chronos/Content/UI/"`: what [`Edit::package`] is relative to.
    pub ui_prefix: &'static str,
    /// The mod container's mount point.
    pub mount_point: &'static str,
    /// `_P` is the engine's own marker for a patch pak: it mounts after the
    /// shipped containers, so every package in it wins over the stock copy.
    pub mod_name: &'static str,
    /// The UMG design size the UI was authored for.
    pub design: (f64, f64),
    pub edits: &'static [Edit],
    /// The formats the game's own containers use, which the mod copies.
    pub toc_version: u8,
    pub container_header_version: u32,
    pub summary: Summary,
}

/// UE `EUIScalingRule::ScaleToFit` -> the UMG design space in slate units.
pub fn design_space(width: u32, height: u32, authored: (f64, f64)) -> (f64, f64) {
    let scale = (width as f64 / authored.0).min(height as f64 / authored.1);
    (width as f64 / scale, height as f64 / scale)
}

/// Python's `%g`, near enough: integers plainly, otherwise trimmed decimals.
fn g(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub struct ModPaths {
    pub utoc: PathBuf,
    pub ucas: PathBuf,
    pub pak: PathBuf,
    pub record: PathBuf,
}

impl ModPaths {
    pub fn container(&self) -> [&Path; 3] {
        [&self.utoc, &self.ucas, &self.pak]
    }
}

pub fn mod_paths(paks: &Path, ui: &UiFix) -> ModPaths {
    let base = paks.join(MOD_DIR).join(ui.mod_name);
    ModPaths {
        utoc: base.with_extension("utoc"),
        ucas: base.with_extension("ucas"),
        pak: base.with_extension("pak"),
        record: paks.join(MOD_DIR).join(format!("{}.json", ui.mod_name)),
    }
}

/// -> how many of the mod's files were there to remove.
pub fn remove_mod(paks: &Path, ui: &UiFix) -> Result<usize> {
    let mp = mod_paths(paks, ui);
    let mut gone = 0;
    for path in [&mp.utoc, &mp.ucas, &mp.pak, &mp.record] {
        if path.is_file() {
            std::fs::remove_file(path).map_err(|e| InstallError(write_failure(path, &e)))?;
            gone += 1;
        }
    }
    Ok(gone)
}

pub fn sha256_file(path: &Path, limit: Option<u64>) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = hash::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut left = limit;
    loop {
        let want = match left {
            Some(0) => break,
            Some(n) => (n as usize).min(buf.len()),
            None => buf.len(),
        };
        let n = f.read(&mut buf[..want])?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        if let Some(l) = left.as_mut() {
            *l -= n as u64;
        }
    }
    Ok(crate::to_hex(&h.finish()))
}

/// Which build of the game the mod container was generated from.
#[derive(Debug, Clone, PartialEq)]
pub struct Fingerprint {
    pub ucas_size: u64,
    pub ucas_head: String,
}

pub fn source_fingerprint(ucas: &Path) -> std::io::Result<Fingerprint> {
    Ok(Fingerprint { ucas_size: std::fs::metadata(ucas)?.len(), ucas_head: sha256_file(ucas, Some(HEAD))? })
}

/// `<mod>.json`: what the container was built from and for.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Record {
    pub ucas_size: Option<u64>,
    pub ucas_head: Option<String>,
    pub utoc_sha256: Option<String>,
    pub version: Option<u64>,
    pub display: Option<(u32, u32)>,
}

pub fn parse_record(text: &str) -> Record {
    let Ok(v) = json::parse(text) else { return Record::default() };
    let display = v.get("display").and_then(Value::as_array).and_then(|a| {
        let w = a.first()?.as_u64()? as u32;
        let h = a.get(1)?.as_u64()? as u32;
        Some((w, h))
    });
    Record {
        ucas_size: v.get("ucas_size").and_then(Value::as_u64),
        ucas_head: v.get("ucas_head").and_then(Value::as_str).map(str::to_string),
        utoc_sha256: v.get("utoc_sha256").and_then(Value::as_str).map(str::to_string),
        version: v.get("version").and_then(Value::as_u64),
        display,
    }
}

pub fn read_record(path: &Path) -> Record {
    std::fs::read_to_string(path).map(|t| parse_record(&t)).unwrap_or_default()
}

/// The record as the Python version wrote it, field for field.
pub fn record_text(fp: &Fingerprint, display: (u32, u32)) -> String {
    format!(
        "{{\"ucas_size\": {}, \"ucas_head\": {}, \"version\": {}, \"display\": [{}, {}]}}",
        fp.ucas_size,
        json::quote(&fp.ucas_head),
        RECORD_VERSION,
        display.0,
        display.1
    )
}

// ---------------------------------------------------------------------------
// Removing the in-place patch older versions of this fix applied to pakchunk0
// ---------------------------------------------------------------------------

/// Put a container an older release edited in place back to stock.
///
/// Up to and including the release before the mod container, the fix
/// appended the edited packages to `pakchunk0-Windows.ucas` and repointed
/// its TOC. Anyone upgrading still has that on disk, and the mod container
/// would sit on top of it, so it is undone first - which also means the
/// packages read later come from a stock container.
pub fn undo_in_place_patch(paks: &Path, ui: &UiFix) -> Result<Option<String>> {
    let utoc = paks.join(format!("{}.utoc", ui.source));
    let backup = paks.join(format!("{}.utoc.original", ui.source));
    let sidecar = paks.join(format!("{}.uipatch.json", ui.source));
    if !backup.exists() {
        return Ok(None);
    }
    let ucas = paks.join(format!("{}.ucas", ui.source));
    let record = read_record(&sidecar);
    let io = |e: std::io::Error| InstallError(format!("could not read the game data ({e})"));

    // an empty or zero value in the sidecar counts as absent, as it did for
    // the Python that wrote it
    let mut stale = false;
    if let Some(want) = record.utoc_sha256.as_deref().filter(|s| !s.is_empty()) {
        stale |= sha256_file(&backup, None).map_err(io)? != want;
    }
    if let Some(want) = record.ucas_head.as_deref().filter(|s| !s.is_empty()) {
        stale |= sha256_file(&ucas, Some(HEAD)).map_err(io)? != want;
    }
    if stale {
        // The game was updated over the top of it: that backup belongs to a
        // build that is no longer installed and writing it back would leave an
        // unbootable container. The update already replaced everything we wrote.
        for path in [&backup, &sidecar] {
            if path.exists() {
                std::fs::remove_file(path).map_err(|e| InstallError(write_failure(path, &e)))?;
            }
        }
        return Ok(Some(
            "removed a backup left by an older version of this fix - it was taken from a different build of the \
             game and is of no use now"
                .into(),
        ));
    }

    std::fs::copy(&backup, &utoc).map_err(|e| InstallError(write_failure(&utoc, &e)))?;
    if let Some(original) = record.ucas_size.filter(|&n| n > 0)
        && std::fs::metadata(&ucas).map_err(io)?.len() > original {
            let f = std::fs::OpenOptions::new().write(true).open(&ucas).map_err(|e| InstallError(write_failure(&ucas, &e)))?;
            f.set_len(original).map_err(|e| InstallError(write_failure(&ucas, &e)))?;
        }
    for path in [&backup, &sidecar] {
        if path.exists() {
            std::fs::remove_file(path).map_err(|e| InstallError(write_failure(path, &e)))?;
        }
    }
    Ok(Some("undid the in-place patch an older version of this fix applied".into()))
}

// ---------------------------------------------------------------------------
// The mod container
// ---------------------------------------------------------------------------

fn decode_failed(what: &str, err: &dyn std::fmt::Display) -> InstallError {
    InstallError(format!(
        "cannot decode {what} ({err}). The game data is read with the fix's own Kraken decoder; a game update may \
         have changed how it is compressed. Please report this line in an issue. The full-width UI is not \
         installed; the game's own files are untouched."
    ))
}

/// `toc.read`, with a decode failure turned into a one-line error naming `what`.
fn read_chunk(toc: &mut Toc, index: usize, what: &str) -> Result<Vec<u8>> {
    toc.read(index).map_err(|e| match e {
        ReadError::Decode(k) => decode_failed(what, &k),
        other => InstallError(format!("could not read {what} ({other})")),
    })
}

fn open_toc(path: &Path) -> Result<Toc> {
    Toc::open(path).map_err(|e| InstallError(format!("could not read {} ({e})", path.display())))
}

/// The `UCanvasPanelSlot` whose `Content` is `widget`: the export, the slot,
/// the payload's offset in the chunk, and the payload itself.
pub fn slot_payload<'a>(pkg: &ZenPackage<'a>, widget: &str, so: &ScriptObjects) -> Option<(usize, Slot, usize, &'a [u8])> {
    for e in &pkg.exports {
        let Some(class) = pkg.script_class(e.class, so) else { continue };
        if class.strip_prefix("/Script/").unwrap_or(class) != "UMG.CanvasPanelSlot" {
            continue;
        }
        let payload = pkg.export_data(e)?;
        let Ok(s) = decode_slot(payload) else { continue };
        let content = s.content_export().and_then(|i| pkg.exports.get(i));
        if content.is_some_and(|c| c.name == widget) {
            return Some((e.index, s, pkg.export_offset(e.index)?, payload));
        }
    }
    None
}

fn find_unique_float(payload: &[u8], value: f32) -> std::result::Result<usize, String> {
    let hits: Vec<usize> = (0..payload.len().saturating_sub(3))
        .filter(|&o| f32::from_le_bytes(payload[o..o + 4].try_into().unwrap()) == value)
        .collect();
    if hits.len() != 1 {
        return Err(format!("{} occurrences of {} in the slot payload (need exactly 1)", hits.len(), g(value as f64)));
    }
    Ok(hits[0])
}

/// The edits grouped by package, in order of first appearance.
fn by_package(ui: &UiFix) -> Vec<(String, Vec<&'static Edit>)> {
    let mut out: Vec<(String, Vec<&'static Edit>)> = Vec::new();
    for e in ui.edits {
        let path = format!("{}{}", ui.ui_prefix, e.package);
        match out.iter_mut().find(|(p, _)| *p == path) {
            Some((_, list)) => list.push(e),
            None => out.push((path, vec![e])),
        }
    }
    out
}

fn short_name(ui: &UiFix, pkg_path: &str) -> String {
    pkg_path.trim_start_matches(ui.ui_prefix).trim_end_matches(".uasset").to_string()
}

/// A mod container, built but not yet written.
pub struct Built {
    pub utoc: Vec<u8>,
    pub ucas: Vec<u8>,
    pub pak: Vec<u8>,
    /// packages published
    pub applied: usize,
    /// packages skipped
    pub failed: usize,
}

/// Edit the packages in memory and build the mod container that carries them.
pub fn build_mod(paks: &Path, ui: &UiFix, design_w: f64, so: &ScriptObjects, r: &mut dyn Report) -> Result<Built> {
    let mut toc = open_toc(&paks.join(format!("{}.utoc", ui.source)))?;
    let header_index = toc
        .find_type(CHUNK_CONTAINER_HEADER)
        .ok_or_else(|| InstallError(format!("{} has no container header - it cannot be read.", ui.source)))?;
    let header = read_chunk(&mut toc, header_index, &format!("{} container header", ui.source))?;
    let (_, stock_entries) = parse_container_header(&header, ui.container_header_version)
        .map_err(|e| InstallError(format!("{} container header: {e}", ui.source)))?;

    let container_id = container_id_for(ui.mod_name);
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut entries: BTreeMap<u64, StoreEntry> = BTreeMap::new();
    let (mut applied, mut failed) = (0, 0);
    for (pkg_path, edits) in by_package(ui) {
        let name = short_name(ui, &pkg_path);
        let Some(&idx) = toc.index.get(&pkg_path) else {
            r.line(&format!("  SKIP {:<34} not in {}", name, ui.source));
            failed += 1;
            continue;
        };
        let Some(&chunk_id) = toc.chunk_ids.get(idx) else {
            r.line(&format!("  SKIP {:<34} directory index points past the chunk table", name));
            failed += 1;
            continue;
        };
        let package_id = package_id_of(&chunk_id);
        let Some(entry) = stock_entries.get(&package_id) else {
            r.line(&format!("  SKIP {:<34} no package store entry", name));
            failed += 1;
            continue;
        };

        let data = read_chunk(&mut toc, idx, &name)?;
        let pkg = ZenPackage::parse(&data, ui.summary).map_err(|e| InstallError(format!("cannot parse {name} ({e})")))?;
        let mut buf = data.clone();
        let mut notes = Vec::new();
        let mut done = 0;
        for edit in edits {
            let Some((_, slot, base, payload)) = slot_payload(&pkg, edit.widget, so) else {
                notes.push(format!("  !! {}: slot not found", edit.widget));
                continue;
            };
            let current = slot.offsets[edit.field as usize];
            if current != edit.old {
                notes.push(format!(
                    "  !! {}.{} is {}, expected {} - skipped",
                    edit.widget,
                    edit.field.name(),
                    g(current as f64),
                    g(edit.old as f64)
                ));
                continue;
            }
            let off = match find_unique_float(payload, edit.old) {
                Ok(o) => o,
                Err(e) => {
                    notes.push(format!("  !! {}: {e}", edit.widget));
                    continue;
                }
            };
            let val = edit.new.apply(design_w, ui.design);
            buf[base + off..base + off + 4].copy_from_slice(&(val as f32).to_le_bytes());
            notes.push(format!("  {:<26} {:<6} {} -> {}", edit.widget, edit.field.name(), g(edit.old as f64), g(val)));
            done += 1;
        }

        r.line(&name);
        for n in &notes {
            r.line(n);
        }
        if done == 0 {
            failed += 1;
            continue;
        }
        chunks.push(Chunk { id: chunk_id, data: buf, path: Some(pkg_path[ui.content_prefix.len()..].to_string()) });
        entries.insert(package_id, entry.clone());
        applied += 1;
    }

    if chunks.is_empty() {
        return Err(InstallError("none of the UI packages could be read - nothing to install.".into()));
    }
    chunks.push(Chunk {
        id: container_header_chunk_id(container_id),
        data: iostore::build_container_header(container_id, &entries, ui.container_header_version),
        path: None,
    });

    let (utoc, ucas) = build_container(ui.mount_point, &chunks, container_id, ui.toc_version).map_err(InstallError)?;
    Ok(Built { utoc, ucas, pak: stub_pak(), applied, failed })
}

/// Write a built container next to the game data.
fn write_mod(paks: &Path, ui: &UiFix, built: &Built, r: &mut dyn Report) -> Result<()> {
    let mods = paks.join(MOD_DIR);
    std::fs::create_dir_all(&mods).map_err(|e| InstallError(write_failure(&mods, &e)))?;
    let mp = mod_paths(paks, ui);
    std::fs::write(&mp.ucas, &built.ucas).map_err(|e| InstallError(write_failure(&mp.ucas, &e)))?;
    std::fs::write(&mp.utoc, &built.utoc).map_err(|e| InstallError(write_failure(&mp.utoc, &e)))?;
    std::fs::write(&mp.pak, &built.pak).map_err(|e| InstallError(write_failure(&mp.pak, &e)))?;
    r.line("");
    r.line(&format!(
        "wrote {}/{}.utoc + .ucas + .pak ({:.0} KB)",
        MOD_DIR,
        ui.mod_name,
        (built.ucas.len() + built.utoc.len()) as f64 / 1024.0
    ));
    Ok(())
}

/// Read every edit back out of the container the game will actually mount.
/// -> how many came back wrong.
fn verify_mod(paks: &Path, ui: &UiFix, design_w: f64, so: &ScriptObjects, r: &mut dyn Report) -> Result<usize> {
    r.line("");
    r.line("verifying through the container reader...");
    let mut toc = open_toc(&mod_paths(paks, ui).utoc)?;
    let mut bad = 0;
    for (pkg_path, edits) in by_package(ui) {
        let Some(&idx) = toc.index.get(&pkg_path) else { continue };
        let data = read_chunk(&mut toc, idx, &format!("{}: {pkg_path}", ui.mod_name))?;
        let pkg = ZenPackage::parse(&data, ui.summary).map_err(|e| InstallError(format!("cannot parse {pkg_path} ({e})")))?;
        for edit in edits {
            let want = edit.new.apply(design_w, ui.design);
            let got = slot_payload(&pkg, edit.widget, so).map(|(_, s, _, _)| s.offsets[edit.field as usize] as f64);
            if got.is_none_or(|got| (got - want).abs() > 0.5) {
                r.line(&format!(
                    "  MISMATCH {}.{} = {} (want {})",
                    edit.widget,
                    edit.field.name(),
                    got.map(g).unwrap_or_else(|| "None".into()),
                    g(want)
                ));
                bad += 1;
            }
        }
    }
    Ok(bad)
}

/// What is installed, and is it still for the game that is installed?
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiStatus {
    /// the container matches the build of the game on disk
    Current,
    /// the game was updated after the container was built, so it now
    /// shadows ten packages with copies cooked for an older build
    Stale,
    /// the files are there but nothing says what they were built from -
    /// an older release of this fix, or a hand copy
    Unrecorded,
    /// some of the three files are missing; the game may load a container
    /// it cannot resolve
    Incomplete,
    /// not installed
    None,
    /// no Paks folder next to the executable
    NoPaks,
    /// not checked
    Error,
}

impl UiStatus {
    /// The word the Windows front-end reads on the `files:` line.
    pub fn as_str(self) -> &'static str {
        match self {
            UiStatus::Current => "current",
            UiStatus::Stale => "stale",
            UiStatus::Unrecorded => "unrecorded",
            UiStatus::Incomplete => "incomplete",
            UiStatus::None => "none",
            UiStatus::NoPaks => "nopaks",
            UiStatus::Error => "error",
        }
    }
}

pub fn container_state(paks: &Path, ui: &UiFix) -> (UiStatus, String) {
    let mp = mod_paths(paks, ui);
    let present = mp.container().iter().filter(|p| p.exists()).count();
    if present == 0 {
        return (UiStatus::None, "not installed".into());
    }
    if present < 3 {
        return (UiStatus::Incomplete, format!("incomplete - {present} of the 3 files are there; re-run the installer"));
    }
    let record = read_record(&mp.record);
    let display = record.display.map(|(w, h)| format!("{w}x{h}"));
    let Some(head) = record.ucas_head.filter(|h| !h.is_empty()) else {
        return (
            UiStatus::Unrecorded,
            "installed, but nothing records which build it was made from - re-run the installer".into(),
        );
    };
    let ucas = paks.join(format!("{}.ucas", ui.source));
    let size = std::fs::metadata(&ucas).map(|m| m.len()).ok();
    if record.ucas_size != size || sha256_file(&ucas, Some(HEAD)).ok().as_deref() != Some(head.as_str()) {
        return (
            UiStatus::Stale,
            "built for a different build of the game - the game has been updated since, and the installer must be \
             run again"
                .into(),
        );
    }
    (UiStatus::Current, format!("installed for {}", display.unwrap_or_else(|| "an unrecorded display".into())))
}

/// The status line of the UI part, changing nothing: what `status` prints
/// as `files:` / `filesdetail:`.
pub fn check_ui(paks: Option<&Path>, ui: &UiFix) -> (UiStatus, String) {
    let Some(paks) = paks.filter(|p| p.is_dir()) else {
        return (UiStatus::NoPaks, "not checked - no Paks folder next to that executable".into());
    };
    for f in [format!("{}.utoc", ui.source), format!("{}.ucas", ui.source)] {
        if !paks.join(&f).exists() {
            return (UiStatus::Error, format!("not checked - {f} is missing from {}", paks.display()));
        }
    }
    let (status, mut detail) = container_state(paks, ui);
    if paks.join(format!("{}.utoc.original", ui.source)).exists() {
        detail.push_str(&format!(
            "; {} still carries the in-place patch of an older version, which the next install removes",
            ui.source
        ));
    }
    (status, detail)
}

fn check_source(paks: &Path, ui: &UiFix) -> Result<()> {
    if !paks.is_dir() {
        return Err(InstallError(format!("the game's data folder is not where it was expected ({}).", paks.display())));
    }
    for f in [format!("{}.utoc", ui.source), format!("{}.ucas", ui.source)] {
        if !paks.join(&f).exists() {
            return Err(InstallError(format!("{f} is missing from {}.", paks.display())));
        }
    }
    Ok(())
}

/// Build and install the mod container for a display.
pub fn install_ui(paks: &Path, ui: &UiFix, width: u32, height: u32, r: &mut dyn Report) -> Result<()> {
    check_source(paks, ui)?;
    if let Some(note) = undo_in_place_patch(paks, ui)? {
        r.line(&note);
        r.line("");
    }
    let (dw, dh) = design_space(width, height, ui.design);
    r.line(&format!("{width}x{height} -> UMG design space {dw:.0}x{dh:.0}"));
    r.line("");
    if (dw - ui.design.0).abs() < 1.0 && (dh - ui.design.1).abs() < 1.0 {
        remove_mod(paks, ui)?;
        r.line("already 16:9 - nothing to do.");
        return Ok(());
    }

    remove_mod(paks, ui)?; // never build on top of an older one
    let so = load_script_objects(&paks.join("global.utoc")).map_err(|e| match e {
        ReadError::Decode(k) => decode_failed("global.utoc", &k),
        other => InstallError(format!("could not read global.utoc ({other})")),
    })?;
    let built = build_mod(paks, ui, dw, &so, r)?;
    write_mod(paks, ui, &built, r)?;
    let bad = verify_mod(paks, ui, dw, &so, r)?;

    r.line(&format!("{} packages published, {} skipped, {bad} mismatches", built.applied, built.failed));
    if bad > 0 {
        remove_mod(paks, ui)?;
        return Err(InstallError("the container did not read back correctly and was removed; the game is untouched.".into()));
    }
    let fp = source_fingerprint(&paks.join(format!("{}.ucas", ui.source)))
        .map_err(|e| InstallError(format!("could not fingerprint the game data ({e})")))?;
    let mp = mod_paths(paks, ui);
    replace_file(&mp.record, record_text(&fp, (width, height)).as_bytes())?;
    r.line("OK - safe to launch.");
    Ok(())
}

/// Remove the mod container, and the in-place patch of older versions.
pub fn restore_ui(paks: &Path, ui: &UiFix, r: &mut dyn Report) -> Result<()> {
    if !paks.is_dir() {
        r.line("nothing to restore - the full-width UI was never installed.");
        return Ok(());
    }
    let note = undo_in_place_patch(paks, ui)?;
    if let Some(n) = &note {
        r.line(n);
        r.line("");
    }
    let gone = remove_mod(paks, ui)?;
    r.line(if gone > 0 || note.is_some() {
        "stock state restored."
    } else {
        "nothing to restore - the full-width UI was never installed."
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn design_space_is_scale_to_fit() {
        let d = (3840.0, 2160.0);
        assert_eq!(design_space(5120, 2160, d), (5120.0, 2160.0));
        assert_eq!(design_space(3440, 1440, d), (5160.0, 2160.0));
        assert_eq!(design_space(2560, 1600, d), (3840.0, 2400.0));
        assert_eq!(design_space(1920, 1080, d), (3840.0, 2160.0));
        assert_eq!(NewValue::Inset(220.0).apply(5120.0, d), 860.0);
        assert_eq!(NewValue::Outset(-220.0).apply(5120.0, d), -860.0);
        assert_eq!(NewValue::HalfWidth.apply(5160.0, d), 2580.0);
        assert_eq!(g(3840.0), "3840");
        assert_eq!(g(-220.0), "-220");
        assert_eq!(g(892.5), "892.5");
    }

    #[test]
    fn the_record_reads_and_writes_the_python_format() {
        let text = r#"{"ucas_size": 18067210736, "ucas_head": "c85e30f3", "version": 1, "display": [5120, 2160]}"#;
        let r = parse_record(text);
        assert_eq!(r.ucas_size, Some(18067210736));
        assert_eq!(r.ucas_head.as_deref(), Some("c85e30f3"));
        assert_eq!(r.display, Some((5120, 2160)));
        assert_eq!(record_text(&Fingerprint { ucas_size: 18067210736, ucas_head: "c85e30f3".into() }, (5120, 2160)), text);
        assert_eq!(parse_record("not json"), Record::default());
    }

    #[test]
    fn unique_float_search() {
        let mut payload = vec![0u8; 16];
        payload[4..8].copy_from_slice(&3840f32.to_le_bytes());
        assert_eq!(find_unique_float(&payload, 3840.0), Ok(4));
        assert!(find_unique_float(&payload, 1920.0).unwrap_err().contains("0 occurrences"));
        payload[8..12].copy_from_slice(&3840f32.to_le_bytes());
        assert!(find_unique_float(&payload, 3840.0).unwrap_err().contains("2 occurrences"));
    }
}
