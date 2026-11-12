//! The workspace list: the attention badge of each workspace of the sidebar and
//! its one status line. The UI reads the same rows.
use crate::attention::Attention;
use crate::{Panel, PanelId};

/// How many agent panels of a workspace wait for the person.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Waiting {
    /// The panels that ask a question or failed a turn.
    pub needs_you: usize,
    /// The panels that finished while the person looked elsewhere.
    pub done: usize,
}

impl Waiting {
    /// The count at the front of the status line, or `None` when no panel waits.
    #[must_use]
    pub fn line(self) -> Option<String> {
        let mut parts = Vec::new();
        if self.needs_you > 0 {
            parts.push(format!(
                "{} {}",
                self.needs_you,
                if self.needs_you == 1 { "needs you" } else { "need you" }
            ));
        }
        if self.done > 0 {
            parts.push(format!("{} done", self.done));
        }
        (!parts.is_empty()).then(|| parts.join(" \u{00B7} "))
    }
}

/// One row of the workspace list.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// The heaviest attention state of the panels of the workspace.
    pub attention: Attention,
    /// How many of its agent panels wait for the person.
    pub waiting: Waiting,
    pub line: String,
}

impl Row {
    /// The row of a workspace that runs on this computer.
    #[must_use]
    pub fn local(attention: Attention, line: Option<&str>) -> Self {
        Self::placed(attention, Waiting::default(), line, None)
    }

    /// The row of a workspace that runs on this computer or on a machine.
    ///
    /// Its second line starts with the count of the agents that wait for the
    /// person, then the place, because a terminal of a workspace on a machine
    /// runs there and not here. The rest is the one line that the status text
    /// underscores; an empty status text leaves the place alone on it.
    #[must_use]
    pub fn placed(attention: Attention, waiting: Waiting, line: Option<&str>, machine: Option<&str>) -> Self {
        Self {
            attention,
            waiting,
            line: second_line(waiting, line.unwrap_or_default(), machine),
        }
    }
}

/// What the second line of a row says: the agents that wait, `on <machine>` and
/// the status text of a workspace on a machine, or the status text alone on this
/// computer.
///
/// The count and the place come first, so a long name cannot cut them off the row.
#[must_use]
fn second_line(waiting: Waiting, line: &str, machine: Option<&str>) -> String {
    let place = match machine {
        Some(machine) if line.is_empty() => format!("on {machine}"),
        Some(machine) => format!("on {machine} \u{00B7} {line}"),
        None => line.to_owned(),
    };
    match (waiting.line(), place.is_empty()) {
        (Some(count), false) => format!("{count} \u{00B7} {place}"),
        (Some(count), true) => count,
        (None, _) => place,
    }
}

/// Footer hints that agent TUIs pin below their input; they say nothing about the work.
const FOOTER_HINTS: [&str; 3] = ["for shortcuts", "shift+tab to cycle", "ctrl+t to"];
/// The longest status line, in characters.
const MAX_LINE_CHARS: usize = 120;

/// Frame and bullet characters around the text of a terminal line.
fn is_frame(character: char) -> bool {
    character.is_whitespace()
        || matches!(character,
            '\u{2500}'..='\u{259F}' // box drawing and block elements
            | '\u{25A0}'..='\u{25FF}' // geometric shapes
            | '\u{2700}'..='\u{27BF}' // dingbats, `❯` among them
            | '\u{23F5}'..='\u{23FA}' // media symbols that agents use as bullets
            | '|' | '•' | '·')
}

/// Prompt and quote characters before the text of a terminal line.
fn is_prompt(character: char) -> bool {
    matches!(character, '>' | '$' | '#' | '%' | '*' | '›' | '»')
}

/// A shell prompt with nothing typed after it, such as `user@host:~/repo$` or the
/// `host%` of zsh. A percentage at the end of a line, such as `50%`, is not one.
fn is_empty_prompt(line: &str) -> bool {
    let line = line.trim_end_matches(is_frame);
    let mut ending = line.chars().rev();
    match ending.next() {
        Some('$' | '#' | '>' | '❯') => true,
        Some('%') => !ending.next().is_some_and(|before| before.is_ascii_digit()),
        _ => false,
    }
}

/// The current window in the window list of tmux, such as `0:bash*`, also when a
/// long session name runs into it, as in `[c8dfe3af-0:bash*`.
fn is_tmux_current_window(word: &str) -> bool {
    word.split_once(':').is_some_and(|(before, window)| {
        before.ends_with(|character: char| character.is_ascii_digit()) && window.len() > 1 && window.ends_with('*')
    })
}

