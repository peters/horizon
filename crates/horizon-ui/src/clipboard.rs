//! Horizon's own writes to the desktop clipboard.
//!
//! A write hands the text or the image to the program that owns the desktop
//! clipboard, and an image write encodes a PNG of the pixels first. Both can
//! wait on another program, so one worker thread owns the clipboard of this
//! process: a frame sends it a copy and does no clipboard work itself. The
//! thread stays for the life of the process, because a desktop clipboard keeps
//! the data that its owner published. `primary_selection` owns the PRIMARY
//! selection the same way.
//!
//! A test build records the copies instead of publishing them, so no test
//! writes the clipboard of the computer that runs it.

use std::sync::{
    Mutex, OnceLock, PoisonError,
    mpsc::{self, Receiver, Sender},
};

use egui::ColorImage;

/// One write of the desktop clipboard.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Copy {
    Text(String),
    Image(ColorImage),
}

type Sink = Box<dyn FnMut(&Copy) + Send>;

/// The thread that owns the clipboard, and the copies it has not read yet.
static OWNER: OnceLock<Sender<Copy>> = OnceLock::new();
static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();

#[cfg(test)]
static RECORDED: Mutex<Vec<(std::thread::ThreadId, Copy)>> = Mutex::new(Vec::new());

/// Puts `text` on the desktop clipboard. `text` is a copy of the text of the
/// frame, so a later change of that text reaches no clipboard.
pub(crate) fn copy_text(text: impl Into<String>) {
    send(Copy::Text(text.into()));
}

/// Puts `image` on the desktop clipboard at its own resolution.
pub(crate) fn copy_image(image: ColorImage) {
    send(Copy::Image(image));
}

fn send(copy: Copy) {
    let owner = OWNER.get_or_init(spawn_owner);
    if let Err(error) = owner.send(copy) {
        tracing::debug!("the clipboard owner is unavailable: {error}");
    }
}

fn spawn_owner() -> Sender<Copy> {
    let (owner, copies) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("clipboard-owner".to_owned())
        .spawn(move || run_owner(copies));
    if let Err(error) = spawned {
        tracing::debug!("could not start the clipboard owner: {error}");
    }
    owner
}

fn run_owner(copies: Receiver<Copy>) {
    for copy in copies {
        let mut sink = SINK
            .get_or_init(|| Mutex::new(default_sink()))
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        sink(&copy);
    }
}

#[cfg(test)]
fn default_sink() -> Sink {
    Box::new(|copy| {
        RECORDED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((std::thread::current().id(), copy.clone()));
    })
}

#[cfg(not(test))]
fn default_sink() -> Sink {
    // The owner keeps the clipboard for the life of the process: a desktop
    // clipboard keeps the data that its owner published while the owner lives.
    let mut clipboard: Option<arboard::Clipboard> = None;
    Box::new(move |copy| {
        let Some(clipboard) = ensure_clipboard(&mut clipboard) else {
            return;
        };
        let written = match copy {
            Copy::Text(text) => clipboard.set_text(text.clone()),
            Copy::Image(image) => clipboard.set_image(clipboard_image(image)),
        };
        if let Err(error) = written {
            tracing::debug!("a clipboard write failed: {error}");
        }
    })
}

/// The copies that a test build recorded, with the thread that recorded each.
#[cfg(test)]
pub(crate) fn recorded() -> Vec<(std::thread::ThreadId, Copy)> {
    RECORDED.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// How many copies the process recorded so far, so a test judges the copies of
/// its own frame and not those of a test that runs beside it.
#[cfg(test)]
pub(crate) fn recorded_len() -> usize {
    RECORDED.lock().unwrap_or_else(PoisonError::into_inner).len()
}

/// Waits for the clipboard owner to record a copy that `matches`, for a test
/// that sends a copy on a frame and judges it afterwards.
#[cfg(test)]
pub(crate) fn wait_for(matches: impl Fn(&Copy) -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if recorded().iter().any(|(_, copy)| matches(copy)) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// The clipboard of the desktop, opened on the first write of the process.
#[cfg(not(test))]
fn ensure_clipboard(clipboard: &mut Option<arboard::Clipboard>) -> Option<&mut arboard::Clipboard> {
    if clipboard.is_none() {
        match arboard::Clipboard::new() {
            Ok(opened) => *clipboard = Some(opened),
            Err(error) => {
                tracing::debug!("the desktop clipboard is unavailable: {error}");
                return None;
            }
        }
    }
    clipboard.as_mut()
}

/// The clipboard form of the pixels: unmultiplied RGBA, which is what an image
/// consumer of the desktop expects.
#[cfg(not(test))]
fn clipboard_image(image: &ColorImage) -> arboard::ImageData<'static> {
    let rgba: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(egui::Color32::to_srgba_unmultiplied)
        .collect();
    arboard::ImageData {
        width: image.size[0],
        height: image.size[1],
        bytes: rgba.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Copy, copy_image, copy_text, recorded};
    use egui::{Color32, ColorImage};

    #[test]
    fn a_copy_text_reaches_the_thread_that_owns_the_clipboard() {
        let text = "clipboard owner test text";
        copy_text(text);
        let waited = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let recorded = loop {
            let recorded = recorded();
            if recorded
                .iter()
                .any(|(_, copy)| matches!(copy, Copy::Text(copied) if copied == text))
            {
                break recorded;
            }
            assert!(std::time::Instant::now() < waited, "the clipboard owner read no copy");
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        let (thread, _) = recorded
            .iter()
            .find(|(_, copy)| matches!(copy, Copy::Text(copied) if copied == text))
            .expect("the text arrived");
        assert_ne!(
            *thread,
            std::thread::current().id(),
            "the copy runs on the thread that owns the clipboard, not on the frame"
        );
    }

    #[test]
    fn one_thread_owns_every_copy_of_the_process() {
        copy_text("clipboard owner first text");
        copy_image(ColorImage::filled([1, 1], Color32::from_rgb(1, 2, 3)));
        copy_text("clipboard owner third text");
        let waited = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let owners = loop {
            let recorded = recorded();
            let mine: Vec<_> = recorded
                .iter()
                .filter(|(_, copy)| {
                    matches!(copy,
                        Copy::Text(text) if text.ends_with("text"))
                        || matches!(copy, Copy::Image(image) if image.pixels == [Color32::from_rgb(1, 2, 3)])
                })
                .map(|(thread, _)| *thread)
                .collect();
            if mine.len() >= 3 {
                break mine;
            }
            assert!(std::time::Instant::now() < waited, "the clipboard owner read no copy");
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        assert!(
            owners.iter().all(|thread| *thread == owners[0]),
            "every copy of the process uses the one owning thread: {owners:?}"
        );
    }
}
