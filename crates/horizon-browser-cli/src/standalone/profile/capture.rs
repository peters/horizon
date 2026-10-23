//! `browser_screenshot` on a profile host: the newest frame of a page as a
//! private PNG, for the actor that holds the live claim of the page. The
//! file stays under the runtime root of the host; `mcp --connect
//! --inline-screenshots` gives its pixels to a client on another computer.

use std::path::{Path, PathBuf};

use horizon_browser::FrameData;
use horizon_browser_control::manifest::{
    self, AgentIdentity, BrowserManifest,
    device::{Outcome, Screenshot, ScreenshotInput},
};
use horizon_browser_control::paths::safe_local_id;

/// At most this many captures stay for each page; older ones go.
pub const MAX_CAPTURES: usize = 8;
const MAX_PIXELS: u64 = 64 * 1024 * 1024;

/// The folder of the captures of every page.
#[must_use]
pub fn captures(root: &Path) -> PathBuf {
    root.join("profile-host-captures")
}

/// The folder of the captures of one page.
#[must_use]
pub fn folder(root: &Path, page: &str) -> PathBuf {
    captures(root).join(safe_local_id(page))
}

/// Answer one screenshot of `input.panel_id` for `caller` from the newest
/// frame of the page. `live` is the manifest of the page, and `now` the time
/// in Unix milliseconds.
pub fn answer(
    root: &Path,
    caller: AgentIdentity<'_>,
    input: &ScreenshotInput,
    live: Option<&BrowserManifest>,
    frame: Option<&FrameData>,
    now: i64,
) -> Outcome {
    let Some(live) = live else {
        return Outcome::failed("browser_unavailable", "Browser manifest is unavailable");
    };
    if live.panel_local_id != input.panel_id || !live.authorizes(caller) {
        return Outcome::failed("panel_unavailable", "Panel is unavailable to this agent");
    }
    if live.live_owner(now).is_none_or(|claim| claim.name != caller.actor) {
        return Outcome::failed("not_owner", "The caller does not hold this browser panel's live claim");
    }
    let Some(frame) = frame else {
        return Outcome::failed("screenshot_unavailable", "Browser has not received a frame");
    };
    match write(root, &input.panel_id, frame) {
        Ok(path) => Outcome::Screenshot {
            capture: Screenshot {
                panel_id: input.panel_id.clone(),
                path,
                width: frame.width,
                height: frame.height,
                clipboard_requested: false,
            },
        },
        Err(message) => Outcome::failed("screenshot_unavailable", &message),
    }
}

pub(super) fn write(root: &Path, page: &str, frame: &FrameData) -> Result<PathBuf, String> {
    let pixels = u64::from(frame.width) * u64::from(frame.height);
    let expected = (1..=MAX_PIXELS)
        .contains(&pixels)
        .then(|| usize::try_from(pixels * 3).ok())
        .flatten();
    if expected != Some(frame.rgb.len()) {
        return Err("Browser frame has invalid dimensions or pixels".into());
    }
    let dir = folder(root, page);
    super::host::private_dir(&dir).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(&frame.rgb))
        .map_err(|error| error.to_string())?;
    let path = dir.join(format!("capture-{:020}-{}.png", manifest::now_millis(), frame.seq));
    super::host::write_private(&path, &bytes).map_err(|error| error.to_string())?;
    prune(&dir, &path);
    Ok(path)
}

/// Keep only the newest captures of a page. The capture that was just
/// `written` always stays, also when a clock step sorts its name first.
fn prune(dir: &Path, written: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path != written && path.extension().is_some_and(|extension| extension == "png"))
        .collect();
    files.sort();
    let old = files.len().saturating_sub(MAX_CAPTURES - 1);
    for path in files.into_iter().take(old) {
        let _ = std::fs::remove_file(path);
    }
}
