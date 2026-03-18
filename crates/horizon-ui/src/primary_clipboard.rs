/// Copy `text` into the system primary selection buffer.
///
/// Uses `xclip`/`xsel` on X11 or `wl-copy` on Wayland, which properly
/// integrate with the display server for cross-application selection.
/// No-op on non-Linux platforms.
pub fn copy_to_primary(text: &str) {
    #[cfg(target_os = "linux")]
    {
        if let Err(error) = copy_primary_linux(text) {
            tracing::debug!("primary selection: copy failed: {error}");
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = text;
    }
}

/// Read text from the system primary selection buffer.
///
/// Returns `None` on non-Linux platforms or when the buffer is empty /
/// inaccessible.
pub fn paste_from_primary() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        match paste_primary_linux() {
            Ok(text) if !text.is_empty() => Some(text),
            Ok(_) => None,
            Err(error) => {
                tracing::debug!("primary selection: read failed: {error}");
                None
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

#[cfg(target_os = "linux")]
fn copy_primary_linux(text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let mut child = if is_wayland() {
        Command::new("wl-copy")
            .arg("--primary")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?
    } else {
        Command::new("xclip")
            .args(["-selection", "primary"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?
    };

    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(text.as_bytes())?;
    }
    // Drop stdin to close the pipe so xclip/wl-copy can finish reading.
    drop(child.stdin.take());

    // Don't wait — xclip forks a daemon to serve the selection, wl-copy
    // stays resident until replaced.  Waiting would block the UI thread.
    Ok(())
}

#[cfg(target_os = "linux")]
fn paste_primary_linux() -> std::io::Result<String> {
    use std::process::{Command, Stdio};

    let output = if is_wayland() {
        Command::new("wl-paste")
            .args(["--primary", "--no-newline"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()?
    } else {
        Command::new("xclip")
            .args(["-selection", "primary", "-o"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()?
    };

    String::from_utf8(output.stdout).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}