/// The status bar that tmux draws at the bottom of a session by default, such as
/// `[c8dfe3af-0:bash*  "host" 06:39 10-Oct-26`: it starts with the session, has
/// the current window and ends with the clock and the date.
fn is_tmux_status_bar(line: &str) -> bool {
    let mut words = line.split_whitespace().rev();
    let (Some(date), Some(clock)) = (words.next(), words.next()) else {
        return false;
    };
    let date: Vec<&str> = date.split('-').collect();
    let digits = |text: &str, count: usize| text.len() == count && text.bytes().all(|byte| byte.is_ascii_digit());
    let clock = clock
        .split_once(':')
        .is_some_and(|(hours, minutes)| digits(hours, 2) && digits(minutes, 2));
    let date = matches!(date.as_slice(), [day, month, year]
        if digits(day, 2) && month.len() == 3 && month.bytes().all(|byte| byte.is_ascii_alphabetic()) && digits(year, 2));
    line.trim_start().starts_with('[') && clock && date && line.split_whitespace().any(is_tmux_current_window)
}

/// The status line of a terminal: its last line, from `lines` oldest first, that has
/// words in it once frames, prompts and bullets are taken away. Footer hints, an
/// empty shell prompt and the status bar of tmux are not status lines.
#[must_use]
pub fn status_line<'a>(lines: impl IntoIterator<Item = &'a str>) -> Option<String> {
    wrapped_status_line(lines.into_iter().map(|line| (line, line.chars().count())), None)
}

/// Rows above the last one that a wrapped status line takes, at most.
const MAX_WRAPPED_ROWS: usize = 3;

/// As [`status_line`], for the rows of a screen `columns` wide, each with the number
/// of columns that its text occupies, blank rows included: a line that fills the
/// width of the screen continues on the next row, so the rows of a wrapped line are
/// joined. A program in tmux wraps its own lines, so the terminal cannot mark them.
#[must_use]
pub fn wrapped_status_line<'a>(
    rows: impl IntoIterator<Item = (&'a str, usize)>,
    columns: Option<usize>,
) -> Option<String> {
    let rows: Vec<(&str, usize)> = rows.into_iter().collect();
    let full = |width: usize| columns.is_some_and(|columns| columns > 1 && width + 1 >= columns);
    (0..rows.len()).rev().find_map(|last| {
        let line = rows[last].0;
        if is_empty_prompt(line) || is_tmux_status_bar(line) {
            return None;
        }
        let mut first = last;
        while first > 0 && last - first < MAX_WRAPPED_ROWS && full(rows[first - 1].1) {
            first -= 1;
        }
        // A row one short of the width lost the space at its end when it was trimmed.
        let joined = rows[first..=last]
            .iter()
            .enumerate()
            .map(|(row, (line, width))| {
                let space = row + first < last && columns.is_some_and(|columns| width + 1 == columns);
                if space { format!("{line} ") } else { (*line).to_owned() }
            })
            .collect::<String>();
        let text = joined
            .trim_start_matches(|character| is_frame(character) || is_prompt(character))
            .trim_end_matches(is_frame);
        let words = text.chars().filter(|character| character.is_alphanumeric()).count();
        let hint = FOOTER_HINTS.iter().any(|hint| text.contains(hint));
        (words >= 3 && !hint).then(|| crate::truncate_chars(text, MAX_LINE_CHARS).into_owned())
    })
}

/// The panel whose terminal speaks for a workspace: its first agent, else its
/// focused terminal, else its first terminal.
#[must_use]
pub fn primary_panel<'a>(
    panels: impl Iterator<Item = &'a Panel> + Clone,
    focused: Option<PanelId>,
) -> Option<&'a Panel> {
    let mut terminals = panels.filter(|panel| panel.terminal().is_some());
    terminals
        .clone()
        .find(|panel| panel.kind.is_agent())
        .or_else(|| terminals.clone().find(|panel| Some(panel.id) == focused))
        .or_else(|| terminals.next())
}

