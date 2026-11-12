//! One command line of the settings of speech input, split into arguments.
//!
//! A recorder and a transcription command are one line of text with the
//! placeholder `{file}`, because that is what a person copies from a page of
//! recipes. The line is split here into the arguments of one program: no shell
//! reads it, and no part of it is interpreted twice, so a `;`, a `|`, a `$(` or a
//! newline is a character of one argument and never a second command.

use std::ffi::OsString;
use std::fmt;
use std::path::Path;

/// The placeholder that one argument of a line replaces with the take file.
pub const FILE_PLACEHOLDER: &str = "{file}";

/// One command line of the settings, split into its arguments.
///
/// The rules of the split are the ones of a shell word, and they end there:
/// white space divides two arguments, single quotes group the characters between
/// them as they are, double quotes group the characters between them, and a
/// backslash outside single quotes keeps the character that follows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandLine {
    arguments: Vec<String>,
}

/// Why a line of the settings is not one command line.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CommandLineError {
    #[error("the line is empty")]
    Empty,
    #[error("the line ends inside a quote")]
    OpenQuote,
    #[error("the line ends with a backslash")]
    TrailingBackslash,
    #[error("the line has no argument that holds {FILE_PLACEHOLDER}")]
    NoPlaceholder,
}

impl CommandLine {
    /// `line` as the arguments of one program, with no shell between them.
    ///
    /// # Errors
    /// Reports an empty line, a line that ends inside a quote and a line that
    /// ends with a backslash.
    pub fn parse(line: &str) -> Result<Self, CommandLineError> {
        let arguments = split(line)?;
        if arguments.is_empty() {
            return Err(CommandLineError::Empty);
        }
        Ok(Self { arguments })
    }

    /// The arguments as the person wrote them, before a file replaces the
    /// placeholder.
    #[must_use]
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    /// The program of the line: the first argument.
    #[must_use]
    pub fn program(&self) -> &str {
        self.arguments.first().map_or("", String::as_str)
    }

    /// Whether any argument holds the placeholder.
    #[must_use]
    pub fn has_placeholder(&self) -> bool {
        self.arguments
            .iter()
            .any(|argument| argument.contains(FILE_PLACEHOLDER))
    }

    /// The arguments of one run, after the program, with every `{file}` replaced
    /// by `file`.
    ///
    /// One argument stays one argument: a path with a space or a quote in it is
    /// the whole text of that argument, because nothing splits the arguments
    /// again.
    #[must_use]
    pub fn arguments_for(&self, file: &Path) -> Vec<OsString> {
        let file = file.to_string_lossy();
        self.arguments
            .iter()
            .skip(1)
            .map(|argument| OsString::from(argument.replace(FILE_PLACEHOLDER, &file)))
            .collect()
    }
}

impl fmt::Display for CommandLine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.arguments.join(" "))
    }
}

