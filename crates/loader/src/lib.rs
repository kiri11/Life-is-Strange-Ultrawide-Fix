// Copyright (C) 2026 Kiri11.  Free software under the GNU General Public
// License, version 3 or later - see LICENSE for the full terms.
//
// Additional term under GPL-3 section 7(b): every copy or modified version,
// in source or binary form, must preserve this notice and credit the
// original author, Kiri11, with a link to the original project at
// https://github.com/kiri11/Life-is-Strange-Ultrawide-Fix.

//! The ultrawide camera fix, applied in memory when the game starts.
//!
//! Installed as `winhttp.dll` next to the game executable. The game imports
//! that DLL and Windows looks for it in the game's folder first, so this
//! library is loaded before the game's own code runs (`forward` says why
//! that name and not version.dll). It forwards every winhttp.dll function
//! to the system copy, and on load it finds the camera patch sites in the
//! mapped executable by signature and writes the bytes RESEARCH.md
//! describes. The file on disk is never touched.
//!
//! Everything that decides *what* to write is in the `lis_ultrawide_core`
//! crate and runs on any host, which is what the tests exercise. Only
//! `runtime` and `forward` talk to Windows. The loader stays a leaf: no
//! threads, no panics escaping (`DllMain` catches them), file I/O only for
//! its own ini and log.

#[cfg(windows)]
mod forward;
#[cfg(windows)]
mod runtime;
#[cfg(windows)]
mod win;

pub use lis_ultrawide_core::VERSION;

/// The DLL entry point.
///
/// # Safety
/// Called by the Windows loader only, with the arguments it documents.
#[cfg(windows)]
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(
    hinst: *mut core::ffi::c_void,
    reason: u32,
    _reserved: *mut core::ffi::c_void,
) -> i32 {
    const DLL_PROCESS_ATTACH: u32 = 1;
    if reason == DLL_PROCESS_ATTACH {
        unsafe { win::DisableThreadLibraryCalls(hinst) };
        // A bug here must never take the game down with it: the worst case is
        // the game running unmodified, and the log saying why.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            runtime::attach(hinst)
        }));
    }
    1
}
