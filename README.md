# Life is Strange: Double Exposure and Reunion - Ultrawide Fix

A native ultrawide fix for **Life is Strange: Double Exposure** and **Life is Strange: Reunion**. Works with 21:9, 32:9, 32:10, 16:10 and any other resolution - the installer detects yours automatically.

- Cutscenes, dialogues and exploration fill the whole screen with no black bars and no cropping.
- No camera zoom or snap when a dialogue or cutscene ends.
- Photos and Max's Polaroids keep their correct proportions.
- Loading screens and HUD elements use the full width of the screen.
- Optional: disable chromatic aberration and reduce blurriness.

Both games get all three parts; for Reunion the UI part is the same fix in the game's newer engine formats (see [RESEARCH.md](RESEARCH.md), section 13).

The game's own files are never modified: the camera fix is a small library the game loads at start, and the UI fix is a mod container next to the game data. Nothing runs in the background, nothing is downloaded, nothing is installed system-wide, and there is no performance impact. Everything can be undone with a single **Restore** button.

---

## Installation (Windows)

1. **[Download the fix](https://github.com/kiri11/Life-is-Strange-Ultrawide-Fix/releases/latest/download/LiS-Ultrawide-Fix-Windows.zip)** and unpack the zip anywhere.
2. Run **`LiSUltrawidePatcher.exe`**.
3. It finds your game and your display automatically. If it does not find the game, click **Browse...** and select the game executable. With more than one game installed, or a game installed twice, the executable field becomes a list: pick the one to fix, and run **Install** once for each. The window title says which game is selected.
4. Leave the options ticked and click **Install**.

That is it. Start the game and play.

**Windows will warn you the first time.** The program is not code-signed, so SmartScreen shows *"Windows protected your PC"*. Click **More info**, then **Run anyway**. Some antivirus tools also flag it, as they do most unsigned game mods. The program is built by GitHub Actions from the source in this repository, so you can check exactly what it does.

The window is a front-end for `cli\lis-ultrawide-fix.exe`, the installer in the `cli` folder next to it, which also works on its own from a command prompt (see the Linux section for its commands).

### After a game update

The camera fix finds the code it patches by signature at every launch, so an update that only moves things around changes nothing. If the game's code itself changed, the fix stays inactive and says why in `LiSUltrawideCamera.log` next to the game executable; a new version of the fix is then needed. The full-width UI is built for one build of the game: run **Install** again after an update, and the installer tells you when it is stale.

Steam's **Verify Integrity of Game Files** does not affect the fix. It only restores the game's own files, and the fix adds files without changing any.

### Undoing the fix

Click **Restore original** in the installer.

---

## Installation (Linux and Steam Deck)

> **Verified on Bazzite** (Desktop Mode, Steam, Proton) as well as on Windows. The installer also supports SteamOS, the Steam Deck and Flatpak or Snap Steam, which have not been tried on real hardware yet. If something does not work, please [open an issue](https://github.com/kiri11/Life-is-Strange-Ultrawide-Fix/issues).

The installer is a single static binary; it needs nothing installed.

1. Start the game once and quit, so that Steam creates the Proton prefix that holds `Engine.ini` and Wine's registry.
2. **[Download the fix](https://github.com/kiri11/Life-is-Strange-Ultrawide-Fix/releases/latest/download/LiS-Ultrawide-Fix-Linux.tar.gz)** and unpack it. On the Steam Deck, switch to **Desktop Mode** first.
3. Open a terminal (**Konsole** on the Deck), `cd` into the unpacked folder and run:

```bash
./lis-ultrawide-fix
```

It asks what to install. Double-clicking `lis-ultrawide-fix` in the file manager works too: it then asks through the desktop's own dialogs (zenity, kdialog or yad, whichever is there) and never installs anything without asking. Or use the commands and flags directly:

```bash
./lis-ultrawide-fix install --yes
./lis-ultrawide-fix install --width 5120 --height 2160 --yes
./lis-ultrawide-fix restore
```

| Command or flag | Purpose |
| :--- | :--- |
| `install`, `restore` | Install the fix, or undo everything it installed |
| `status` | Report whether the camera loader and the UI container are installed and current |
| `find` | Print where the game was found and exit |
| `--game double-exposure` or `--game reunion` | Which game, when both are installed (the first found otherwise) |
| `--exe <path>` | Point at a specific `Chronos-Win64-Shipping.exe` or `Iris-Win64-Shipping.exe` |
| `--width`, `--height` | Install for this resolution instead of the detected one |
| `--yes` | Take the defaults without asking |
| `--engine-ini <path>` | Path to `Engine.ini` inside a prefix Steam does not manage (Heroic, Lutris, plain Wine) |
| `--no-camera`, `--no-ui`, `--no-chromatic-fix` | Skip individual parts of the fix |
| `--sharpen` | Also apply the anti-blur settings |

With Steam, `Engine.ini` lives at `steamapps/compatdata/1874000/pfx/drive_c/users/steamuser/AppData/Local/Chronos/Saved/Config/Windows/Engine.ini` (Double Exposure) or `steamapps/compatdata/2624870/pfx/drive_c/users/steamuser/AppData/Local/Iris/Saved/Config/Windows/Engine.ini` (Reunion) and is found automatically.

Wine loads its own `winhttp.dll` unless told otherwise, so the installer registers the fix's library in the prefix's `user.reg`, for the game's executable only; **Restore** removes the entry again. For a prefix Steam does not manage, `--engine-ini` tells the installer which prefix that is. If the library still does not run (no `LiSUltrawideCamera.log` appears next to the game executable after a launch), add `WINEDLLOVERRIDES="winhttp=n,b" %command%` to the game's launch options.

### Gaming Mode (gamescope)

In Gaming Mode the game only sees the screen gamescope gives it, and Steam's per-game resolution list stops at that size. If gamescope runs at 16:9 (3840x2160, say), the game renders 16:9, gamescope adds the side bars, and the camera fix stays inactive; the log will show that 16:9 size. Nothing needs reinstalling: the loader reads the screen at every launch, so the fix switches on as soon as gamescope's screen has the panel's aspect.

To set that screen, put the panel size in `~/.config/environment.d/gamescope-session-plus.conf` and log out of Gaming Mode and back in (Bazzite, SteamOS and ChimeraOS sessions all read this file):

```
SCREEN_WIDTH=5120
SCREEN_HEIGHT=2160
```

Then pick **Native** in the game's Steam properties; 5120x2160 is not in Steam's preset list. The fix only needs the aspect, not the pixel count: a lighter 21:9 screen such as `3440` x `1440` works just as well and is the setting to use if the full size is too heavy for the GPU. On NVIDIA, keep HDR off in Gaming Mode at sizes above 2560x1440, which still corrupts the picture with current drivers.

---

## What each option does

| Option | Effect | Changes |
| :--- | :--- | :--- |
| **Ultrawide camera** | Full-width cutscenes, dialogue and exploration | adds `winhttp.dll`, the loader, next to the game executable (`Chronos/Binaries/Win64` or `Iris/Binaries/Win64`) |
| **Full-width UI** (Double Exposure) | Loading screens and HUD use the whole screen | adds `Content/Paks/Mods/LiSUltrawideUI_P.*` |
| **Disable chromatic aberration** | Removes colour fringing at the edges | `Engine.ini` |
| **Reduce blurriness** | Recommended TSR settings for your resolution (off by default) | `Engine.ini` |

The loader writes `LiSUltrawideCamera.log` next to itself at every launch, saying what it did. The `Engine.ini` settings are added as a clearly marked block. Running the installer again never stacks changes, and an executable that a version of the fix from before September 2026 edited is put back to stock from its backup.

---

## Troubleshooting

- **Windows or your antivirus blocked the installer:** click **More info**, then **Run anyway**. If an antivirus quarantined it, restore the file and exclude the folder.
- **The installer cannot find the game:** click **Browse** and select `Chronos\Binaries\Win64\Chronos-Win64-Shipping.exe` (Double Exposure) or `Iris\Binaries\Win64\Iris-Win64-Shipping.exe` (Reunion) inside the game folder.
- **"The system refused permission to write the game files":** the game is installed somewhere only an administrator may write to. The installer offers to run again as administrator - say yes.
- **The camera fix is not active:** open `LiSUltrawideCamera.log` next to the game executable. It says whether the loader ran, what it found, and why it applied nothing. If it says the game's build is not one the fix knows, a new version of the fix is needed. If there is no log at all, the game did not load the library: on Linux, see the launch-option note above.
- **The installer says another winhttp.dll is next to the game:** another mod's loader uses the same name. Only one can load, so move it away, or untick the camera part.
- **A cutscene shows black bars:** report which scene, so its camera can be included.
- **The UI is still 16:9 in game (Double Exposure):** check that `Chronos/Content/Paks/Mods/LiSUltrawideUI_P.utoc`, `.ucas` and `.pak` are all present. If they are, open an issue with your resolution.
- **"Could not locate Engine.ini" on Linux or the Steam Deck:** start the game once, quit, and run the installer again.

### Still broken? Open an issue

[Open an issue](https://github.com/kiri11/Life-is-Strange-Ultrawide-Fix/issues) and include:

- What you did and which options were ticked.
- What is wrong and where, with a screenshot if possible.
- Your display resolution and where the game came from (Steam, Epic, other).
- The installer's log output.

---

## Technical details

The fix changes a handful of bytes in the game's code, in memory at every launch, to force Unreal Engine's built-in Hor+ projection for every camera, and adds a small mod container with full-width versions of the game's UI packages. The complete reverse-engineering breakdown is in **[RESEARCH.md](RESEARCH.md)** (sections 1 to 12 for Double Exposure, 13 for Reunion). Each game is one descriptor under `crates/core/src/games/`: names, paths, the signatures of its patch sites and the bytes of its caves.

Everything is Rust, in one workspace. The only crates it pulls in are pure Rust: `blake3`, `sha1` and `sha2` for the three digests the container formats use, and `winresource` at build time for the Windows version resources. Nothing is needed at run time beyond the operating system:

| Crate | What it is |
| :--- | :--- |
| [`crates/core`](crates/core) | The logic: the camera patch planner, the Oodle Kraken decoder, the IoStore and Zen package readers, the container writer, the UI slot edits, the `Engine.ini` block, and where games, Steam libraries and Wine prefixes are |
| [`crates/loader`](crates/loader) | The library the game loads as `winhttp.dll`: it forwards that DLL's functions to the system copy and patches the game before its own code runs |
| [`crates/installer`](crates/installer) | `lis-ultrawide-fix`, the command-line installer for Windows and Linux, with the loader embedded |

`LiSUltrawidePatcher.exe` is a thin Windows window that runs `lis-ultrawide-fix.exe`, which it looks for in a `cli` folder next to itself and then in its own folder; it is compiled from [`LiSUltrawidePatcher.cs`](LiSUltrawidePatcher.cs) with the compiler that ships with Windows. [The release workflow](.github/workflows/build.yml) tests the workspace on Linux and Windows, builds the loader and the installers, and names the commit each release was built from. To build it yourself:

```
cargo build --release -p lis-ultrawide-loader
cargo build --release -p lis-ultrawide-fix
%WINDIR%\Microsoft.NET\Framework64\v4.0.30319\csc.exe /target:winexe /win32icon:LiSUltrawidePatcher.ico /out:LiSUltrawidePatcher.exe LiSUltrawidePatcher.cs /r:System.dll /r:System.Windows.Forms.dll /r:System.Drawing.dll
```

The loader goes first because the installer embeds it. `cargo test --release --workspace` runs the tests; the ones against the game's own files run where the game is installed and skip elsewhere.

---

## License

This project is licensed under the **GNU General Public License v3.0 or later** - see [LICENSE](LICENSE). The source of every release is this repository at the commit the release names. The Kraken decoder is a port of the Kraken parts of [ooz](https://github.com/powzix/ooz), Copyright (C) 2016 Powzix, under the same license.

As an additional term under GPL-3.0 section 7(b), every copy or modified version must preserve the copyright notice and credit the original author, Kiri11, with a link to this project.
