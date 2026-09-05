//! The installer's flow: find the game, say what is installed, settle the
//! resolution and the parts, then install or restore them in order.

use std::path::{Path, PathBuf};

use lis_ultrawide_core::games::{self, Game};
use lis_ultrawide_core::report::{InstallError, Report, Stdout};
use lis_ultrawide_core::ui_layout::UiStatus;
use lis_ultrawide_core::{VERSION, display, engine_ini, loader_install, locate, ui_layout};

use crate::ui::{self, Ui};
use crate::{Args, Command, dialog, shipped_loader};

/// Every line goes to standard output and into a transcript, which a
/// dialog front-end shows at the end.
struct Out {
    lines: Vec<String>,
}

impl Report for Out {
    fn line(&mut self, text: &str) {
        Stdout.line(text);
        self.lines.push(text.to_string());
    }
}

/// Lines of a step, indented under its heading.
struct Indent<'a>(&'a mut dyn Report);

impl Report for Indent<'_> {
    fn line(&mut self, text: &str) {
        self.0.line(&format!("  {text}"));
    }
}

enum Fail {
    Exit(i32),
    Cancelled,
    /// Before any step ran: nothing was touched.
    Install(InstallError),
    /// A step failed; the steps after it still ran.
    Step(InstallError),
}

impl From<InstallError> for Fail {
    fn from(e: InstallError) -> Self {
        Fail::Install(e)
    }
}

type R<T> = Result<T, Fail>;

pub fn run(args: Args) -> i32 {
    let asks = matches!(args.command, Command::Interactive | Command::Install) && !args.yes;
    let mut ui: Box<dyn Ui> = if !asks {
        Box::new(ui::Silent)
    } else if ui::console_available() {
        Box::new(ui::Console)
    } else if let Some(d) = dialog::find("Life is Strange - Ultrawide Fix") {
        Box::new(d)
    } else {
        eprintln!(
            "lis-ultrawide-fix: nothing to ask in - no terminal, and no dialog tool (zenity, kdialog or yad) on \
             the PATH. Run it from a terminal, or pass --yes to install the defaults without asking."
        );
        return 1;
    };
    let mut out = Out { lines: Vec::new() };
    let code = match run_inner(&args, ui.as_mut(), &mut out) {
        Ok(code) => code,
        Err(Fail::Exit(code)) => code,
        Err(Fail::Cancelled) => {
            out.line("");
            out.line("Cancelled.");
            130
        }
        Err(Fail::Install(e)) => {
            out.line("");
            out.line(&format!("Error: {e}"));
            out.line("");
            out.line("Nothing was left half-applied - the game is as it was before this run.");
            2
        }
        Err(Fail::Step(e)) => {
            out.line("");
            out.line(&format!("Error: {e}"));
            out.line("");
            out.line("The other steps went through. Fix the problem above and run this again; a second run redoes every step.");
            2
        }
    };
    let tail: Vec<&str> = out.lines.iter().rev().take(24).map(String::as_str).collect::<Vec<_>>().into_iter().rev().collect();
    ui.finish(code == 0, &tail.join("\n"));
    code
}

fn self_dir() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf))
}

