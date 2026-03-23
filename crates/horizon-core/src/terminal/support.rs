use std::path::PathBuf;
use std::sync::Arc;

use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{self, Rgb};

use super::TerminalEventProxy;

pub(super) trait ColorLookup {
    fn lookup(&self, index: usize) -> Rgb;
}

impl ColorLookup for alacritty_terminal::term::color::Colors {
    fn lookup(&self, index: usize) -> Rgb {
        self[index].unwrap_or_else(|| default_terminal_rgb(index))
    }
}

pub(super) fn default_terminal_rgb(index: usize) -> Rgb {
    if let Some(color) = TERMINAL_BASE_COLORS.get(index) {
        return *color;
    }

    match index {
        16..=231 => {
            let idx = index - 16;
            let steps = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];
            Rgb {
                r: steps[idx / 36],
                g: steps[(idx % 36) / 6],
                b: steps[idx % 6],
            }
        }
        232..=255 => {
            let value = 8 + ((index - 232) * 10);
            let value = u8::try_from(value).unwrap_or(u8::MAX);
            Rgb {
                r: value,
                g: value,
                b: value,
            }
        }
        256 | 267 => Rgb { r: 224, g: 230, b: 241 },
        257 | 268 => Rgb { r: 15, g: 19, b: 28 },
        258 => Rgb { r: 196, g: 223, b: 255 },
        _ => Rgb { r: 255, g: 255, b: 255 },
    }
}

pub(super) fn replay_terminal_bytes(term: &Arc<FairMutex<Term<TerminalEventProxy>>>, bytes: &[u8]) {
    let mut parser = ansi::Processor::<ansi::StdSyncHandler>::default();
    let mut terminal = term.lock();
    parser.advance(&mut *terminal, bytes);

    let reset_bytes = replay_mode_reset_bytes(*terminal.mode());
    if !reset_bytes.is_empty() {
        parser.advance(&mut *terminal, &reset_bytes);
    }
}

fn replay_mode_reset_bytes(mode: TermMode) -> Vec<u8> {
    let mut bytes = Vec::new();

    if mode.contains(TermMode::APP_CURSOR) {
        bytes.extend_from_slice(b"\x1b[?1l");
    }
    if mode.contains(TermMode::APP_KEYPAD) {
        bytes.extend_from_slice(b"\x1b>");
    }
    if mode.intersects(TermMode::MOUSE_MODE) {
        bytes.extend_from_slice(b"\x1b[?1000l\x1b[?1002l\x1b[?1003l");
    }
    if mode.contains(TermMode::FOCUS_IN_OUT) {
        bytes.extend_from_slice(b"\x1b[?1004l");
    }
    if mode.contains(TermMode::UTF8_MOUSE) {
        bytes.extend_from_slice(b"\x1b[?1005l");
    }
    if mode.contains(TermMode::SGR_MOUSE) {
        bytes.extend_from_slice(b"\x1b[?1006l");
    }
    if mode.contains(TermMode::ALT_SCREEN) {
        bytes.extend_from_slice(b"\x1b[?1049l");
    }
    if !mode.contains(TermMode::SHOW_CURSOR) {
        bytes.extend_from_slice(b"\x1b[?25h");
    }

    bytes
}

