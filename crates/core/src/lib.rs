// Copyright (C) 2026 Kiri11.  Free software under the GNU General Public
// License, version 3 or later - see LICENSE for the full terms.
//
// Additional term under GPL-3 section 7(b): every copy or modified version,
// in source or binary form, must preserve this notice and credit the
// original author, Kiri11, with a link to the original project at
// https://github.com/kiri11/Life-is-Strange-Ultrawide-Fix.

//! The fix, minus the operating system.
//!
//! Everything that decides what the fix does lives here and runs on any
//! host: the camera patch planner the loader runs over the mapped game
//! ([`plan`], [`scan`], [`pe`], [`games`]), the container formats and the
//! Kraken decoder the full-width UI is built with ([`iostore`], [`zen`],
//! [`kraken`], [`ui_layout`]), the managed `Engine.ini` block
//! ([`engine_ini`]), and where games, Steam libraries and Wine prefixes are
//! ([`locate`], [`steam`], [`wine`]). The loader and the installer are thin
//! shells around it.
//!
//! Which game is being fixed is a [`games::Game`] descriptor; adding a game
//! is one module under `games/`.

pub mod camera;
pub mod camera_ini;
pub mod display;
pub mod engine_ini;
pub mod games;
pub mod hash;
pub mod iostore;
pub mod json;
pub mod kraken;
pub mod loader_install;
pub mod locate;
pub mod pe;
pub mod plan;
pub mod report;
pub mod scan;
pub mod steam;
pub mod zen;
pub mod ui_layout;
pub mod unver;
pub mod wine;

/// The release this build belongs to, stamped by the release workflow.
pub const VERSION: &str = match option_env!("LIS_FIX_VERSION") {
    Some(v) => v,
    None => "dev",
};

/// `"0FB6 83"`-style hex, spaces optional, into bytes.
pub fn hex(text: &str) -> Vec<u8> {
    let digits: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(digits.len().is_multiple_of(2), "odd number of hex digits");
    (0..digits.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&digits[i..i + 2], 16).expect("hex digit"))
        .collect()
}

/// Four hex bytes, the size of a float or a rel32.
pub fn hex4(text: &str) -> [u8; 4] {
    hex(text).try_into().expect("four bytes")
}

/// Bytes as lower-case hex.
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