/// Which game, and where its executable is.
fn locate_game(args: &Args, ui: &mut dyn Ui, out: &mut dyn Report) -> R<(&'static dyn Game, PathBuf)> {
    let forced = match &args.game {
        Some(id) => Some(games::game_for_id(id).ok_or_else(|| {
            let known: Vec<&str> = games::GAMES.iter().map(|g| g.id()).collect();
            InstallError(format!("unknown game '{id}'; known: {}", known.join(", ")))
        })?),
        None => None,
    };
    if let Some(exe) = &args.exe {
        let game = forced.or_else(|| locate::game_of(exe)).unwrap_or(games::GAMES[0]);
        if !exe.is_file() {
            out.line(&format!("Error: could not find file at '{}'", exe.display()));
            return Err(Fail::Exit(1));
        }
        let abs = std::path::absolute(exe).unwrap_or_else(|_| exe.clone());
        return Ok((game, abs));
    }
    // every copy of every game (or of the forced one), most trustworthy first
    let wanted: Vec<&'static dyn Game> = match forced {
        Some(g) => vec![g],
        None => games::GAMES.to_vec(),
    };
    let candidates: Vec<(&'static dyn Game, locate::Found)> =
        wanted.iter().flat_map(|&g| locate::candidates(g, self_dir().as_deref()).into_iter().map(move |f| (g, f))).collect();
    match candidates.len() {
        1 => {
            let (game, found) = &candidates[0];
            out.line(&format!("Found game via {}", found.source));
            if args.command == Command::Find {
                out.line(&found_line(*game, found));
            }
            Ok((*game, found.exe.clone()))
        }
        n if n > 1 => {
            let items: Vec<String> = candidates.iter().map(|(g, f)| format!("{} ({})", g.title(), f.exe.display())).collect();
            let pick = ui.choose("More than one game, or more than one copy, was found; which one", &items, Some(0)).ok_or(Fail::Cancelled)?;
            let (game, found) = &candidates[pick];
            out.line(&format!("Found game via {}", found.source));
            // two copies of one game share the Engine.ini under the user's
            // profile: the display tweaks cannot be installed for one alone
            if candidates.iter().filter(|(g, _)| g.id() == game.id()).count() > 1 {
                out.line(&format!(
                    "Note: the copies of {} share one Engine.ini, so the display tweaks are installed and restored for all of them at once.",
                    game.title()
                ));
            }
            if args.command == Command::Find {
                // every installed game, the pick first: the front-end offers
                // them as a list
                out.line(&found_line(*game, found));
                for (i, (g, f)) in candidates.iter().enumerate() {
                    if i != pick {
                        out.line(&found_line(*g, f));
                    }
                }
            }
            Ok((*game, found.exe.clone()))
        }
        _ => {
            out.line(
                "Could not find the game automatically (searched next to this program, every Steam library, the Epic \
                 Games Launcher and the usual game folders).",
            );
            if args.command == Command::Find {
                return Err(Fail::Exit(1));
            }
            let game = forced.unwrap_or(games::GAMES[0]);
            let Some(text) = ui.ask_text(&format!("Enter the path to {}", game.exe_name())) else {
                return Err(Fail::Exit(1));
            };
            let exe = PathBuf::from(text.trim());
            if !exe.is_file() {
                out.line(&format!("Error: could not find file at '{}'", exe.display()));
                return Err(Fail::Exit(1));
            }
            let game = forced.or_else(|| locate::game_of(&exe)).unwrap_or(game);
            Ok((game, std::path::absolute(&exe).unwrap_or(exe)))
        }
    }
}

/// A `found:` line of `find`: title, short title, path and where it was found, tab-separated.
fn found_line(game: &dyn Game, found: &locate::Found) -> String {
    format!("found: {}\t{}\t{}\t{}", game.title(), game.short_title(), found.exe.display(), found.source)
}

struct Parts {
    camera: bool,
    ui: bool,
    chromatic: bool,
    sharpen: bool,
}

/// Recognize our files even when an installation is incomplete or outdated.
/// A different mod's proxy DLL alone must not count as this fix.
fn has_fix(game: &dyn Game, exe: &Path, engine_ini: Option<&Path>) -> bool {
    let camera = loader_install::camera_paths(game, exe);
    let mut backup = exe.as_os_str().to_os_string();
    backup.push(".original");
    if loader_install::is_our_dll(&camera.dll) || camera.ini.is_file() || camera.log.is_file() || Path::new(&backup).is_file() {
        return true;
    }
    if let (Some(fix), Some(paks)) = (game.ui(), game.paks_dir(exe)) {
        let paths = ui_layout::mod_paths(&paks, fix);
        if paths.container().iter().any(|p| p.is_file()) || paths.record.is_file() || paks.join(format!("{}.utoc.original", fix.source)).is_file() {
            return true;
        }
    }
    engine_ini::engine_ini_path(game, Some(exe), engine_ini)
        .and_then(|p| std::fs::read(p).ok())
        .is_some_and(|bytes| String::from_utf8_lossy(&bytes).lines().any(|line| line.trim() == game.ini_markers().0))
}

