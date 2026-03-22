mod content;
mod events;
mod lifecycle;
mod replay;
mod resize;
mod selection;
mod support;

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg, State};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{self, RenderableContent, Term, TermDamage, TermMode, viewport_to_point};
use alacritty_terminal::tty::{self, Options as PtyOptions, Shell};
use alacritty_terminal::vte::ansi::Rgb;

use crate::error::{Error, Result};

use self::replay::{ReplayRestoreState, drain_replay_events};
#[cfg(test)]
use self::resize::{queue_debounced_pty_resize, should_debounce_pty_resize};
#[cfg(test)]
use self::support::default_terminal_rgb;
pub use self::support::open_url;
use self::support::{
    ColorLookup, current_cwd_for_pid, find_file_path_at_column, find_url_at_column, replay_terminal_bytes,
};

#[cfg(not(windows))]
type TerminalPty = tty::Pty;
#[cfg(windows)]
type TerminalPty = tty::Pty;

type TerminalEventLoop = EventLoop<TerminalPty, TerminalEventProxy>;
type TerminalEventLoopState = State;

pub struct TerminalSpawnOptions {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub rows: u16,
    pub cols: u16,
    pub cell_width: u16,
    pub cell_height: u16,
    pub scrollback_limit: usize,
    pub window_id: u64,
    pub replay_bytes: Vec<u8>,
    pub env: HashMap<String, String>,
    pub kitty_keyboard: bool,
}

/// A structured notification parsed from an OSC title sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentNotification {
    pub severity: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HorizonOscTitle {
    Notification(AgentNotification),
    SetTitle(String),
    ClearTitle,
    Ignore,
}

#[derive(Clone)]
struct TerminalEventProxy {
    event_tx: mpsc::Sender<Event>,
}

impl EventListener for TerminalEventProxy {
    fn send_event(&self, event: Event) {
        let _ = self.event_tx.send(event);
    }
}

#[derive(Clone, Copy)]
struct TerminalDimensions {
    rows: usize,
    cols: usize,
}

impl TerminalDimensions {
    fn new(rows: u16, cols: u16) -> Self {
        Self {
            rows: usize::from(rows.max(1)),
            cols: usize::from(cols.max(2)),
        }
    }
}

impl Dimensions for TerminalDimensions {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct Terminal {
    term: Arc<FairMutex<Term<TerminalEventProxy>>>,
    event_sender: EventLoopSender,
    event_rx: mpsc::Receiver<Event>,
    event_loop_handle: Option<JoinHandle<(TerminalEventLoop, TerminalEventLoopState)>>,
    child_pid: Option<u32>,
    rows: u16,
    cols: u16,
    cell_width: u16,
    cell_height: u16,
    scrollback_limit: usize,
    title: String,
    clipboard_contents: String,
    selection_contents: String,
    pending_pty_resize: Option<std::time::Instant>,
    pty_resized: bool,
    child_exited: bool,
    bell_pending: bool,
    pending_notification: Option<AgentNotification>,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use std::collections::HashMap;

    #[cfg(target_os = "linux")]
    use super::current_cwd_for_pid;
    use super::{
        AgentNotification, HorizonOscTitle, Terminal, TerminalDimensions, TerminalEventProxy, TerminalSpawnOptions,
        default_terminal_rgb, find_file_path_at_column, find_url_at_column, queue_debounced_pty_resize,
        replay_terminal_bytes, should_debounce_pty_resize,
    };
    use alacritty_terminal::event::Event;
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::selection::SelectionType;
    use alacritty_terminal::sync::FairMutex;
    use alacritty_terminal::term::{self, Term, TermMode};
    use std::sync::Arc;
    use std::sync::mpsc;

    #[test]
    fn terminal_dimensions_clamp_to_supported_minimums() {
        let dimensions = TerminalDimensions::new(0, 1);

        assert_eq!(dimensions.screen_lines(), 1);
        assert_eq!(dimensions.columns(), 2);
        assert_eq!(dimensions.total_lines(), 1);
    }

    #[test]
    fn indexed_color_cube_matches_xterm_steps() {
        let color = default_terminal_rgb(21);

        assert_eq!((color.r, color.g, color.b), (0, 0, 255));
    }

