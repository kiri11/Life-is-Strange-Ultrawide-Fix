//! The game search and the Engine.ini lookup against a fake Steam layout
//! built in a temporary home directory.
//!
//! This is the only place the Linux paths are ever exercised: the fix is
//! developed on Windows, and CI's Ubuntu runner is the one Linux it sees.
//! The fake layout has the Steam install in ~/.local/share/Steam, a second
//! library registered in its libraryfolders.vdf with the older SteamApps
//! spelling, and the game inside that second library - which is where a
//! Steam Deck's SD card or a separate drive on any distribution puts it.
//! The test then checks that the Proton prefix is found from the game's own
//! path, that it is not claimed before the game's first launch, that
//! --engine-ini overrides the search, and that the managed block
//! round-trips inside the prefix.
//!
//! It rewrites HOME and friends, so it is the only test in this binary.

use std::path::{Path, PathBuf};

use lis_ultrawide_core::engine_ini::{apply_engine_ini, engine_ini_path};
use lis_ultrawide_core::games::Game;
use lis_ultrawide_core::games::double_exposure::DOUBLE_EXPOSURE;
use lis_ultrawide_core::report::write_failure;
use lis_ultrawide_core::{display, locate, steam};

fn same(a: &Path, b: &Path) -> bool {
    steam::path_key(a) == steam::path_key(b)
}