fn run_inner(args: &Args, ui: &mut dyn Ui, out: &mut Out) -> R<i32> {
    if args.command == Command::Find {
        // every game the fix knows, installed or not: the front-end builds
        // its file filter from these, so it never carries a name of its own
        for g in games::GAMES {
            out.line(&format!("known: {}\t{}\t{}", g.title(), g.short_title(), g.exe_name()));
        }
    }
    let (game, exe) = locate_game(args, ui, out)?;
    out.line(&"=".repeat(60));
    out.line(&format!(" {} - Ultrawide Fix v{VERSION}", game.title()));
    out.line(&"=".repeat(60));
    out.line(&format!("Game executable: {}", exe.display()));
    if args.command == Command::Find {
        return Ok(0);
    }

    let shipped = shipped_loader();
    let (camera_status, camera_detail) = loader_install::check_camera(game, &exe, shipped);
    let paks = game.paks_dir(&exe);
    let (ui_status, ui_detail) = match game.ui() {
        Some(fix) => ui_layout::check_ui(paks.as_deref(), fix),
        None => (UiStatus::None, "no full-width UI fix for this game yet".to_string()),
    };
    if args.command == Command::Status {
        // machine-readable, for the Windows front-end
        out.line(&format!("title: {}", game.title()));
        out.line(&format!("status: {}", camera_status.as_str()));
        out.line(&format!("detail: {camera_detail}"));
        out.line(&format!("files: {}", ui_status.as_str()));
        out.line(&format!("filesdetail: {ui_detail}"));
        return Ok(0);
    }
    out.line(&format!("Ultrawide camera: {camera_detail}"));
    out.line(&format!("Full-width UI: {ui_detail}"));
    if ui_status == UiStatus::Stale {
        out.line("  !! the game has been updated since this was installed - install again before playing.");
    }

    let mut restore = args.command == Command::Restore;
    if matches!(args.command, Command::Interactive | Command::Install) && !args.yes && has_fix(game, &exe, args.engine_ini.as_deref()) {
        let items = vec!["Reinstall / change the fix's options".to_string(), "Remove the fix (restore)".to_string()];
        restore = ui
            .choose(&format!("The ultrawide fix is already installed for {}. What would you like to do?", game.title()), &items, Some(0))
            .ok_or(Fail::Cancelled)? == 1;
    }
    let detected = if restore { None } else { display::detect_resolution() };
    let (width, height) = match (args.width, args.height) {
        (Some(w), Some(h)) => (w, h),
        _ if restore => detected.unwrap_or((1920, 1080)), // irrelevant when restoring
        _ if args.yes => match detected {
            Some(d) => {
                out.line(&format!("Display: {}x{} (detected)", d.0, d.1));
                d
            }
            None => {
                out.line("Error: could not detect the display - pass --width and --height");
                return Err(Fail::Exit(1));
            }
        },
        _ => ui.choose_resolution(detected).ok_or(Fail::Cancelled)?,
    };
    // chosen by hand, or not what this machine's display says: the loader
    // then gets told, instead of reading the display itself at launch
    let explicit = (detected != Some((width, height))).then_some((width, height));

    let mut parts = Parts { camera: !args.no_camera, ui: !args.no_ui, chromatic: !args.no_chromatic, sharpen: args.sharpen };
    if restore {
        if args.command != Command::Restore {
            // The menu removes the whole fix, regardless of install-only flags.
            parts = Parts { camera: true, ui: true, chromatic: true, sharpen: true };
        }
        parts.sharpen = true;
        return run_install(game, &exe, width, height, &parts, true, args.engine_ini.as_deref(), explicit, out);
    }

    if !args.yes {
        out.line("");
        out.line("What to install:");
        parts.camera = ui
            .ask_yes(
                "\n  Ultrawide camera - Hor+ cutscenes, dialogue and exploration with no\n  black bars and no zoom when a dialogue ends. Installs a small library\n  the game loads at start; the executable itself is not changed.",
                parts.camera,
            )
            .ok_or(Fail::Cancelled)?;
        parts.ui = ui
            .ask_yes(
                "\n  Full-width UI - loading screens cover the whole screen and the HUD\n  sits on the real screen edge. Adds a mod container next to the game data.",
                parts.ui,
            )
            .ok_or(Fail::Cancelled)?;
        parts.chromatic = ui
            .ask_yes(
                "\n  Disable chromatic aberration - removes the colour fringing that is\n  most visible at the widened edges. Writes Engine.ini.",
                parts.chromatic,
            )
            .ok_or(Fail::Cancelled)?;
        parts.sharpen = ui
            .ask_yes("\n  Reduce blurriness - recommended TSR settings for this resolution.\n  Writes Engine.ini.", parts.sharpen)
            .ok_or(Fail::Cancelled)?;
    }
    if !(parts.camera || parts.ui || parts.chromatic || parts.sharpen) {
        out.line("");
        out.line("Nothing selected - exiting.");
        return Ok(0);
    }
    run_install(game, &exe, width, height, &parts, false, args.engine_ini.as_deref(), explicit, out)
}