    #[test]
    fn shutdown_with_timeout_waits_for_pty_exit() {
        let mut terminal = Terminal::spawn(TerminalSpawnOptions {
            program: std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()),
            args: Vec::new(),
            cwd: None,
            rows: 24,
            cols: 80,
            cell_width: 8,
            cell_height: 16,
            scrollback_limit: 256,
            window_id: 41,
            replay_bytes: Vec::new(),
            env: HashMap::new(),
            kitty_keyboard: true,
        })
        .expect("terminal should spawn");

        assert!(terminal.shutdown_with_timeout(Duration::from_secs(2)));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn current_cwd_for_pid_reads_procfs_cwd() {
        let cwd = current_cwd_for_pid(std::process::id()).expect("cwd");
        assert_eq!(cwd, std::env::current_dir().expect("current dir"));
    }

    #[test]
    fn replay_clears_stale_fullscreen_modes_from_transcripts() {
        let term = test_term();
        replay_terminal_bytes(
            &term,
            b"\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[?1004h\x1b=\x1b[?1h\x1b[?25l",
        );

        let mode = *term.lock().mode();
        assert_eq!(mode, TermMode::default());
    }

    #[test]
    fn parse_horizon_notify_title() {
        assert_eq!(
            Terminal::parse_horizon_title("HORIZON_NOTIFY:attention:Need review"),
            Some(HorizonOscTitle::Notification(AgentNotification {
                severity: "attention".to_string(),
                message: "Need review".to_string(),
            })),
        );
    }

    #[test]
    fn parse_horizon_title_set_command() {
        assert_eq!(
            Terminal::parse_horizon_title("HORIZON_TITLE:set:Fix issue #42"),
            Some(HorizonOscTitle::SetTitle("Fix issue #42".to_string())),
        );
    }

    #[test]
    fn parse_horizon_title_clear_command() {
        assert_eq!(
            Terminal::parse_horizon_title("HORIZON_TITLE:clear"),
            Some(HorizonOscTitle::ClearTitle),
        );
    }

    #[test]
    fn parse_invalid_horizon_title_command_is_ignored() {
        assert_eq!(
            Terminal::parse_horizon_title("HORIZON_TITLE:rename:Fix issue #42"),
            Some(HorizonOscTitle::Ignore),
        );
    }

    #[test]
    fn parse_notify_without_message_separator_is_ignored() {
        assert_eq!(
            Terminal::parse_horizon_title("HORIZON_NOTIFY:attention"),
            Some(HorizonOscTitle::Ignore),
        );
    }

    #[test]
    fn parse_notify_without_severity_is_ignored() {
        assert_eq!(
            Terminal::parse_horizon_title("HORIZON_NOTIFY::Saved"),
            Some(HorizonOscTitle::Ignore),
        );
    }

    fn spawn_test_terminal() -> Terminal {
        Terminal::spawn(TerminalSpawnOptions {
            // These tests only exercise local event handling, so use a
            // short-lived child process instead of an interactive shell.
            // That keeps PTY teardown deterministic on macOS runners.
            program: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
            args: vec!["-c".to_string(), "exit".to_string()],
            cwd: None,
            rows: 24,
            cols: 80,
            cell_width: 8,
            cell_height: 16,
            scrollback_limit: 256,
            window_id: 41,
            replay_bytes: Vec::new(),
            env: HashMap::new(),
            kitty_keyboard: true,
        })
        .expect("terminal should spawn")
    }

    #[test]
    fn horizon_notify_event_sets_notification_without_overwriting_title() {
        let mut terminal = spawn_test_terminal();
        terminal.title = "Existing title".to_string();

        terminal.handle_event(Event::Title("HORIZON_NOTIFY:info:Saved".to_string()));

        assert_eq!(terminal.title(), "Existing title");
        assert_eq!(
            terminal.take_notification(),
            Some(AgentNotification {
                severity: "info".to_string(),
                message: "Saved".to_string(),
            })
        );
        assert!(terminal.shutdown_with_timeout(Duration::from_secs(2)));
    }

