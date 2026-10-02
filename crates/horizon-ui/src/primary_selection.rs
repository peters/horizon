use egui::Context;
use horizon_core::PanelId;

#[cfg(target_os = "linux")]
use arboard::{Clipboard, GetExtLinux, LinuxClipboardKind, SetExtLinux};
#[cfg(target_os = "linux")]
use std::sync::mpsc::{self, Receiver, Sender};

pub struct PrimarySelectionPaste {
    pub panel_id: PanelId,
    pub text: String,
}

pub struct PrimarySelection {
    #[cfg(target_os = "linux")]
    owner_tx: Sender<OwnerCommand>,
    #[cfg(target_os = "linux")]
    paste_tx: Sender<PrimarySelectionPaste>,
    #[cfg(target_os = "linux")]
    paste_rx: Receiver<PrimarySelectionPaste>,
}

impl Default for PrimarySelection {
    fn default() -> Self {
        Self::new()
    }
}

impl PrimarySelection {
    pub fn new() -> Self {
        #[cfg(target_os = "linux")]
        {
            let owner_tx = spawn_owner_worker();
            let (paste_tx, paste_rx) = mpsc::channel();
            Self {
                owner_tx,
                paste_tx,
                paste_rx,
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            Self {}
        }
    }

    pub fn copy(&self, text: &str) {
        #[cfg(target_os = "linux")]
        if let Err(error) = self.owner_tx.send(OwnerCommand::Set(text.to_owned())) {
            tracing::debug!("primary selection owner unavailable: {error}");
        }

        #[cfg(not(target_os = "linux"))]
        let _ = (self, text);
    }

    pub fn request_paste(&self, panel_id: PanelId, ctx: Context) {
        #[cfg(target_os = "linux")]
        {
            let tx = self.paste_tx.clone();
            let spawn_result = std::thread::Builder::new()
                .name("primary-selection-read".to_owned())
                .spawn(move || match read_primary_text() {
                    Ok(Some(text)) => {
                        if tx.send(PrimarySelectionPaste { panel_id, text }).is_ok() {
                            ctx.request_repaint();
                        }
                    }
                    Ok(None) => {}
                    Err(error) => tracing::debug!("primary selection read failed: {error}"),
                });

            if let Err(error) = spawn_result {
                tracing::debug!("failed to spawn primary selection reader: {error}");
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = (self, panel_id, ctx);
        }
    }

    pub fn try_recv_paste(&mut self) -> Option<PrimarySelectionPaste> {
        #[cfg(target_os = "linux")]
        {
            self.paste_rx.try_recv().ok()
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = self;
            None
        }
    }
}

#[cfg(target_os = "linux")]
enum OwnerCommand {
    Set(String),
}

#[cfg(target_os = "linux")]
fn spawn_owner_worker() -> Sender<OwnerCommand> {
    let (tx, rx) = mpsc::channel();
    let spawn_result = std::thread::Builder::new()
        .name("primary-selection-owner".to_owned())
        .spawn(move || run_owner_worker(rx));

    if let Err(error) = spawn_result {
        tracing::debug!("failed to spawn primary selection owner: {error}");
    }

    tx
}

#[cfg(target_os = "linux")]
fn run_owner_worker(rx: Receiver<OwnerCommand>) {
    let mut clipboard = None;

    for command in rx {
        match command {
            OwnerCommand::Set(text) => set_primary_text(&mut clipboard, &text),
        }
    }
}

#[cfg(target_os = "linux")]
fn set_primary_text(clipboard: &mut Option<Clipboard>, text: &str) {
    if wayland::in_session() {
        match wayland::copy(text) {
            Ok(()) => return,
            Err(error) => tracing::debug!("wayland primary selection write unavailable, using X11: {error}"),
        }
    }

    let Some(primary_clipboard) = ensure_clipboard(clipboard, "set") else {
        return;
    };

    if let Err(error) = primary_clipboard
        .set()
        .clipboard(LinuxClipboardKind::Primary)
        .text(text.to_owned())
    {
        tracing::debug!("primary selection write failed: {error}");
        *clipboard = None;
    }
}

#[cfg(target_os = "linux")]
fn read_primary_text() -> Result<Option<String>, arboard::Error> {
    if wayland::in_session() {
        match wayland::read() {
            Ok(text) => return Ok(text),
            Err(error) => tracing::debug!("wayland primary selection read unavailable, using X11: {error}"),
        }
    }

    let mut clipboard = Clipboard::new()?;
    let text = clipboard.get().clipboard(LinuxClipboardKind::Primary).text()?;

    Ok((!text.is_empty()).then_some(text))
}

#[cfg(target_os = "linux")]
fn ensure_clipboard<'a>(clipboard: &'a mut Option<Clipboard>, operation: &str) -> Option<&'a mut Clipboard> {
    if clipboard.is_none() {
        match Clipboard::new() {
            Ok(new_clipboard) => *clipboard = Some(new_clipboard),
            Err(error) => {
                tracing::debug!("primary selection {operation} unavailable: {error}");
                return None;
            }
        }
    }

    clipboard.as_mut()
}

/// PRIMARY over the Wayland data-control protocols. arboard picks its own
/// Wayland backend from the environment alone and never retries X11 when the
/// compositor lacks primary-selection support, so each operation tries this
/// first and the X11 path covers every failure, as it did before.
#[cfg(target_os = "linux")]
mod wayland {
    use std::io::{self, Read as _};

    use wl_clipboard_rs::copy::{ClipboardType as CopyKind, MimeType as CopyMime, Options, Source};
    use wl_clipboard_rs::paste::{ClipboardType as PasteKind, MimeType as PasteMime, Seat, get_contents};
    use wl_clipboard_rs::utils::{PrimarySelectionCheckError, is_primary_selection_supported};
    use wl_clipboard_rs::{copy, paste};

    #[derive(Debug, thiserror::Error)]
    pub(super) enum Error {
        #[error("the compositor offers no primary selection through data-control")]
        Unsupported,
        #[error(transparent)]
        Check(#[from] PrimarySelectionCheckError),
        #[error(transparent)]
        Copy(#[from] copy::Error),
        #[error(transparent)]
        Paste(#[from] paste::Error),
        #[error(transparent)]
        Read(#[from] io::Error),
    }

    pub(super) fn in_session() -> bool {
        std::env::var_os("WAYLAND_DISPLAY").is_some()
    }

    fn ensure_supported() -> Result<(), Error> {
        if is_primary_selection_supported()? {
            Ok(())
        } else {
            Err(Error::Unsupported)
        }
    }

    pub(super) fn copy(text: &str) -> Result<(), Error> {
        ensure_supported()?;
        let mut options = Options::new();
        options.clipboard(CopyKind::Primary);
        options.copy(Source::Bytes(text.as_bytes().into()), CopyMime::Text)?;
        Ok(())
    }

    pub(super) fn read() -> Result<Option<String>, Error> {
        ensure_supported()?;
        let mut pipe = match get_contents(PasteKind::Primary, Seat::Unspecified, PasteMime::Text) {
            Ok((pipe, _)) => pipe,
            Err(paste::Error::ClipboardEmpty | paste::Error::NoMimeType) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut text = String::new();
        pipe.read_to_string(&mut text)?;
        Ok((!text.is_empty()).then_some(text))
    }
}
