use egui::text::{LayoutJob, LayoutSection};
use egui::{Color32, FontId, TextFormat};

use crate::theme;

/// Build a syntax-highlighted `LayoutJob` for a YAML string.
///
/// The tokenizer is intentionally simple — it handles the subset of YAML
/// that appears in Horizon config files (keys, quoted strings, comments,
/// booleans, numbers) without pulling in `syntect` or `tree-sitter`.
pub fn highlight_yaml(text: &str, font_id: &FontId) -> LayoutJob {
    let mut job = LayoutJob {
        text: text.into(),
        ..Default::default()
    };
    job.wrap.max_width = f32::INFINITY;

    for line in text.split_inclusive('\n') {
        highlight_line(&mut job, text, line, font_id);
    }

    job
}

fn highlight_line(job: &mut LayoutJob, full: &str, line: &str, font_id: &FontId) {
    let line_start = byte_offset(full, line);

    // Comment line (leading whitespace + #)
    if let Some(hash_pos) = line.find('#') {
        let before_hash = &line[..hash_pos];
        if before_hash.chars().all(|c| c == ' ' || c == '\t') || before_hash.ends_with(' ') {
            // Everything before the # keeps default color.
            if hash_pos > 0 {
                push(job, line_start, hash_pos, theme::FG(), font_id);
            }
            // The comment itself.
            push(
                job,
                line_start + hash_pos,
                line.len() - hash_pos,
                theme::FG_DIM(),
                font_id,
            );
            return;
        }
    }

    // Try to split on first `:` for key/value.
    if let Some(colon_pos) = first_mapping_colon(line) {
        // Key portion (before colon).
        push(job, line_start, colon_pos, theme::ACCENT(), font_id);
        // The colon (and any trailing space).
        let sep_len = if line.as_bytes().get(colon_pos + 1) == Some(&b' ') {
            2
        } else {
            1
        };
        push(job, line_start + colon_pos, sep_len, theme::FG_SOFT(), font_id);
        // Value portion.
        let value_start = colon_pos + sep_len;
        let value = &line[value_start..];
        highlight_value(job, line_start + value_start, value, font_id);
    } else {
        // List item or plain line.
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if indent > 0 {
            push(job, line_start, indent, theme::FG(), font_id);
        }
        if let Some(rest) = trimmed.strip_prefix("- ") {
            push(job, line_start + indent, 2, theme::FG_SOFT(), font_id);
            highlight_value(job, line_start + indent + 2, rest, font_id);
        } else {
            highlight_value(job, line_start + indent, trimmed, font_id);
        }
    }
}

fn highlight_value(job: &mut LayoutJob, offset: usize, value: &str, font_id: &FontId) {
    let trimmed = value.trim_end_matches('\n');
    let trailing = value.len() - trimmed.len();
    let v = trimmed.trim();

    let color = if v.is_empty() {
        theme::FG()
    } else if is_quoted(v) {
        theme::PALETTE_GREEN()
    } else if is_number_or_bool(v) {
        theme::PALETTE_YELLOW()
    } else {
        theme::FG()
    };

    if !trimmed.is_empty() {
        push(job, offset, trimmed.len(), color, font_id);
    }
    if trailing > 0 {
        push(job, offset + trimmed.len(), trailing, theme::FG(), font_id);
    }
}

fn push(job: &mut LayoutJob, byte_offset: usize, len: usize, color: Color32, font_id: &FontId) {
    job.sections.push(LayoutSection {
        leading_space: 0.0,
        byte_range: byte_offset..byte_offset + len,
        format: TextFormat {
            font_id: font_id.clone(),
            color,
            ..Default::default()
        },
    });
}

/// Return the byte offset of `sub` within `full`.
fn byte_offset(full: &str, sub: &str) -> usize {
    sub.as_ptr() as usize - full.as_ptr() as usize
}

/// Find the first `:` that looks like a YAML mapping separator —
/// must not be inside quotes and should be followed by whitespace, EOL, or EOF.
fn first_mapping_colon(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();

    let mut in_single = false;
    let mut in_double = false;
    for (i, ch) in trimmed.char_indices() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            ':' if !in_single && !in_double => {
                let after = trimmed.as_bytes().get(i + 1).copied();
                if after == Some(b' ') || after == Some(b'\n') || after.is_none() {
                    return Some(indent + i);
                }
            }
            _ => {}
        }
    }
    None
}

fn is_quoted(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
}

fn is_number_or_bool(s: &str) -> bool {
    matches!(s, "true" | "false" | "yes" | "no" | "null" | "~") || s.parse::<f64>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_line_uses_dim() {
        let text = "# comment\n";
        let job = highlight_yaml(text, &FontId::monospace(13.0));
        assert!(!job.sections.is_empty());
        let last = job.sections.last().unwrap();
        assert_eq!(last.format.color, theme::FG_DIM());
    }

    #[test]
    fn key_value_colored() {
        let text = "name: hello\n";
        let job = highlight_yaml(text, &FontId::monospace(13.0));
        // First section is the key — should be ACCENT.
        assert_eq!(job.sections[0].format.color, theme::ACCENT());
    }

    #[test]
    fn quoted_string_is_green() {
        let text = "key: \"value\"\n";
        let job = highlight_yaml(text, &FontId::monospace(13.0));
        let value_section = &job.sections[2];
        assert_eq!(value_section.format.color, theme::PALETTE_GREEN());
    }

    #[test]
    fn boolean_is_yellow() {
        let text = "enabled: true\n";
        let job = highlight_yaml(text, &FontId::monospace(13.0));
        let value_section = &job.sections[2];
        assert_eq!(value_section.format.color, theme::PALETTE_YELLOW());
    }
}