    #[test]
    fn horizon_title_set_event_updates_title_without_notification() {
        let mut terminal = spawn_test_terminal();

        terminal.handle_event(Event::Title("HORIZON_TITLE:set:Build running".to_string()));

        assert_eq!(terminal.title(), "Build running");
        assert_eq!(terminal.take_notification(), None);
        assert!(terminal.shutdown_with_timeout(Duration::from_secs(2)));
    }

    #[test]
    fn horizon_title_clear_event_clears_existing_title() {
        let mut terminal = spawn_test_terminal();
        terminal.title = "Build running".to_string();

        terminal.handle_event(Event::Title("HORIZON_TITLE:clear".to_string()));

        assert!(terminal.title().is_empty());
        assert_eq!(terminal.take_notification(), None);
        assert!(terminal.shutdown_with_timeout(Duration::from_secs(2)));
    }

    #[test]
    fn invalid_horizon_title_event_does_not_clobber_existing_title() {
        let mut terminal = spawn_test_terminal();
        terminal.title = "Build running".to_string();

        terminal.handle_event(Event::Title("HORIZON_TITLE:rename:other".to_string()));

        assert_eq!(terminal.title(), "Build running");
        assert_eq!(terminal.take_notification(), None);
        assert!(terminal.shutdown_with_timeout(Duration::from_secs(2)));
    }

    #[test]
    fn malformed_horizon_notify_event_does_not_leak_into_visible_title() {
        let mut terminal = spawn_test_terminal();
        terminal.title = "Existing title".to_string();

        terminal.handle_event(Event::Title("HORIZON_NOTIFY:attention".to_string()));

        assert_eq!(terminal.title(), "Existing title");
        assert_eq!(terminal.take_notification(), None);
        assert!(terminal.shutdown_with_timeout(Duration::from_secs(2)));
    }

    #[test]
    fn ordinary_terminal_title_replaces_horizon_title() {
        let mut terminal = spawn_test_terminal();

        terminal.handle_event(Event::Title("HORIZON_TITLE:set:Build running".to_string()));
        terminal.handle_event(Event::Title("cargo test".to_string()));

        assert_eq!(terminal.title(), "cargo test");
        assert_eq!(terminal.take_notification(), None);
        assert!(terminal.shutdown_with_timeout(Duration::from_secs(2)));
    }

    #[test]
    fn full_text_lines_preserve_unicode_matrix() {
        let mut terminal = spawn_test_terminal();

        replay_terminal_bytes(
            &terminal.term,
            "ascii æøå åäö\r\nmixed 你 e\u{0301} ✈\u{fe0f}\r\n".as_bytes(),
        );

        let (lines, total_lines) = terminal.full_text_lines(terminal.rows().into());

        assert_eq!(total_lines, usize::from(terminal.rows()));
        assert_eq!(lines, vec!["ascii æøå åäö", "mixed 你 e\u{0301} ✈\u{fe0f}"]);
        assert!(terminal.shutdown_with_timeout(Duration::from_secs(2)));
    }

    #[test]
    fn line_selection_preserves_unicode_content() {
        let mut terminal = spawn_test_terminal();

        replay_terminal_bytes(&terminal.term, "æøå åäö 你 e\u{0301} ✈\u{fe0f}".as_bytes());
        terminal.start_selection(SelectionType::Lines, 0, 0);

        assert_eq!(
            terminal.selection_to_string(),
            Some("æøå åäö 你 e\u{0301} ✈\u{fe0f}\n".to_string())
        );
        assert!(terminal.shutdown_with_timeout(Duration::from_secs(2)));
    }

    #[test]
    fn horizon_title_replaces_ordinary_terminal_title() {
        let mut terminal = spawn_test_terminal();

        terminal.handle_event(Event::Title("vim src/main.rs".to_string()));
        terminal.handle_event(Event::Title("HORIZON_TITLE:set:Build running".to_string()));

        assert_eq!(terminal.title(), "Build running");
        assert_eq!(terminal.take_notification(), None);
        assert!(terminal.shutdown_with_timeout(Duration::from_secs(2)));
    }

    fn test_term() -> Arc<FairMutex<Term<TerminalEventProxy>>> {
        let (event_tx, _event_rx) = mpsc::channel();
        let dimensions = TerminalDimensions::new(24, 80);
        let config = term::Config {
            scrolling_history: 256,
            kitty_keyboard: true,
            ..term::Config::default()
        };

        Arc::new(FairMutex::new(Term::new(
            config,
            &dimensions,
            TerminalEventProxy { event_tx },
        )))
    }

