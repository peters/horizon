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

/// Open a URL or file path with the platform's default handler.
///
/// A thread starts the handler, so the frame that opens a link waits for no
/// `fork` and no `exec`. A handler that cannot start is a warning in the log.
///
/// # Errors
///
/// Returns the error of the thread that could not start.
pub fn open_url(url: &str) -> std::io::Result<()> {
    // The start of the handler is a `fork` and an `exec`, which a slow disk can
    // stretch, so a small thread starts it and the frame returns at once.
    let (program, args) = url_opener_invocation(url);
    let url = url.to_owned();
    std::thread::Builder::new()
        .name("open-url".to_owned())
        .spawn(move || {
            crate::ui_thread::blocking("a URL opens in a program of the desktop");
            if let Err(error) = std::process::Command::new(program)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                tracing::warn!(%error, %url, "could not open a URL");
            }
        })
        .map(|_| ())
}

fn url_opener_invocation(url: &str) -> (&'static str, Vec<String>) {
    if cfg!(target_os = "macos") {
        ("open", vec![url.to_owned()])
    } else {
        ("xdg-open", vec![url.to_owned()])
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{open_url, url_opener_invocation};

    /// The bound of the wait of a test, so a stall fails instead of hanging.
    const HANG_GUARD: Duration = Duration::from_secs(10);

    #[test]
    fn url_opener_uses_the_platform_default_handler() {
        let (program, args) = url_opener_invocation("https://example.com");
        if cfg!(target_os = "macos") {
            assert_eq!(program, "open");
            assert_eq!(args, ["https://example.com"]);
        } else {
            assert_eq!(program, "xdg-open");
            assert_eq!(args, ["https://example.com"]);
        }
    }

    #[test]
    fn a_url_opens_on_a_thread_of_its_own() {
        // The start of the handler is a call that can wait of the guide, so the
        // guard of the UI thread is the separator: a frame that ran the program
        // reports that call here and the test fails. A worker is never inside a
        // frame, so the thread that Horizon starts makes the same call without a
        // report, however long the program takes to start.
        crate::ui_thread::use_strict_mode();
        let started = Instant::now();
        let frame = crate::ui_thread::FrameScope::enter();
        let opened = open_url("https://example.invalid/");
        drop(frame);
        assert!(opened.is_ok(), "a URL of the desktop starts its handler: {opened:?}");
        assert!(
            started.elapsed() < HANG_GUARD,
            "the thread that opens a URL starts without a stall"
        );
    }
}