#[test]
fn finds_the_game_and_its_prefix_in_a_fake_steam_layout() {
    let game: &dyn Game = &DOUBLE_EXPOSURE;
    let tmp = std::env::temp_dir().join(format!("lisde-paths-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let home = tmp.join("home");
    let library = tmp.join("Library2");
    unsafe {
        std::env::remove_var("LOCALAPPDATA");
        std::env::remove_var("PROGRAMDATA");
        std::env::set_var("HOME", &home);
        std::env::set_var("USERPROFILE", &home);
    }

    let steam_dir = home.join(".local").join("share").join("Steam");
    std::fs::create_dir_all(steam_dir.join("steamapps")).unwrap();
    let esc = |p: &Path| p.display().to_string().replace('\\', "\\\\");
    std::fs::write(
        steam_dir.join("steamapps").join("libraryfolders.vdf"),
        format!(
            "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
            esc(&steam_dir),
            esc(&library)
        ),
    )
    .unwrap();
    let flatpak = home.join(".var").join("app").join("com.valvesoftware.Steam").join("data").join("Steam");
    let snap = home.join("snap").join("steam").join("common").join(".local").join("share").join("Steam");
    std::fs::create_dir_all(&flatpak).unwrap();
    std::fs::create_dir_all(&snap).unwrap();

    let exe = library.join("SteamApps").join("common").join(game.install_dir()).join(game.exe_relative());
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"MZ").unwrap();

    let installs = steam::installs();
    for root in [&steam_dir, &flatpak, &snap] {
        assert!(installs.iter().any(|p| same(root, p)), "Steam root not searched: {}", root.display());
    }
    assert!(steam::libraries().iter().any(|p| same(&library, p)), "library from libraryfolders.vdf not searched");
    let found: Vec<_> = locate::candidates(game, None).into_iter().filter(|f| same(&f.exe, &exe)).collect();
    assert!(!found.is_empty(), "the game in the second library was not found");
    assert!(found[0].source.starts_with("Steam library"), "the game was found, but not through Steam: {:?}", found[0]);

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        std::fs::create_dir_all(home.join(".steam")).unwrap();
        symlink(&steam_dir, home.join(".steam/steam")).unwrap();
        symlink(&steam_dir, home.join(".steam/root")).unwrap();
        let home_alias = tmp.join("home-alias");
        symlink(&home, &home_alias).unwrap();
        let library_alias = tmp.join("library-alias");
        symlink(&library, &library_alias).unwrap();
        // Register aliases as well as the real library, as Steam does on Bazzite.
        std::fs::write(
            steam_dir.join("steamapps/libraryfolders.vdf"),
            format!("\"path\" \"{}\"\n\"path\" \"{}\"\n\"path\" \"{}\"\n",
                esc(&home_alias.join(".local/share/Steam")), esc(&library), esc(&library_alias)),
        ).unwrap();
        assert_eq!(steam::installs().iter().filter(|p| same(p, &steam_dir)).count(), 1);
        assert_eq!(steam::libraries().iter().filter(|p| same(p, &library)).count(), 1);
        // A symlinked game directory and a local discovery alias must merge too.
        let common = library.join("SteamApps/common");
        symlink(common.join(game.install_dir()), common.join(format!("{}-alias", game.install_dir()))).unwrap();
        let alias_exe = library_alias.join(exe.strip_prefix(&library).unwrap());
        let copies = locate::candidates(game, alias_exe.parent());
        assert_eq!(copies.iter().filter(|f| same(&f.exe, &exe)).count(), 1);
        assert_eq!(copies[0].exe, alias_exe, "keep the first path and its Steam ancestry");
        // Identical bytes in an independent directory are still a separate copy.
        let independent = common.join(format!("{}-copy", game.install_dir())).join(game.exe_relative());
        std::fs::create_dir_all(independent.parent().unwrap()).unwrap();
        std::fs::copy(&exe, &independent).unwrap();
        let copies = locate::candidates(game, None);
        assert_eq!(copies.iter().filter(|f| same(&f.exe, &exe)).count(), 1);
        assert_eq!(copies.iter().filter(|f| same(&f.exe, &independent)).count(), 1);
        assert_ne!(steam::path_key(&exe), steam::path_key(&independent));
    }

    // the prefix does not exist until the game has been started once
    assert_eq!(engine_ini_path(game, Some(&exe), None), None, "an Engine.ini path was returned with no prefix");
    let prefix = library.join("SteamApps").join("compatdata").join(game.steam_appid().to_string()).join("pfx");
    std::fs::create_dir_all(prefix.join("drive_c")).unwrap();
    let want = prefix
        .join("drive_c")
        .join("users")
        .join("steamuser")
        .join("AppData")
        .join("Local")
        .join("Chronos")
        .join("Saved")
        .join("Config")
        .join("Windows")
        .join("Engine.ini");
    let got = engine_ini_path(game, Some(&exe), None);
    assert!(got.as_deref().is_some_and(|g| same(g, &want)), "Engine.ini not found from the game path: {got:?}");
    // ...and through the libraries alone, when the game path gives no lead
    let got = engine_ini_path(game, Some(&tmp.join("elsewhere").join(game.exe_name())), None);
    assert!(got.as_deref().is_some_and(|g| same(g, &want)), "Engine.ini not found through the libraries: {got:?}");
    let override_ = tmp.join("heroic").join("Engine.ini");
    assert!(same(&engine_ini_path(game, Some(&exe), Some(&override_)).unwrap(), &override_), "--engine-ini was not honoured");

    // the managed block goes in and comes out again, creating the folders
    let mut r: Vec<String> = Vec::new();
    apply_engine_ini(game, Some(&exe), 5120, 2160, true, true, false, None, &mut r).unwrap();
    assert!(want.is_file(), "Engine.ini was not written into the prefix");
    assert!(std::fs::read_to_string(&want).unwrap().contains(game.ini_markers().0.trim()), "the managed block is missing");
    apply_engine_ini(game, Some(&exe), 5120, 2160, false, false, true, None, &mut r).unwrap();
    assert_eq!(std::fs::read_to_string(&want).unwrap(), "", "the managed block was not removed");

    // a permission error reads the same on every platform
    let err = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    assert!(!write_failure(&exe, &err).contains("Windows"), "permission wording names Windows");
    if let Some((w, h)) = display::detect_resolution() {
        assert!(w > 0 && h > 0);
    }
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = PathBuf::new();
}