    #[test]
    fn queue_debounced_pty_resize_preserves_first_request_time() {
        let first = std::time::Instant::now();
        let second = first + Duration::from_millis(40);
        let mut pending = None;

        queue_debounced_pty_resize(&mut pending, first);
        queue_debounced_pty_resize(&mut pending, second);

        assert_eq!(pending, Some(first));
    }

    #[test]
    fn should_debounce_pty_resize_skips_alt_screen() {
        assert!(should_debounce_pty_resize(true, TermMode::empty()));
        assert!(!should_debounce_pty_resize(true, TermMode::ALT_SCREEN));
        assert!(!should_debounce_pty_resize(false, TermMode::empty()));
    }

    #[test]
    fn url_detected_at_clicked_column() {
        let line: Vec<char> = "Created: https://github.com/foo/bar/issues/123".chars().collect();
        assert_eq!(
            find_url_at_column(&line, 10),
            Some("https://github.com/foo/bar/issues/123".to_string()),
        );
    }

    #[test]
    fn click_outside_url_returns_none() {
        let line: Vec<char> = "Created: https://github.com/foo".chars().collect();
        assert_eq!(find_url_at_column(&line, 0), None);
    }

    #[test]
    fn trailing_punctuation_stripped_from_url() {
        let line: Vec<char> = "See https://example.com.".chars().collect();
        assert_eq!(find_url_at_column(&line, 5), Some("https://example.com".to_string()),);
    }

    #[test]
    fn http_and_file_schemes_detected() {
        let line: Vec<char> = "open http://localhost:3000/api".chars().collect();
        assert_eq!(
            find_url_at_column(&line, 6),
            Some("http://localhost:3000/api".to_string()),
        );

        let line: Vec<char> = "file:///home/user/doc.pdf rest".chars().collect();
        assert_eq!(
            find_url_at_column(&line, 0),
            Some("file:///home/user/doc.pdf".to_string()),
        );
    }

    #[test]
    fn markdown_link_target_strips_unmatched_closing_parenthesis() {
        let line: Vec<char> = "[YouPark.no](https://youpark.no)".chars().collect();
        assert_eq!(find_url_at_column(&line, 15), Some("https://youpark.no".to_string()),);
    }

    #[test]
    fn balanced_parentheses_preserved_in_url() {
        let line: Vec<char> = "See https://example.com/a(b)".chars().collect();
        assert_eq!(
            find_url_at_column(&line, 10),
            Some("https://example.com/a(b)".to_string()),
        );
    }

    #[test]
    fn absolute_file_path_detected() {
        let line: Vec<char> = "error at /home/user/project/src/main.rs rest".chars().collect();
        assert_eq!(
            find_file_path_at_column(&line, 10),
            Some("/home/user/project/src/main.rs".to_string()),
        );
    }

    #[test]
    fn home_relative_path_detected() {
        let line: Vec<char> = "see ~/project/config.yaml for details".chars().collect();
        assert_eq!(
            find_file_path_at_column(&line, 5),
            Some("~/project/config.yaml".to_string()),
        );
    }

    #[test]
    fn line_col_suffix_stripped_from_path() {
        let line: Vec<char> = "error: /src/lib.rs:42:15 something".chars().collect();
        assert_eq!(find_file_path_at_column(&line, 8), Some("/src/lib.rs".to_string()),);
    }

    #[test]
    fn click_outside_path_returns_none() {
        let line: Vec<char> = "error at /home/user/file.rs".chars().collect();
        assert_eq!(find_file_path_at_column(&line, 0), None);
    }

    #[test]
    fn bare_slash_not_detected_as_path() {
        let line: Vec<char> = "a / b".chars().collect();
        assert_eq!(find_file_path_at_column(&line, 2), None);
    }

    #[test]
    fn url_takes_priority_over_path() {
        let line: Vec<char> = "see file:///home/user/doc.pdf".chars().collect();
        // find_url_at_column should match file:// scheme first
        assert!(find_url_at_column(&line, 5).is_some());
    }
}
