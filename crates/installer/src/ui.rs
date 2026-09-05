//! How the installer asks: at a terminal, through a dialog tool, or not at
//! all. The flow only ever calls these; the choice of resolution is built
//! on them so every front-end offers the same list.

use std::io::{BufRead, IsTerminal, Write};

use lis_ultrawide_core::report::{Report, Stdout};

/// Common displays, offered when the detected one is not what the user wants.
pub const PRESETS: &[(&str, u32, u32)] = &[
    ("5120x2160 (21:9 WUHD 4K)", 5120, 2160),
    ("3440x1440 (21:9 UWQHD)", 3440, 1440),
    ("2560x1080 (21:9 UWD)", 2560, 1080),
    ("3840x1600 (24:10 UW)", 3840, 1600),
    ("5120x1440 (32:9 Super Ultrawide)", 5120, 1440),
    ("3840x1080 (32:9 Super Ultrawide)", 3840, 1080),
    ("7680x2160 (32:9 Super Ultrawide)", 7680, 2160),
    ("3840x1200 (32:10)", 3840, 1200),
    ("2560x1600 (16:10)", 2560, 1600),
    ("1280x800 (16:10 Steam Deck)", 1280, 800),
];

/// A question's answer, or None when the user cancelled (end of input, a
/// closed dialog).
pub trait Ui {
    fn ask_yes(&mut self, question: &str, default: bool) -> Option<bool>;
    /// Pick one of `items`; `default` is preselected where the front-end can.
    fn choose(&mut self, title: &str, items: &[String], default: Option<usize>) -> Option<usize>;
    fn ask_text(&mut self, prompt: &str) -> Option<String>;
    /// The run is over: a dialog front-end shows the outcome, others do nothing.
    fn finish(&mut self, _ok: bool, _summary: &str) {}

    /// The display resolution to install for.
    fn choose_resolution(&mut self, detected: Option<(u32, u32)>) -> Option<(u32, u32)> {
        let mut items: Vec<String> = PRESETS
            .iter()
            .map(|(name, w, h)| {
                if detected == Some((*w, *h)) { format!("{name}   <- detected") } else { name.to_string() }
            })
            .collect();
        let detected_index = PRESETS.iter().position(|(_, w, h)| detected == Some((*w, *h)));
        // a detected display that is no preset gets its own entry, and
        // "Custom..." is always last, which is what the console's "c" picks
        let detected_entry = match detected {
            Some((w, h)) if detected_index.is_none() => {
                items.push(format!("Detected: {w}x{h}"));
                Some(items.len() - 1)
            }
            _ => None,
        };
        items.push("Custom...".to_string());
        let default = detected_index.or(detected_entry);
        let picked = self.choose("Select your display resolution", &items, default)?;
        if picked < PRESETS.len() {
            return Some((PRESETS[picked].1, PRESETS[picked].2));
        }
        if Some(picked) == detected_entry {
            return detected;
        }
        loop {
            let text = self.ask_text("Resolution, as WIDTHxHEIGHT (for example 5120x2160)")?;
            if let Some((w, h)) = parse_resolution(&text) {
                return Some((w, h));
            }
            if text.trim().is_empty() {
                return None;
            }
        }
    }
}

pub fn parse_resolution(text: &str) -> Option<(u32, u32)> {
    let nums: Vec<u32> = text.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).map(|s| s.parse().ok()).collect::<Option<_>>()?;
    match nums[..] {
        [w, h] if w > 0 && h > 0 => Some((w, h)),
        _ => None,
    }
}

/// Never asks: every question takes its default, and a question with no
/// default is left unanswered.
pub struct Silent;

impl Ui for Silent {
    fn ask_yes(&mut self, _: &str, default: bool) -> Option<bool> {
        Some(default)
    }
    fn choose(&mut self, _: &str, _: &[String], default: Option<usize>) -> Option<usize> {
        default
    }
    fn ask_text(&mut self, _: &str) -> Option<String> {
        None
    }
}

/// A terminal to read answers from. Only standard input has to be one:
/// piping the output through `tee` for a bug report still asks.
pub fn console_available() -> bool {
    std::io::stdin().is_terminal()
}

/// Questions on the terminal, answers from standard input.
pub struct Console;

impl Console {
    fn read_line(&self, prompt: &str) -> Option<String> {
        let mut out = std::io::stdout().lock();
        let _ = write!(out, "{prompt}");
        let _ = out.flush();
        let mut line = String::new();
        match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => {
                let _ = writeln!(out);
                None
            }
            Ok(_) => Some(line.trim().to_string()),
        }
    }
}

impl Ui for Console {
    fn ask_yes(&mut self, question: &str, default: bool) -> Option<bool> {
        let answer = self.read_line(&format!("{question} {}: ", if default { "[Y/n]" } else { "[y/N]" }))?;
        Some(if answer.is_empty() { default } else { answer.to_lowercase().starts_with('y') })
    }

    fn choose(&mut self, title: &str, items: &[String], default: Option<usize>) -> Option<usize> {
        let mut r = Stdout;
        r.line("");
        r.line(&format!("{title}:"));
        for (i, item) in items.iter().enumerate() {
            r.line(&format!("  [{}] {item}", i + 1));
        }
        let prompt = match default {
            Some(d) => format!("\nEnter choice [1-{}, or Enter for {}]: ", items.len(), d + 1),
            None => format!("\nEnter choice [1-{}]: ", items.len()),
        };
        loop {
            let answer = self.read_line(&prompt)?;
            if answer.is_empty() {
                if let Some(d) = default {
                    return Some(d);
                }
                continue;
            }
            match answer.parse::<usize>() {
                Ok(n) if (1..=items.len()).contains(&n) => return Some(n - 1),
                _ => {
                    // "c" for the custom entry, as the old installer took
                    if answer.eq_ignore_ascii_case("c") {
                        return Some(items.len() - 1);
                    }
                    r.line("Unrecognised choice.");
                }
            }
        }
    }

    fn ask_text(&mut self, prompt: &str) -> Option<String> {
        self.read_line(&format!("{prompt}: ")).map(|s| s.trim_matches(|c| c == '"' || c == '\'').to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolutions_parse_in_the_common_spellings() {
        assert_eq!(parse_resolution("5120x2160"), Some((5120, 2160)));
        assert_eq!(parse_resolution(" 3440 X 1440 "), Some((3440, 1440)));
        assert_eq!(parse_resolution("2560*1080"), Some((2560, 1080)));
        assert_eq!(parse_resolution("wide"), None);
        assert_eq!(parse_resolution("0x10"), None);
    }

    #[test]
    fn silent_takes_defaults_and_the_detected_display() {
        let mut s = Silent;
        assert_eq!(s.choose_resolution(Some((5120, 2160))), Some((5120, 2160)));
        assert_eq!(s.choose_resolution(Some((6000, 2000))), Some((6000, 2000)));
        assert_eq!(s.choose_resolution(None), None);
        assert_eq!(s.ask_yes("?", false), Some(false));
    }
}
