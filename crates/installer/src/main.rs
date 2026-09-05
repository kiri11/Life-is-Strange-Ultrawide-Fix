// Copyright (C) 2026 Kiri11.  Free software under the GNU General Public
// License, version 3 or later - see LICENSE for the full terms.
//
// Additional term under GPL-3 section 7(b): every copy or modified version,
// in source or binary form, must preserve this notice and credit the
// original author, Kiri11, with a link to the original project at
// https://github.com/kiri11/Life-is-Strange-Ultrawide-Fix.

//! `lis-ultrawide-fix`: the installer, for Windows and Linux.
//!
//! Four independent parts; the first three are on by default, the fourth
//! is opt-in (`--sharpen`):
//!
//! 1. Ultrawide camera: installs the loader library next to the executable
//!    (embedded in this binary; see crates/loader and RESEARCH.md)
//! 2. Full-width UI: builds a mod container next to the game data
//! 3. Chromatic aberration off, and
//! 4. Anti-blur TSR settings: one managed block in the user's Engine.ini
//!
//! `install`, `restore`, `status` and `find` are the subcommands; with none,
//! at a terminal, it asks. Without a terminal it asks through a dialog tool
//! (zenity, kdialog or yad) when one is on the PATH, and otherwise does
//! nothing: a double-click never installs silently. `--yes` runs unattended.
//!
//! The Windows front-end (LiSUltrawidePatcher.cs) runs this binary and
//! reads its output. Its contract, which a change to either side must keep:
//! `status` prints `title:`, `status:`, `detail:`, `files:` and `filesdetail:`
//! lines; `find` prints one `known:` line per game the fix knows (title,
//! short title, executable name, tab-separated), `Game executable:` for the
//! game it picked, one `found:` line per installed copy of a game (title,
//! short title, path, where it was found), the pick first, and `Note:`
//! lines worth showing;
//! the exit code is 0 for success, 1 for a usage problem or a game that was
//! not found, 2 for an error the user can act on, 130 when cancelled; the
//! output is UTF-8; the phrase "as administrator" in the output means a
//! permission problem that elevation would solve; and the argument names
//! below are stable.

mod dialog;
mod flow;
mod ui;

use std::path::PathBuf;

use lis_ultrawide_core::{VERSION, hash, to_hex};

/// The loader DLL, embedded by build.rs; empty when the build had none.
static LOADER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/loader.dll"));

pub fn shipped_loader() -> Option<&'static [u8]> {
    (!LOADER.is_empty()).then_some(LOADER)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Install,
    Restore,
    Status,
    Find,
    /// No subcommand: the interactive flow.
    Interactive,
}

#[derive(Debug)]
pub struct Args {
    pub command: Command,
    pub exe: Option<PathBuf>,
    pub game: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub yes: bool,
    pub no_camera: bool,
    pub no_ui: bool,
    pub no_chromatic: bool,
    pub sharpen: bool,
    pub engine_ini: Option<PathBuf>,
    pub help: bool,
    pub version: bool,
}

pub fn usage() -> String {
    format!(
        "lis-ultrawide-fix {VERSION} - Life is Strange ultrawide fix installer

Usage: lis-ultrawide-fix [install|restore|status|find] [options]

  install      install the fix (asks what to install unless --yes)
  restore      undo everything the fix installed
  status       report what is installed, in lines a program can read
  find         print where the game was found and exit
  (no command) the interactive install

Options:
  --exe PATH          the game executable (found automatically when omitted)
  --game ID           which game: double-exposure or reunion (detected when omitted)
  --width W           display width, e.g. 5120 (detected when omitted)
  --height H          display height, e.g. 2160
  --yes, -y           accept the defaults, never ask, never open a dialog
  --no-camera         skip the ultrawide camera (the loader next to the executable)
  --no-ui             skip the full-width UI (the mod container next to the game data)
  --no-chromatic-fix  skip disabling chromatic aberration
  --sharpen           also write the recommended anti-blur TSR settings
  --engine-ini PATH   write the display tweaks to this Engine.ini instead of the
                      one found automatically (a prefix Steam does not manage)
  --version           print the version and exit
  --help              this text

Exit codes: 0 done, 1 usage or game not found, 2 a problem you can act on
(printed as one line), 130 cancelled."
    )
}

pub fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut a = Args {
        command: Command::Interactive,
        exe: None,
        game: None,
        width: None,
        height: None,
        yes: false,
        no_camera: false,
        no_ui: false,
        no_chromatic: false,
        sharpen: false,
        engine_ini: None,
        help: false,
        version: false,
    };
    let mut command_seen = false;
    let mut i = 0;
    let value = |i: &mut usize, name: &str, inline: Option<&str>| -> Result<String, String> {
        if let Some(v) = inline {
            return Ok(v.to_string());
        }
        *i += 1;
        argv.get(*i).cloned().ok_or_else(|| format!("{name} needs a value"))
    };
    while i < argv.len() {
        let arg = &argv[i];
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v)),
            _ => (arg.as_str(), None),
        };
        match name {
            "install" | "restore" | "status" | "find" if !arg.starts_with('-') => {
                if command_seen {
                    return Err(format!("only one command, not '{arg}' as well"));
                }
                command_seen = true;
                a.command = match name {
                    "install" => Command::Install,
                    "restore" => Command::Restore,
                    "status" => Command::Status,
                    _ => Command::Find,
                };
            }
            "--exe" => a.exe = Some(PathBuf::from(value(&mut i, name, inline)?)),
            "--game" => a.game = Some(value(&mut i, name, inline)?),
            "--width" => a.width = Some(value(&mut i, name, inline)?.parse().map_err(|_| "--width needs a number")?),
            "--height" => a.height = Some(value(&mut i, name, inline)?.parse().map_err(|_| "--height needs a number")?),
            "--engine-ini" => a.engine_ini = Some(PathBuf::from(value(&mut i, name, inline)?)),
            "--yes" | "-y" => a.yes = true,
            "--no-camera" => a.no_camera = true,
            "--no-ui" => a.no_ui = true,
            "--no-chromatic-fix" => a.no_chromatic = true,
            "--sharpen" => a.sharpen = true,
            "--help" | "-h" => a.help = true,
            "--version" | "-V" => a.version = true,
            _ => return Err(format!("unknown argument '{arg}'")),
        }
        i += 1;
    }
    if a.width.is_some() != a.height.is_some() {
        return Err("--width and --height go together".into());
    }
    if matches!((a.width, a.height), (Some(0), _) | (_, Some(0))) {
        return Err("the resolution must be positive".into());
    }
    Ok(a)
}

fn main() {
    let argv: Vec<String> = match std::env::args_os().skip(1).map(|a| a.into_string()).collect() {
        Ok(v) => v,
        Err(bad) => {
            eprintln!("lis-ultrawide-fix: the argument {bad:?} is not valid Unicode");
            std::process::exit(1);
        }
    };
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("lis-ultrawide-fix: {e}");
            eprintln!("{}", usage());
            std::process::exit(1);
        }
    };
    if args.help {
        println!("{}", usage());
        return;
    }
    if args.version {
        // the release workflow matches the digest against the DLL it built
        println!(
            "LiS Ultrawide Fix {VERSION} (loader: {})",
            match shipped_loader() {
                Some(l) => format!("embedded, {} KB, sha256 {}", l.len() / 1024, &to_hex(&hash::sha256(l))[..16]),
                None => "not embedded".to_string(),
            }
        );
        return;
    }
    std::process::exit(flow::run(args));
}
