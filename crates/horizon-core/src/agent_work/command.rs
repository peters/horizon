use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

use super::lifecycle::quote_argument;

/// Resolve in the same interactive shell as the real launch. Functions and
/// aliases retain ordinary launch behavior but cannot prove process ownership.
pub(super) fn owned_command(
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
    env: &HashMap<String, String>,
) -> Option<String> {
    if !cfg!(unix) || args.len() != 2 || args[0] != "-ic" {
        return None;
    }
    if !matches!(Path::new(program).file_name()?.to_str()?, "bash" | "zsh" | "sh" | "ksh") {
        return None;
    }
    let (name, suffix) = args[1].split_once(' ').unwrap_or((&args[1], ""));
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return None;
    }
    let output = tempfile::NamedTempFile::new().ok()?;
    let script = format!("command -v {name} > {}", quote_argument(output.path().to_str()?));
    let mut probe = crate::Terminal::spawn(crate::terminal::TerminalSpawnOptions {
        program: program.to_owned(),
        args: vec!["-ic".into(), script],
        cwd: cwd.map(Path::to_path_buf),
        rows: 24,
        cols: 80,
        cell_width: 8,
        cell_height: 16,
        scrollback_limit: 0,
        window_id: 0,
        replay_bytes: Vec::new(),
        env: env.clone(),
        kitty_keyboard: false,
    })
    .ok()?;
    let deadline = Instant::now() + Duration::from_secs(1);
    while !probe.child_exited() && Instant::now() < deadline {
        probe.process_events();
        std::thread::sleep(Duration::from_millis(5));
    }
    let success = probe.child_exit_status().is_some_and(|status| status.success());
    if !probe.shutdown_with_timeout(Duration::from_millis(250)) || !success {
        return None;
    }
    let mut bytes = Vec::new();
    output.as_file().take(4097).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 4096 {
        return None;
    }
    let text = std::str::from_utf8(&bytes).ok()?.trim_end_matches(['\r', '\n']);
    if text.contains(['\r', '\n']) {
        return None;
    }
    let path = Path::new(text);
    if !path.is_absolute() || !path.is_file() {
        return None;
    }
    Some(format!(
        "exec {}{}{}",
        quote_argument(text),
        if suffix.is_empty() { "" } else { " " },
        suffix
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn functions_and_aliases_preserve_the_ordinary_launch() {
        let home = tempfile::tempdir().expect("home");
        let mut env = HashMap::new();
        env.insert("HOME".into(), home.path().display().to_string());
        for definition in [
            "cat() { :; }",
            "alias cat='echo fixture'",
            "if test -t 0; then cat() { :; }; fi",
        ] {
            std::fs::write(home.path().join(".bashrc"), definition).expect("shell config");
            let args = vec!["-ic".into(), "cat --resume same-session".into()];
            assert!(owned_command("/bin/bash", &args, Some(home.path()), &env).is_none());
            assert_eq!(args[1], "cat --resume same-session");
        }
    }

    #[test]
    fn executable_resolution_pins_the_path_and_preserves_arguments() {
        let home = tempfile::tempdir().expect("home");
        std::fs::write(home.path().join(".bashrc"), "").expect("shell config");
        let env = HashMap::from([("HOME".into(), home.path().display().to_string())]);
        let args = vec!["-ic".into(), "cat -- 'quoted argument'".into()];
        let owned = owned_command("/bin/bash", &args, Some(home.path()), &env).expect("external command");
        assert!(owned.starts_with("exec '/"));
        assert!(owned.ends_with(" -- 'quoted argument'"));
    }
}