fn split(line: &str) -> Result<Vec<String>, CommandLineError> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut chars = line.chars();
    while let Some(character) = chars.next() {
        match character {
            character if character.is_whitespace() => {
                if started {
                    arguments.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            '\'' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(character) => current.push(character),
                        None => return Err(CommandLineError::OpenQuote),
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(escaped @ ('"' | '\\')) => current.push(escaped),
                            Some(escaped) => {
                                current.push('\\');
                                current.push(escaped);
                            }
                            None => return Err(CommandLineError::OpenQuote),
                        },
                        Some(character) => current.push(character),
                        None => return Err(CommandLineError::OpenQuote),
                    }
                }
            }
            '\\' => {
                started = true;
                match chars.next() {
                    Some(escaped) => current.push(escaped),
                    None => return Err(CommandLineError::TrailingBackslash),
                }
            }
            character => {
                started = true;
                current.push(character);
            }
        }
    }
    if started {
        arguments.push(current);
    }
    Ok(arguments)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{CommandLine, CommandLineError, FILE_PLACEHOLDER};

    #[test]
    fn the_default_recorder_line_splits_into_its_program_and_arguments() {
        let line = CommandLine::parse("pw-record --rate 16000 --channels 1 --format s16 {file}").expect("a line");
        assert_eq!(line.program(), "pw-record");
        assert_eq!(
            line.arguments(),
            [
                "pw-record",
                "--rate",
                "16000",
                "--channels",
                "1",
                "--format",
                "s16",
                FILE_PLACEHOLDER
            ]
        );
        assert!(line.has_placeholder());
    }

    #[test]
    fn a_hostile_line_adds_no_second_command() {
        // A line that a shell would read as three commands stays the arguments of
        // one program: nothing splits it again and no shell reads it.
        let line = CommandLine::parse("rec; rm -rf ~ $(id) {file} | tee /etc/passwd").expect("a line");
        assert_eq!(line.program(), "rec;");
        assert_eq!(
            line.arguments(),
            ["rec;", "rm", "-rf", "~", "$(id)", "{file}", "|", "tee", "/etc/passwd"]
        );

        // A newline is white space of one argument list, and never an argument.
        let line = CommandLine::parse("rec\n{file}\n> /etc/passwd").expect("a line");
        assert_eq!(line.arguments(), ["rec", "{file}", ">", "/etc/passwd"]);
    }

    #[test]
    fn quotes_group_one_argument_and_a_backslash_keeps_its_character() {
        let line = CommandLine::parse(r#"rec --title 'my take' --device "a b" {file}"#).expect("a line");
        assert_eq!(
            line.arguments(),
            ["rec", "--title", "my take", "--device", "a b", "{file}"]
        );

        let line = CommandLine::parse(r#"rec a\ b \"c\" {file}"#).expect("a line");
        assert_eq!(line.arguments(), ["rec", "a b", "\"c\"", "{file}"]);

        let line = CommandLine::parse(r#"rec "a \"b\" c" {file}"#).expect("a line");
        assert_eq!(line.arguments(), ["rec", "a \"b\" c", "{file}"]);
    }

    #[test]
    fn a_line_that_ends_inside_a_word_is_refused() {
        assert_eq!(CommandLine::parse(""), Err(CommandLineError::Empty));
        assert_eq!(CommandLine::parse("   "), Err(CommandLineError::Empty));
        assert_eq!(CommandLine::parse("rec 'a {file}"), Err(CommandLineError::OpenQuote));
        assert_eq!(CommandLine::parse("rec \"a {file}"), Err(CommandLineError::OpenQuote));
        assert_eq!(
            CommandLine::parse("rec {file} \\"),
            Err(CommandLineError::TrailingBackslash)
        );
    }

    #[test]
    fn the_run_arguments_leave_the_program_out_and_keep_the_path_as_one_argument() {
        let line = CommandLine::parse("ffmpeg -y -i :default {file}").expect("a line");
        let arguments = line.arguments_for(Path::new("/home/a person/.horizon/speech/take-1.wav"));
        assert_eq!(
            arguments,
            ["-y", "-i", ":default", "/home/a person/.horizon/speech/take-1.wav"]
        );

        // A placeholder inside an argument replaces only its own text.
        let line = CommandLine::parse("rec --out={file} x").expect("a line");
        assert_eq!(
            line.arguments_for(Path::new("/t/take-2.wav")),
            ["--out=/t/take-2.wav", "x"]
        );

        let line = CommandLine::parse("rec {file} {file}").expect("a line");
        assert_eq!(line.arguments_for(Path::new("/t/a.wav")), ["/t/a.wav", "/t/a.wav"]);

        // The program of the line is never an argument of the run.
        let line = CommandLine::parse("{file} --flag").expect("a line");
        assert_eq!(line.arguments_for(Path::new("/t/a.wav")), ["--flag"]);
    }

    #[test]
    fn a_line_without_the_placeholder_answers_that_it_has_none() {
        let line = CommandLine::parse("rec --default").expect("a line");
        assert!(!line.has_placeholder());
        assert_eq!(line.to_string(), "rec --default");
    }

    #[test]
    fn a_quoting_character_of_a_path_reaches_the_recorder_as_one_argument() {
        // A hostile take name cannot break out of the argument that the person
        // wrote, because the split is already done and the path is no word.
        let line = CommandLine::parse("rec {file} --flag").expect("a line");
        let arguments = line.arguments_for(Path::new("/t/a b'; rm -rf ~; '.wav"));
        assert_eq!(arguments, ["/t/a b'; rm -rf ~; '.wav", "--flag"]);
    }
}
