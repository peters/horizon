use egui::Context;
use horizon_core::PanelId;

#[cfg(target_os = "linux")]
use arboard::{Clipboard, GetExtLinux, LinuxClipboardKind, SetExtLinux};
#[cfg(target_os = "linux")]
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

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
        match self.paste_rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
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

    while let Ok(command) = rx.recv() {
        match command {
            OwnerCommand::Set(text) => set_primary_text(&mut clipboard, &text),
        }
    }
}

#[cfg(target_os = "linux")]
fn set_primary_text(clipboard: &mut Option<Clipboard>, text: &str) {
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
