//! Where the game is (RESEARCH.md 8a): next to the installer, every Steam
//! library, the Epic Games Launcher's manifests, then the usual game roots
//! on every fixed drive. Nothing is scanned recursively.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::games::{Game, flatten};
use crate::json;
use crate::steam;

/// One of the places the search looks.
type Finder = fn(&dyn Game, Option<&Path>) -> Vec<Found>;

/// The game executable and how it was found.
#[derive(Debug, Clone)]
pub struct Found {
    pub exe: PathBuf,
    pub source: String,
}

/// First hit of the layered search; None if the game is nowhere to be found.
/// `self_dir` is the installer's own folder.
pub fn find_exe(game: &dyn Game, self_dir: Option<&Path>) -> Option<Found> {
    candidates(game, self_dir).into_iter().next()
}

/// Every place the game was found, most trustworthy first.
pub fn candidates(game: &dyn Game, self_dir: Option<&Path>) -> Vec<Found> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let finders: [Finder; 4] = [local_candidates, steam_candidates, epic_candidates, generic_candidates];
    for finder in finders {
        for f in finder(game, self_dir) {
            if seen.insert(steam::path_key(&f.exe)) {
                out.push(f);
            }
        }
    }
    out
}

/// `<game_root>/<project>/Binaries/Win64/<exe>`, if that file exists.
pub fn exe_under(game: &dyn Game, game_root: &Path) -> Option<PathBuf> {
    let p = game_root.join(game.exe_relative());
    p.is_file().then(|| std::path::absolute(&p).unwrap_or(p))
}

fn looks_like_game(game: &dyn Game, name: &str) -> bool {
    flatten(name).contains(game.folder_hint())
}

/// Next to the installer, next to the shell's cwd, or one level up.
fn local_candidates(game: &dyn Game, self_dir: Option<&Path>) -> Vec<Found> {
    let mut out = Vec::new();
    let rel = game.exe_relative();
    let relatives = [PathBuf::from(game.exe_name()), rel.clone(), Path::new("..").join(&rel), Path::new("..").join("..").join(&rel)];
    let mut bases = Vec::new();
    if let Some(d) = self_dir {
        bases.push((d.to_path_buf(), "the installer's own folder"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        bases.push((cwd, "the current folder"));
    }
    for (base, label) in bases {
        for r in &relatives {
            let p = base.join(r);
            if p.is_file() {
                out.push(Found { exe: std::path::absolute(&p).unwrap_or(p), source: label.to_string() });
            }
        }
    }
    out
}

fn steam_candidates(game: &dyn Game, _: Option<&Path>) -> Vec<Found> {
    let mut out = Vec::new();
    for library in steam::libraries() {
        let apps = steam::child(&library, "steamapps");
        let common = steam::child(&apps, "common");
        if !common.is_dir() {
            continue;
        }
        let mut names = Vec::new();
        if let Some(n) = steam::install_dir_from_manifest(&apps, game.steam_appid()) {
            names.push(n);
        }
        names.push(game.install_dir().to_string());
        if let Ok(entries) = std::fs::read_dir(&common) {
            for e in entries.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if looks_like_game(game, &n) {
                    names.push(n);
                }
            }
        }
        for name in names {
            if let Some(exe) = exe_under(game, &common.join(name)) {
                out.push(Found { exe, source: format!("Steam library {}", library.display()) });
            }
        }
    }
    out
}

fn epic_candidates(game: &dyn Game, _: Option<&Path>) -> Vec<Found> {
    let mut out = Vec::new();
    let program_data = std::env::var_os("PROGRAMDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\ProgramData"));
    let manifests = program_data.join("Epic").join("EpicGamesLauncher").join("Data").join("Manifests");
    let Ok(entries) = std::fs::read_dir(&manifests) else { return out };
    for e in entries.flatten() {
        if !e.file_name().to_string_lossy().to_lowercase().ends_with(".item") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
        let Ok(info) = json::parse(&text) else { continue };
        let Some(location) = info.get("InstallLocation").and_then(|v| v.as_str()) else { continue };
        if let Some(exe) = exe_under(game, Path::new(location)) {
            out.push(Found { exe, source: "the Epic Games Launcher".into() });
        }
    }
    out
}

/// The usual places a game folder ends up when no launcher claims it.
fn generic_candidates(game: &dyn Game, _: Option<&Path>) -> Vec<Found> {
    let mut out = Vec::new();
    for drive in steam::fixed_drives() {
        let mut roots = vec![drive.clone()];
        for rel in [
            "Games",
            "Program Files",
            "Program Files (x86)",
            "GOG Games",
            "Epic Games",
            "SteamLibrary/steamapps/common",
            "Games/steamapps/common",
        ] {
            roots.push(drive.join(rel));
        }
        for root in roots {
            if let Some(exe) = exe_under(game, &root.join(game.install_dir())) {
                out.push(Found { exe, source: root.display().to_string() });
            }
            let Ok(entries) = std::fs::read_dir(&root) else { continue };
            for e in entries.flatten() {
                if !looks_like_game(game, &e.file_name().to_string_lossy()) {
                    continue;
                }
                if let Some(exe) = exe_under(game, &e.path()) {
                    out.push(Found { exe, source: root.display().to_string() });
                }
            }
        }
    }
    out
}

/// Which game an executable path belongs to, by its file name.
pub fn game_of(exe: &Path) -> Option<&'static dyn Game> {
    let name = exe.file_name()?.to_string_lossy();
    crate::games::game_for_exe(&name)
}
