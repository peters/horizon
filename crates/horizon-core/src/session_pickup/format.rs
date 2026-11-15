//! The text of one row of the pickup list.
//!
//! Every value here comes from a session store of an agent program, so every
//! value is untrusted. [`one_line`] is the only way that a title or a message
//! reaches the window: it removes the control characters, runs of white space
//! become one space, and the text is cut to a named number of characters.

use std::path::Path;

use crate::util::truncate_chars;

/// The most characters of a title that a row shows.
///
/// The bound is what a row of the widest block of the page holds on one line: the
/// block stops at 1 480 points, and a title of this many characters fills the room
/// that the row keeps beside its press without reaching it. The row cuts a title
/// that is longer than its own room with an ellipsis, so this bound is the one of
/// the stored text, not of the width of a row.
pub const MAX_TITLE_CHARS: usize = 160;
/// The most characters of a folder name or of a parent path that a row shows.
pub const MAX_FOLDER_CHARS: usize = 48;

const MINUTE_MILLIS: i64 = 60_000;
const HOUR_MILLIS: i64 = 60 * MINUTE_MILLIS;
const DAY_MILLIS: i64 = 24 * HOUR_MILLIS;

/// `value` as the one line that a row shows, at most `max_chars` characters.
///
/// A control character becomes a space, so a title cannot move the cursor of a
/// terminal, break the line of a row or hide its own end. Every run of white
/// space becomes one space, and the text is cut with the ellipsis of
/// [`truncate_chars`].
#[must_use]
pub fn one_line(value: &str, max_chars: usize) -> String {
    let mut cleaned = String::with_capacity(value.len());
    let mut after_space = false;
    for character in value.chars() {
        let character = if character.is_control() { ' ' } else { character };
        if character.is_whitespace() {
            if !after_space && !cleaned.is_empty() {
                cleaned.push(' ');
            }
            after_space = true;
            continue;
        }
        after_space = false;
        cleaned.push(character);
    }
    let trimmed = cleaned.trim_end();
    truncate_chars(trimmed, max_chars).into_owned()
}

/// The words of the time that passed between `then_millis` and `now_millis`.
///
/// A time that lies after `now_millis` is `just now`, because a clock of another
/// computer or a store that another program wrote can be ahead of this one.
#[must_use]
pub fn relative_time(now_millis: i64, then_millis: i64) -> String {
    let elapsed = now_millis.saturating_sub(then_millis).max(0);
    if elapsed < MINUTE_MILLIS {
        return "just now".to_string();
    }
    if elapsed < HOUR_MILLIS {
        let minutes = elapsed / MINUTE_MILLIS;
        return if minutes == 1 {
            "a minute ago".to_string()
        } else {
            format!("{minutes} minutes ago")
        };
    }
    if elapsed < DAY_MILLIS {
        let hours = elapsed / HOUR_MILLIS;
        return if hours == 1 {
            "an hour ago".to_string()
        } else {
            format!("{hours} hours ago")
        };
    }
    let days = elapsed / DAY_MILLIS;
    if days == 1 {
        "yesterday".to_string()
    } else {
        format!("{days} days ago")
    }
}

/// The name of `path` and the path of its folder, both as a row shows them.
///
/// The folder of the person is `~`, in the name and in the parent path, so no row
/// repeats the name of the person. A folder that is the home folder itself has no
/// parent path.
#[must_use]
pub fn folder_name_and_parent(path: &Path, home: &Path) -> (String, String) {
    let home = crate::folder_id::folder_identity(home);
    let resolved = crate::folder_id::folder_identity(path);
    if resolved == home {
        return ("~".to_string(), String::new());
    }
    let name = resolved.file_name().map_or_else(
        || shown_below(&resolved, &home),
        |name| shown_below(Path::new(name), &home),
    );
    let parent = resolved
        .parent()
        .map_or_else(String::new, |parent| shown_below(parent, &home));
    (name, parent)
}

/// `path` as a row writes it: the home folder is `~`, and the text is cut to
/// [`MAX_FOLDER_CHARS`] characters.
fn shown_below(path: &Path, home: &Path) -> String {
    let text = match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    };
    truncate_chars(&text, MAX_FOLDER_CHARS).into_owned()
}

