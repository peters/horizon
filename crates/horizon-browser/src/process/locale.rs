//! Chromium converts the file paths of uploads and drops with the character
//! set of the C library. In a locale without UTF-8, such as the default `C`
//! locale of a cloud worker, it drops each file whose name is not ASCII.
use std::ffi::OsStr;
use std::process::Command;

/// Gives a browser process UTF-8 file names when its locale has none. An
/// explicit `LC_ALL` stays as it is, because it overrides `LC_CTYPE`.
pub(super) fn apply(command: &mut Command) {
    let var = std::env::var_os;
    set_utf8_ctype(
        command,
        var("LC_ALL").as_deref(),
        var("LC_CTYPE").as_deref(),
        var("LANG").as_deref(),
    );
}

fn set_utf8_ctype(command: &mut Command, lc_all: Option<&OsStr>, lc_ctype: Option<&OsStr>, lang: Option<&OsStr>) {
    fn set(value: Option<&OsStr>) -> Option<&OsStr> {
        value.filter(|value| !value.is_empty())
    }
    let utf8 = |value: &OsStr| {
        let value = value.to_string_lossy().to_ascii_lowercase();
        value.contains("utf-8") || value.contains("utf8")
    };
    if set(lc_all).is_none() && !set(lc_ctype).or(set(lang)).is_some_and(utf8) {
        command.env("LC_CTYPE", "C.UTF-8");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_locale_without_utf8_gets_a_utf8_character_set() {
        let os = |value: &'static str| Some(OsStr::new(value));
        let cases = [
            (None, None, None, true),
            (os(""), None, os("C"), true),
            (None, os("POSIX"), os("en_US.UTF-8"), true),
            (None, None, os("nb_NO.ISO-8859-1"), true),
            (None, None, os("nb_NO.UTF-8"), false),
            (None, os("C.utf8"), None, false),
            (None, os(""), os("en_US.utf8"), false),
            // LC_ALL is the person's choice.
            (os("C"), None, None, false),
        ];
        for (lc_all, lc_ctype, lang, expected) in cases {
            let mut command = Command::new("browser");
            set_utf8_ctype(&mut command, lc_all, lc_ctype, lang);
            let set: Vec<_> = command.get_envs().collect();
            let wanted = [(OsStr::new("LC_CTYPE"), Some(OsStr::new("C.UTF-8")))];
            assert_eq!(set == wanted, expected, "{lc_all:?} {lc_ctype:?} {lang:?}");
            assert!(set.is_empty() || set == wanted);
        }
    }
}
