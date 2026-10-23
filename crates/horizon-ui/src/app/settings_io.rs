//! Settings reads and writes on a worker thread.
//!
//! A save writes the store with `synchronous=FULL`, and a reload opens it and
//! waits for the file. Those are waits, so they run on this thread and never on
//! the UI thread, as
//! [`docs/architecture/ui-thread.md`](../../../../docs/architecture/ui-thread.md)
//! requires. The window applies a reply in a later frame.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use horizon_core::Config;

/// What a save writes.
pub(super) enum SettingsWrite {
    /// The canonical settings of the window.
    Config(Box<Config>),
    /// The YAML text of a person, and the settings that it parses to.
    Text(String),
}

/// One request for the worker thread.
enum Request {
    Save { path: PathBuf, write: SettingsWrite },
    Reload { path: PathBuf },
}

/// What the worker did, for the window to apply.
pub(super) enum SettingsReply {
    /// The save finished, and this is the YAML that describes the store now.
    Saved {
        path: PathBuf,
        text: String,
        config: Box<Config>,
    },
    /// The settings of `path`, or `None` when the store and the copy did not
    /// exist.
    Reloaded { path: PathBuf, config: Option<Box<Config>> },
    /// The request failed. `what` says which one.
    Failed { what: &'static str, error: String },
}

/// The window's handle to the worker thread.
pub(super) struct SettingsIo {
    requests: Sender<Request>,
    replies: Receiver<SettingsReply>,
    /// How many requests have no reply yet.
    pending: AtomicUsize,
}

impl SettingsIo {
    /// Start the worker. The thread ends when this handle is dropped.
    pub(super) fn new() -> Self {
        let (requests, request_rx) = channel::<Request>();
        let (reply_tx, replies) = channel::<SettingsReply>();
        let spawned = std::thread::Builder::new()
            .name("horizon-settings".to_string())
            .spawn(move || {
                while let Ok(request) = request_rx.recv() {
                    if reply_tx.send(serve(request)).is_err() {
                        return;
                    }
                }
            });
        if let Err(error) = spawned {
            // The dropped thread body closed both channels, so a save reports
            // that the thread does not run and the window keeps its settings.
            tracing::error!("cannot start the settings thread: {error}");
        }
        Self {
            requests,
            replies,
            pending: AtomicUsize::new(0),
        }
    }

    /// Whether a request is still running, so the window keeps drawing frames
    /// until its reply arrives.
    pub(super) fn is_busy(&self) -> bool {
        self.pending.load(Ordering::Relaxed) > 0
    }

    /// Save the configuration.
    pub(super) fn save_config(&self, path: PathBuf, config: Config) {
        self.send(Request::Save {
            path,
            write: SettingsWrite::Config(Box::new(config)),
        });
    }

    /// Save the text that a person wrote.
    pub(super) fn save_text(&self, path: PathBuf, text: String) {
        self.send(Request::Save {
            path,
            write: SettingsWrite::Text(text),
        });
    }

    /// Read the settings again.
    pub(super) fn reload(&self, path: PathBuf) {
        self.send(Request::Reload { path });
    }

    /// The next reply, when the worker has one. It never waits.
    pub(super) fn poll(&self) -> Option<SettingsReply> {
        match self.replies.try_recv() {
            Ok(reply) => {
                self.pending.fetch_sub(1, Ordering::Relaxed);
                Some(reply)
            }
            // An empty channel and a closed one both mean no answer yet.
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    fn send(&self, request: Request) {
        if self.requests.send(request).is_err() {
            tracing::error!("the settings thread does not run");
            return;
        }
        self.pending.fetch_add(1, Ordering::Relaxed);
    }
}

fn serve(request: Request) -> SettingsReply {
    match request {
        Request::Save { path, write } => match write {
            SettingsWrite::Config(config) => match Config::save(&path, &config) {
                Ok(()) => match config.to_yaml() {
                    Ok(text) => SettingsReply::Saved { path, text, config },
                    Err(error) => SettingsReply::Failed {
                        what: "save",
                        error: error.to_string(),
                    },
                },
                Err(error) => SettingsReply::Failed {
                    what: "save",
                    error: error.to_string(),
                },
            },
            SettingsWrite::Text(text) => match Config::save_text(&path, &text) {
                Ok(()) => match Config::from_yaml(&text) {
                    // The store keeps settings, not text, so the window shows the
                    // canonical YAML of what it saved.
                    Ok(config) => match config.to_yaml() {
                        Ok(text) => SettingsReply::Saved {
                            path,
                            text,
                            config: Box::new(config),
                        },
                        Err(error) => SettingsReply::Failed {
                            what: "save",
                            error: error.to_string(),
                        },
                    },
                    Err(error) => SettingsReply::Failed {
                        what: "save",
                        error: error.to_string(),
                    },
                },
                Err(error) => SettingsReply::Failed {
                    what: "save",
                    error: error.to_string(),
                },
            },
        },
        Request::Reload { path } => {
            let store = horizon_core::config_store::ConfigStore::resolve(&path);
            if !path.exists() && !store.path().exists() {
                return SettingsReply::Reloaded { path, config: None };
            }
            match Config::load(Some(&path)) {
                Ok(config) => SettingsReply::Reloaded {
                    path,
                    config: Some(Box::new(config)),
                },
                Err(error) => SettingsReply::Failed {
                    what: "reload",
                    error: error.to_string(),
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait(io: &SettingsIo) -> SettingsReply {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(reply) = io.poll() {
                return reply;
            }
            assert!(Instant::now() < deadline, "the worker answered nothing");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_text_save_writes_the_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.yaml");
        let io = SettingsIo::new();
        io.save_text(path.clone(), "version: 11\nappearance:\n  theme: light\n".to_string());

        match wait(&io) {
            SettingsReply::Saved { text, config, .. } => {
                assert!(text.contains("theme: light"));
                assert_eq!(config.appearance.theme, horizon_core::AppearanceTheme::Light);
            }
            SettingsReply::Failed { error, .. } => panic!("{error}"),
            SettingsReply::Reloaded { .. } => panic!("a save answered a reload"),
        }
        assert!(
            horizon_core::config_store::ConfigStore::resolve(&path).path().is_file(),
            "the store holds the settings"
        );
        assert!(
            !path.exists(),
            "a save writes no YAML file; a person exports one on request"
        );
    }

    #[test]
    fn a_reload_reports_the_settings_of_the_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.yaml");
        let io = SettingsIo::new();
        io.save_text(path.clone(), "version: 11\nappearance:\n  theme: light\n".to_string());
        let _ = wait(&io);

        io.reload(path);
        match wait(&io) {
            SettingsReply::Reloaded { config, .. } => {
                let config = config.expect("the settings exist");
                assert_eq!(config.appearance.theme, horizon_core::AppearanceTheme::Light);
            }
            SettingsReply::Failed { error, .. } => panic!("{error}"),
            SettingsReply::Saved { .. } => panic!("a reload answered a save"),
        }
    }

    #[test]
    fn a_reload_of_nothing_reports_no_settings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let io = SettingsIo::new();
        io.reload(dir.path().join("config.yaml"));
        match wait(&io) {
            SettingsReply::Reloaded { config, .. } => assert!(config.is_none()),
            SettingsReply::Failed { error, .. } => panic!("{error}"),
            SettingsReply::Saved { .. } => panic!("a reload answered a save"),
        }
    }
}