#[allow(clippy::too_many_arguments)]
fn run_install(
    game: &dyn Game,
    exe: &Path,
    width: u32,
    height: u32,
    parts: &Parts,
    restore: bool,
    engine_ini: Option<&Path>,
    explicit: Option<(u32, u32)>,
    out: &mut Out,
) -> R<i32> {
    let paks = game.paks_dir(exe);
    // The steps are independent, so one failing does not stop the others:
    // a restore that cannot touch the game data still takes the Engine.ini
    // block out, and the first failure is what the run reports at the end.
    let mut failed: Option<InstallError> = None;
    let mut step = |out: &mut Out, result: Result<(), InstallError>| {
        if let Err(e) = result {
            out.line(&format!("  Error: {e}"));
            failed.get_or_insert(e);
        }
    };
    if restore {
        out.line("");
        out.line("Restoring everything to stock...");
        if parts.camera {
            let r = loader_install::remove_camera(game, exe, engine_ini, out);
            step(out, r);
        }
        if parts.ui
            && let (Some(fix), Some(paks)) = (game.ui(), &paks)
        {
            let r = ui_layout::restore_ui(paks, fix, &mut Indent(out));
            step(out, r);
        }
        if parts.chromatic || parts.sharpen {
            let r = engine_ini::apply_engine_ini(game, Some(exe), width, height, false, false, true, engine_ini, out);
            step(out, r.map(|_| ()));
        }
        if let Some(e) = failed {
            return Err(Fail::Step(e));
        }
        out.line("");
        out.line("Done - the game is back to its shipped state.");
        return Ok(0);
    }

    out.line("");
    out.line(&format!("Installing for {width}x{height} ({:.4}:1)", width as f64 / height as f64));

    out.line("");
    out.line("[1/3] Ultrawide camera (loader library)");
    if parts.camera {
        let r = loader_install::install_camera(game, exe, shipped_loader(), explicit, engine_ini, out);
        step(out, r);
    } else {
        out.line("  skipped");
    }

    out.line("");
    out.line("[2/3] Full-width UI (game files)");
    if parts.ui {
        match (game.ui(), &paks) {
            (Some(fix), Some(paks)) => {
                let r = ui_layout::install_ui(paks, fix, width, height, &mut Indent(out));
                step(out, r);
            }
            _ => out.line("  not available for this game"),
        }
    } else {
        out.line("  skipped");
    }

    out.line("");
    out.line("[3/3] Display tweaks (Engine.ini)");
    if parts.chromatic || parts.sharpen {
        let r = engine_ini::apply_engine_ini(game, Some(exe), width, height, parts.chromatic, parts.sharpen, false, engine_ini, out);
        step(out, r.map(|_| ()));
    } else {
        out.line("  skipped");
    }

    if let Some(e) = failed {
        return Err(Fail::Step(e));
    }
    out.line("");
    out.line(&"=".repeat(60));
    out.line(" Done.");
    out.line(" Launch the game through Steam.");
    out.line(&"=".repeat(60));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lis_ultrawide_core::games::double_exposure::DOUBLE_EXPOSURE;

    struct Answers {
        action: Option<usize>,
        choices: usize,
        questions: usize,
    }

    impl Ui for Answers {
        fn choose(&mut self, title: &str, items: &[String], _: Option<usize>) -> Option<usize> {
            assert!(title.contains("already installed"));
            assert!(items[1].contains("restore"));
            self.choices += 1;
            self.action
        }
        fn ask_yes(&mut self, _: &str, _: bool) -> Option<bool> {
            assert_eq!(self.action, Some(0), "restore must skip component questions");
            self.questions += 1;
            Some(self.questions == 3) // reinstall only the Engine.ini block
        }
        fn ask_text(&mut self, _: &str) -> Option<String> {
            panic!("no path or resolution question expected")
        }
    }

    #[test]
    fn existing_fix_can_be_cancelled_reinstalled_or_restored() {
        let game: &dyn Game = &DOUBLE_EXPOSURE;
        let tmp = std::env::temp_dir().join(format!("lis-flow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let exe = tmp.join(game.exe_relative());
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"MZ").unwrap();
        // An explicit non-Steam settings path prevents tests using a real prefix.
        let ini = tmp.join("Engine.ini");
        let mut args = crate::parse_args(&[]).unwrap();
        args.exe = Some(exe.clone());
        args.engine_ini = Some(ini.clone());
        let camera = loader_install::camera_paths(game, &exe);
        std::fs::write(&camera.dll, b"some other mod").unwrap();
        assert!(!has_fix(game, &exe, Some(&ini)), "foreign DLL is not our fix");
        let user = "[UserSettings]\nKeepMe=1\n";
        let installed = format!("{user}\n{}", engine_ini::build_ini_block(game, 3440, 1440, true, false));
        std::fs::write(&ini, &installed).unwrap();
        assert!(has_fix(game, &exe, Some(&ini)), "settings-only installation");
        let mut out = Out { lines: Vec::new() };
        let mut answers = Answers { action: None, choices: 0, questions: 0 };
        assert!(matches!(run_inner(&args, &mut answers, &mut out), Err(Fail::Cancelled)));
        assert_eq!(std::fs::read_to_string(&ini).unwrap(), installed);
        assert_eq!(answers.choices, 1);

        args.width = Some(5120);
        args.height = Some(2160);
        answers = Answers { action: Some(0), choices: 0, questions: 0 };
        assert!(matches!(run_inner(&args, &mut answers, &mut out), Ok(0)));
        assert_eq!(answers.choices, 1);
        assert_eq!(answers.questions, 4);
        assert_eq!(std::fs::read_to_string(&ini).unwrap().matches(game.ini_markers().0).count(), 1);

        // --yes remains unattended, even when a fix is already installed.
        args.yes = true;
        args.no_camera = true;
        args.no_ui = true;
        answers = Answers { action: None, choices: 0, questions: 0 };
        assert!(matches!(run_inner(&args, &mut answers, &mut out), Ok(0)));
        assert_eq!(answers.choices, 0);

        // A partial UI install is removable even without the source containers.
        let paks = game.paks_dir(&exe).unwrap();
        let mp = ui_layout::mod_paths(&paks, game.ui().unwrap());
        std::fs::create_dir_all(mp.pak.parent().unwrap()).unwrap();
        std::fs::write(&mp.pak, b"partial mod").unwrap();
        args.yes = false;
        args.width = None;
        args.height = None;
        answers = Answers { action: Some(1), choices: 0, questions: 0 };
        assert!(matches!(run_inner(&args, &mut answers, &mut out), Ok(0)));
        assert_eq!(answers.choices, 1);
        assert_eq!(answers.questions, 0);
        assert_eq!(std::fs::read_to_string(&ini).unwrap(), user);
        assert!(!mp.pak.exists());
        assert_eq!(std::fs::read(&camera.dll).unwrap(), b"some other mod");
        assert!(!has_fix(game, &exe, Some(&ini)));

        std::fs::write(&camera.dll, loader_install::DLL_MARKER.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>()).unwrap();
        assert!(has_fix(game, &exe, Some(&ini)), "old camera loader recognized without a shipped DLL");
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