pub(super) fn current_cwd_for_pid(pid: u32) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }

    #[cfg(target_os = "macos")]
    {
        current_cwd_for_pid_via_lsof(pid)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

#[cfg(target_os = "macos")]
fn current_cwd_for_pid_via_lsof(pid: u32) -> Option<PathBuf> {
    let target_pid = deepest_child_pid(pid).unwrap_or(pid);
    lsof_cwd_for_pid(target_pid).or_else(|| lsof_cwd_for_pid(pid))
}

#[cfg(target_os = "macos")]
fn deepest_child_pid(mut pid: u32) -> Option<u32> {
    let mut saw_child = false;

    while let Some(child_pid) = direct_child_pid(pid) {
        saw_child = true;
        pid = child_pid;
    }

    saw_child.then_some(pid)
}

#[cfg(target_os = "macos")]
fn lsof_cwd_for_pid(pid: u32) -> Option<PathBuf> {
    let output = std::process::Command::new("lsof")
        .args(["-a", "-d", "cwd", "-p", &pid.to_string(), "-Fn"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    parse_lsof_cwd(std::str::from_utf8(&output.stdout).ok()?)
}

#[cfg(any(target_os = "macos", test))]
fn parse_lsof_cwd(output: &str) -> Option<PathBuf> {
    let mut in_cwd_entry = false;

    for line in output.lines() {
        if let Some(descriptor) = line.strip_prefix('f') {
            in_cwd_entry = descriptor == "cwd";
            continue;
        }
        if in_cwd_entry && let Some(path) = line.strip_prefix('n') {
            return Some(PathBuf::from(path));
        }
    }

    None
}

#[cfg(target_os = "macos")]
fn direct_child_pid(pid: u32) -> Option<u32> {
    let output = std::process::Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    parse_pgrep_pid(std::str::from_utf8(&output.stdout).ok()?)
}

#[cfg(any(target_os = "macos", test))]
fn parse_pgrep_pid(output: &str) -> Option<u32> {
    output.lines().find_map(|line| line.trim().parse().ok())
}

const TERMINAL_BASE_COLORS: [Rgb; 16] = [
    rgb(0x1d, 0x1f, 0x21),
    rgb(0xcc, 0x66, 0x66),
    rgb(0xb5, 0xbd, 0x68),
    rgb(0xf0, 0xc6, 0x74),
    rgb(0x81, 0xa2, 0xbe),
    rgb(0xb2, 0x94, 0xbb),
    rgb(0x8a, 0xbe, 0xb7),
    rgb(0xc5, 0xc8, 0xc6),
    rgb(0x66, 0x66, 0x66),
    rgb(0xd5, 0x4e, 0x53),
    rgb(0xb9, 0xca, 0x4a),
    rgb(0xe7, 0xc5, 0x47),
    rgb(0x7a, 0xa6, 0xda),
    rgb(0xc3, 0x97, 0xd8),
    rgb(0x70, 0xc0, 0xb1),
    rgb(0xea, 0xea, 0xea),
];

const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

const URL_SCHEMES: [&str; 3] = ["https://", "http://", "file://"];

pub(super) fn find_url_at_column(chars: &[char], col: usize) -> Option<String> {
    for scheme in URL_SCHEMES {
        let scheme_chars: Vec<char> = scheme.chars().collect();
        let scheme_len = scheme_chars.len();
        if chars.len() < scheme_len {
            continue;
        }
        for start in 0..=chars.len() - scheme_len {
            if chars[start..start + scheme_len] != *scheme_chars {
                continue;
            }
            let end = url_end_column(chars, start);
            if col >= start && col < end {
                return Some(chars[start..end].iter().collect());
            }
        }
    }
    None
}

fn url_end_column(chars: &[char], start: usize) -> usize {
    let mut end = chars.len();
    for (index, character) in chars.iter().enumerate().skip(start) {
        if character.is_whitespace() || matches!(character, '<' | '>' | '"' | '\'') {
            end = index;
            break;
        }
    }
    strip_trailing_url_chars(chars, start, end)
}

fn strip_trailing_url_chars(chars: &[char], start: usize, mut end: usize) -> usize {
    while end > start && matches!(chars[end - 1], '.' | ',' | ';' | '!' | '?') {
        end -= 1;
    }

    for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
        while end > start && chars[end - 1] == close && unmatched_closing_delimiter(chars, start, end, open, close) {
            end -= 1;
        }
    }

    end
}

fn unmatched_closing_delimiter(chars: &[char], start: usize, end: usize, open: char, close: char) -> bool {
    let mut balance = 0usize;
    for character in &chars[start..end] {
        if *character == open {
            balance += 1;
        } else if *character == close {
            if balance == 0 {
                return true;
            }
            balance -= 1;
        }
    }
    false
}

pub(super) fn find_file_path_at_column(chars: &[char], col: usize) -> Option<String> {
    let mut index = 0;
    while index < chars.len() {
        let is_path_start = (chars[index] == '/'
            || (chars[index] == '~' && index + 1 < chars.len() && chars[index + 1] == '/'))
            && (index == 0 || is_path_boundary(chars[index - 1]));
        if !is_path_start {
            index += 1;
            continue;
        }
        let start = index;
        let end = path_end_column(chars, start);
        let path = strip_line_col_suffix_chars(&chars[start..end]);
        if path.len() > 1 && col >= start && col < start + path.len() {
            return Some(path.iter().collect());
        }
        index = end;
    }
    None
}

fn is_path_boundary(character: char) -> bool {
    character.is_whitespace() || matches!(character, '"' | '\'' | '(' | '[' | '{' | '<' | '=' | ':')
}

fn path_end_column(chars: &[char], start: usize) -> usize {
    let mut end = chars.len();
    for (index, character) in chars.iter().enumerate().skip(start) {
        if character.is_whitespace() || matches!(character, '<' | '>' | '"' | '\'' | ')' | ']' | '}') {
            end = index;
            break;
        }
    }
    while end > start && matches!(chars[end - 1], '.' | ',' | ';' | '!' | '?') {
        end -= 1;
    }
    end
}

fn strip_line_col_suffix_chars(chars: &[char]) -> &[char] {
    let mut result = chars;
    loop {
        let Some(colon_pos) = result.iter().rposition(|character| *character == ':') else {
            return result;
        };
        let suffix = &result[colon_pos + 1..];
        if !suffix.is_empty() && suffix.iter().all(char::is_ascii_digit) {
            result = &result[..colon_pos];
        } else {
            return result;
        }
    }
}

/// Open a URL or file path with the platform's default handler.
pub fn open_url(url: &str) {
    let command = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let result = std::process::Command::new(command)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(error) = result {
        tracing::warn!("failed to open URL {url}: {error}");
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::parse_lsof_cwd;

    #[test]
    fn parse_lsof_cwd_extracts_working_directory() {
        let output = "p1234\nfcwd\nn/tmp/project\n";

        assert_eq!(parse_lsof_cwd(output), Some(PathBuf::from("/tmp/project")));
    }

    #[test]
    fn parse_lsof_cwd_returns_none_without_cwd_entry() {
        let output = "p1234\nf1\nn/tmp/project/file.txt\n";

        assert_eq!(parse_lsof_cwd(output), None);
    }

    #[test]
    fn parse_pgrep_pid_extracts_first_child_pid() {
        let output = "12027\n12099\n";

        assert_eq!(super::parse_pgrep_pid(output), Some(12027));
    }
}