/// The status line of the terminal of `panel`, read from the bottom of its screen.
#[must_use]
pub fn panel_line(panel: &Panel) -> Option<String> {
    let terminal = panel.terminal()?;
    // The whole screen: output at the top of a tall screen leaves its bottom rows empty.
    let rows = terminal.screen_rows();
    wrapped_status_line(
        rows.iter().map(|(text, width)| (text.as_str(), *width)),
        Some(usize::from(terminal.cols())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_of_a_workspace_on_a_machine_starts_with_the_place() {
        let none = Waiting::default();
        assert_eq!(
            Row::placed(Attention::Working, none, Some("Edit 2 files (+41 -7)"), None).line,
            "Edit 2 files (+41 -7)"
        );
        assert_eq!(
            Row::placed(Attention::Working, none, Some("Edit 2 files (+41 -7)"), Some("build-1")).line,
            "on build-1 \u{00B7} Edit 2 files (+41 -7)"
        );
        assert_eq!(
            Row::placed(Attention::Idle, none, None, Some("build-1")).line,
            "on build-1",
            "a row without a status text still says where the workspace lives"
        );
        assert_eq!(Row::placed(Attention::Idle, none, None, None).line, "");
        assert_eq!(Row::local(Attention::Working, Some("a line")).line, "a line");
    }

    #[test]
    fn a_row_that_waits_starts_its_line_with_the_count_of_the_agents() {
        assert_eq!(Waiting { needs_you: 2, done: 0 }.line().as_deref(), Some("2 need you"));
        assert_eq!(
            Waiting { needs_you: 1, done: 0 }.line().as_deref(),
            Some("1 needs you"),
            "one agent needs the person, so its count takes the singular verb"
        );
        assert_eq!(Waiting { needs_you: 0, done: 1 }.line().as_deref(), Some("1 done"));
        assert_eq!(Waiting { needs_you: 0, done: 2 }.line().as_deref(), Some("2 done"));
        assert_eq!(
            Waiting { needs_you: 1, done: 2 }.line().as_deref(),
            Some("1 needs you \u{00B7} 2 done")
        );
        assert_eq!(Waiting::default().line(), None);
        let waiting = Waiting { needs_you: 2, done: 0 };
        assert_eq!(
            Row::placed(Attention::NeedsYou, waiting, Some("Editing"), None).line,
            "2 need you \u{00B7} Editing"
        );
        assert_eq!(
            Row::placed(Attention::Done, waiting, None, Some("build-1")).line,
            "2 need you \u{00B7} on build-1",
            "a count is never cut off, also when the workspace says where it lives"
        );
    }

    #[test]
    fn the_status_line_skips_frames_prompts_and_footer_hints() {
        let screen = [
            "● Edit 2 files (+41 -7)",
            "╭──────────────────────╮",
            "│ >                    │",
            "╰──────────────────────╯",
            "  ? for shortcuts",
        ];
        assert_eq!(
            status_line(screen.iter().copied()).as_deref(),
            Some("Edit 2 files (+41 -7)")
        );
        assert_eq!(
            status_line(["$ cargo test -p core", "$ "]).as_deref(),
            Some("cargo test -p core")
        );
        assert_eq!(status_line(["", "──", "> a"]), None);
        assert_eq!(
            status_line(["Finished dev profile", "user@host:~/repo$"]).as_deref(),
            Some("Finished dev profile")
        );
        assert_eq!(
            status_line(["synthetic output 12", "[c8dfe3af-0:bash*   \"example\" 06:39 10-Oct-26"]).as_deref(),
            Some("synthetic output 12"),
            "the tmux status bar of a remote session is not a status line"
        );
        for log in [
            "[INFO] build done at 06:39 on 10-Oct-26",
            "[INFO] build done 06:39 10-Oct-26",
        ] {
            assert_eq!(
                status_line([log]).as_deref(),
                Some(log),
                "a log line with a clock is a status line"
            );
        }
        assert_eq!(status_line(["% cargo test", "host%"]).as_deref(), Some("cargo test"));
        assert_eq!(
            status_line(["Downloading layers 50%"]).as_deref(),
            Some("Downloading layers 50%")
        );
    }

    fn wrapped(rows: &[&str], columns: usize) -> Option<String> {
        wrapped_status_line(rows.iter().map(|row| (*row, row.chars().count())), Some(columns))
    }

    #[test]
    fn a_line_that_fills_the_screen_continues_on_the_next_row() {
        let screen = [
            "synthetic output 11",
            "Session process exited with status 0. Reconnect",
            "preserves this result.",
        ];
        // The first row ended with a space at the edge of a screen 48 columns wide.
        assert_eq!(
            wrapped(&screen, 48).as_deref(),
            Some("Session process exited with status 0. Reconnect preserves this result.")
        );
        // A row that fills the whole width broke inside a word.
        assert_eq!(
            wrapped(&["Downloading the wor", "ker image"], 19).as_deref(),
            Some("Downloading the worker image")
        );
        // Short rows are separate lines.
        assert_eq!(
            wrapped(&["synthetic output 11", "synthetic output 12"], 80).as_deref(),
            Some("synthetic output 12")
        );
        // A blank row ends a line, also after a row that fills the width.
        assert_eq!(
            wrapped(&["Downloading the wor", "", "synthetic output 12"], 19).as_deref(),
            Some("synthetic output 12")
        );
        // Wide glyphs fill a row in fewer characters than columns.
        let wide = "\u{4e2d}".repeat(9);
        let rows = [(wide.as_str(), 18), ("is the worker log", 17)];
        assert_eq!(
            wrapped_status_line(rows, Some(19)).as_deref(),
            Some(format!("{wide} is the worker log").as_str())
        );
    }

    #[test]
    fn a_long_status_line_is_shortened() {
        let long = "word ".repeat(60);
        let line = status_line([long.as_str()]).unwrap();
        assert_eq!(line.chars().count(), MAX_LINE_CHARS);
    }
}
