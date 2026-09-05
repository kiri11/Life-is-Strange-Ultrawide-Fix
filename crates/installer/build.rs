// Embeds the loader DLL in the installer, and on Windows gives the
// executable a version resource.
//
// The loader is a separate crate that has to be built first (it is a DLL
// for Windows whatever the installer is built for), so this looks for it:
// LIS_LOADER_DLL names it outright (what the release workflow does for the
// Linux build), otherwise the workspace's own target folder is searched for
// the loader built with the same profile. Without one the installer still
// builds, warns here, and refuses the camera step at run time; the release
// workflow checks the embedded size so a release can never ship that way.

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=LIS_LOADER_DLL");
    println!("cargo:rerun-if-env-changed=LIS_FIX_VERSION");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "release".into());

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("LIS_LOADER_DLL") {
        candidates.push(PathBuf::from(p));
    }
    // OUT_DIR is <target>/[<triple>/]<profile>/build/<pkg>-<hash>/out
    if let Some(profile_dir) = out_dir.ancestors().nth(3) {
        let dll = "lis_ultrawide_loader.dll";
        candidates.push(profile_dir.join(dll));
        if let Some(target_root) = profile_dir.parent() {
            candidates.push(target_root.join(&profile).join(dll));
            candidates.push(target_root.join("x86_64-pc-windows-msvc").join(&profile).join(dll));
            if let Some(grand) = target_root.parent() {
                // a cross build: <target>/<triple>/<profile>, the host build one up
                candidates.push(grand.join(&profile).join(dll));
                candidates.push(grand.join("x86_64-pc-windows-msvc").join(&profile).join(dll));
            }
        }
    }
    let found: Option<&Path> = candidates.iter().map(PathBuf::as_path).find(|c| c.is_file());
    let dest = out_dir.join("loader.dll");
    match found {
        Some(p) => {
            // Watch the one that was embedded. Cargo treats a missing
            // rerun-if-changed path as always changed, so naming every
            // candidate would rebuild and relink the installer every time.
            println!("cargo:rerun-if-changed={}", p.display());
            std::fs::copy(p, &dest).expect("copy the loader DLL");
            println!("cargo:warning=embedding the loader from {}", p.display());
        }
        None => {
            // Nothing to embed yet: watch every place one could appear, which
            // re-runs this until a loader is built (the placeholder state
            // costs a rebuild per build, and is never what ships).
            for c in &candidates {
                println!("cargo:rerun-if-changed={}", c.display());
            }
            std::fs::write(&dest, []).expect("write an empty loader placeholder");
            println!(
                "cargo:warning=no loader DLL found (build it first: cargo build --release -p lis-ultrawide-loader, \
                 or set LIS_LOADER_DLL); this installer will carry none"
            );
        }
    }

    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let version = std::env::var("LIS_FIX_VERSION").unwrap_or_else(|_| "0.0.0.0".to_string());
        let numeric = version
            .split('.')
            .map(|p| p.parse::<u16>().unwrap_or(0) as u64)
            .chain(std::iter::repeat(0))
            .take(4)
            .fold(0u64, |n, p| (n << 16) | p);
        let mut res = winresource::WindowsResource::new();
        res.set("CompanyName", "Kiri11");
        res.set("ProductName", "Life is Strange - Ultrawide Fix");
        res.set("FileDescription", "Installer for the Life is Strange ultrawide fix");
        res.set("InternalName", "lis-ultrawide-fix");
        res.set("OriginalFilename", "lis-ultrawide-fix.exe");
        res.set("LegalCopyright", "Copyright (C) 2026 Kiri11. GPL-3.0-or-later; see LICENSE.");
        res.set("FileVersion", &version);
        res.set("ProductVersion", &version);
        res.set_version_info(winresource::VersionInfo::FILEVERSION, numeric);
        res.set_version_info(winresource::VersionInfo::PRODUCTVERSION, numeric);
        res.compile().expect("compile the version resource");
    }
}