#[cfg(test)]
mod tests {
    use super::{MAX_TITLE_CHARS, folder_name_and_parent, one_line, relative_time};

    #[test]
    fn a_title_is_one_line_without_control_characters() {
        assert_eq!(one_line("  Fix\tthe\n\nbuild  ", 80), "Fix the build");
        assert_eq!(
            one_line("a\u{1b}[31mred\u{7}bell\u{0}", 80),
            "a [31mred bell",
            "an escape of a terminal and a bell are spaces"
        );
        assert_eq!(one_line("", 80), "");
        assert_eq!(one_line("   ", 80), "");
        assert_eq!(one_line("\n\t\u{1b}", 80), "");
    }

    #[test]
    fn a_long_title_is_cut_to_the_named_number_of_characters() {
        let long = "x".repeat(5000);
        let shown = one_line(&long, MAX_TITLE_CHARS);

        assert!(shown.chars().count() <= MAX_TITLE_CHARS, "{}", shown.chars().count());
        assert!(shown.ends_with('…'), "the reader sees that the text is cut");
        assert_eq!(
            one_line(&"é".repeat(5000), 10).chars().count(),
            10,
            "a character outside the Basic Latin block counts once"
        );
    }

    /// The bound of the stored title is the number that the boundary page states,
    /// and a title at that bound stays whole.
    #[test]
    fn a_title_at_the_bound_of_the_row_is_kept_whole() {
        assert_eq!(
            MAX_TITLE_CHARS, 160,
            "the boundary page `agent-session-pickup.md` states the bound of a title"
        );

        let at_the_bound = "é".repeat(MAX_TITLE_CHARS);
        assert_eq!(
            one_line(&at_the_bound, MAX_TITLE_CHARS),
            at_the_bound,
            "a title of {MAX_TITLE_CHARS} characters is stored whole"
        );

        let over = "x".repeat(MAX_TITLE_CHARS + 1);
        let shown = one_line(&over, MAX_TITLE_CHARS);
        assert_eq!(
            shown.chars().count(),
            MAX_TITLE_CHARS,
            "the bound holds one character more"
        );
        assert!(shown.ends_with('…'), "the reader sees that the title is cut");
    }

    #[test]
    fn the_time_of_a_session_is_a_word_of_this_computer() {
        let now = 1_000_000_000_000_i64;
        let at = |millis: i64| relative_time(now, now.saturating_sub(millis));

        assert_eq!(at(0), "just now");
        assert_eq!(at(59_000), "just now");
        assert_eq!(at(60_000), "a minute ago");
        assert_eq!(at(2 * 60_000), "2 minutes ago");
        assert_eq!(at(59 * 60_000), "59 minutes ago");
        assert_eq!(at(60 * 60_000), "an hour ago");
        assert_eq!(at(2 * 60 * 60_000), "2 hours ago");
        assert_eq!(at(23 * 60 * 60_000), "23 hours ago");
        assert_eq!(at(24 * 60 * 60_000), "yesterday");
        assert_eq!(at(2 * 24 * 60 * 60_000), "2 days ago");
        assert_eq!(
            at(-60_000),
            "just now",
            "a store that is ahead of this clock is not a time in the future"
        );
    }

    #[test]
    fn a_folder_names_itself_and_its_parent() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path().join("home");
        let folder = home.join("github").join("horizon");
        std::fs::create_dir_all(&folder).expect("the folder");

        let (name, parent) = folder_name_and_parent(&folder, &home);
        assert_eq!(name, "horizon");
        assert_eq!(parent, "~/github", "the parent path writes the home folder as ~");

        let (name, parent) = folder_name_and_parent(&home, &home);
        assert_eq!(name, "~");
        assert!(parent.is_empty(), "the home folder has no parent path: {parent}");
    }

    #[test]
    fn a_long_folder_name_is_cut_to_the_named_number_of_characters() {
        let temp = tempfile::tempdir().expect("temp dir");
        let home = temp.path().join("home");
        let folder = home.join("n".repeat(200));
        std::fs::create_dir_all(&folder).expect("the folder");

        let (name, _) = folder_name_and_parent(&folder, &home);

        assert!(name.chars().count() <= super::MAX_FOLDER_CHARS, "{name}");
    }
}
