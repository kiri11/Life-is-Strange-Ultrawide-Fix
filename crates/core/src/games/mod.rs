//! The games the fix knows, as data plus a small trait.
//!
//! Adding a game is one module here and one entry in [`GAMES`]: the loader
//! picks the game by the executable's name, the installer by the Steam app
//! id and the executable it finds. Everything path-shaped derives from
//! [`Game::project`] and [`Game::install_dir`]:
//! `<game>/<project>/Binaries/Win64/<exe>`, `<game>/<project>/Content/Paks`,
//! `%LOCALAPPDATA%\<project>\Saved\Config\Windows\Engine.ini`, and under
//! Proton `compatdata/<appid>/pfx/drive_c/users/steamuser/AppData/Local/<project>/...`.

pub mod double_exposure;
pub mod reunion;

use std::path::{Path, PathBuf};

use crate::plan::Plan;
use crate::scan::Image;
use crate::ui_layout::UiFix;

pub trait Game: Sync {
    /// `"double-exposure"` or `"reunion"`: what `--game` takes.
    fn id(&self) -> &'static str;
    fn title(&self) -> &'static str;
    /// `"Double Exposure"`: the title without the series name, for lists
    /// and headings that name several games at once.
    fn short_title(&self) -> &'static str;
    fn steam_appid(&self) -> u32;
    /// `"Chronos-Win64-Shipping.exe"`: the loader does nothing in any other process.
    fn exe_name(&self) -> &'static str;
    /// `"Chronos"`: `Binaries/`, `Content/` and `Saved/` live under it.
    fn project(&self) -> &'static str;
    /// The folder Steam installs the game into.
    fn install_dir(&self) -> &'static str;
    /// What a folder name contains when it is probably the game's, for the
    /// search of the usual game roots; lower case, letters and digits only.
    fn folder_hint(&self) -> &'static str;
    /// The names the loader can be installed under, in preference order:
    /// one today. A second needs a `forward!` block of its exports in the
    /// loader, and the load-order check that ruled version.dll out.
    fn proxy_dlls(&self) -> &'static [&'static str] {
        &["winhttp.dll"]
    }
    fn plan_camera(&self, image: &Image, gate_upper: [u8; 4]) -> Result<Plan, String>;
    /// None until the game's UI packages are known.
    fn ui(&self) -> Option<&'static UiFix>;
    /// The markers of the managed block in the user's Engine.ini. They are
    /// part of the on-disk contract with existing installs: never change
    /// them for a game that has shipped.
    fn ini_markers(&self) -> (&'static str, &'static str);

    // ---- derived paths --------------------------------------------------

    /// `<project>/Binaries/Win64/<exe>`.
    fn exe_relative(&self) -> PathBuf {
        Path::new(self.project()).join("Binaries").join("Win64").join(self.exe_name())
    }
    /// `<game>/<project>/Content/Paks` from the executable's path.
    fn paks_dir(&self, exe: &Path) -> Option<PathBuf> {
        let win64 = exe.parent()?;
        let project = win64.parent()?.parent()?;
        Some(project.join("Content").join("Paks"))
    }
    /// `<project>/Saved/Config/Windows/Engine.ini` below a Local AppData folder.
    fn engine_ini_relative(&self) -> PathBuf {
        Path::new(self.project()).join("Saved").join("Config").join("Windows").join("Engine.ini")
    }
}

/// Every game the fix knows, in the order the installer offers them.
pub static GAMES: &[&dyn Game] = &[&double_exposure::DOUBLE_EXPOSURE, &reunion::REUNION];

pub fn game_for_exe(exe_name: &str) -> Option<&'static dyn Game> {
    GAMES.iter().copied().find(|g| g.exe_name().eq_ignore_ascii_case(exe_name))
}

pub fn game_for_id(id: &str) -> Option<&'static dyn Game> {
    GAMES.iter().copied().find(|g| g.id().eq_ignore_ascii_case(id))
}

pub fn game_for_appid(appid: u32) -> Option<&'static dyn Game> {
    GAMES.iter().copied().find(|g| g.steam_appid() == appid)
}

/// Letters and digits only, lower case: how folder names are compared.
pub fn flatten(name: &str) -> String {
    name.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}
