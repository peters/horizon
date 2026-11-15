//! The card sockets of the sessions of the machines of the machine list.
//!
//! One Unix socket here carries each session through the `ssh` child of C5, and
//! this module serves the card contract of [`horizon_browser_mcp::serve_stream`]
//! on it, under the identity that this computer minted for the session. A frame
//! only sends the set of sessions to one supervisor thread, which binds, joins
//! and removes the sockets; dropping it ends every socket and the thread. The
//! design is `docs/architecture/machine-agent-cards.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

use horizon_core::node_cards;
use tokio::sync::Semaphore;
use tokio::sync::mpsc::UnboundedSender;

/// Most card connections that one socket serves at the same time. A machine that
/// opens more drops them, so one agent cannot fill this window.
const MAX_CONNECTIONS: usize = 4;

/// One set of sessions to serve, with the identity of the host that serves them.
type Wanted = (Vec<(String, String)>, String);

/// The card sockets of the sessions of the machines of the list.
pub(in crate::app) struct NodeCards {
    /// The temporary folder whose folder holds the sockets of this process.
    base: PathBuf,
    wanted: Option<UnboundedSender<Wanted>>,
    supervisor: Option<JoinHandle<()>>,
}

impl Default for NodeCards {
    fn default() -> Self {
        Self {
            base: std::env::temp_dir(),
            wanted: None,
            supervisor: None,
        }
    }
}

impl Drop for NodeCards {
    fn drop(&mut self) {
        self.stop_all();
    }
}

impl NodeCards {
    /// Serves exactly the sockets of `sessions`, whose every connection is
    /// `host_instance` beside the actor of its session. The frame sends the set
    /// and returns; the supervisor binds, joins and removes the sockets.
    pub(in crate::app) fn set_sessions(&mut self, sessions: &[(String, String)], host_instance: &str) {
        if self.wanted.is_none() {
            let (wanted, orders) = tokio::sync::mpsc::unbounded_channel();
            let base = self.base.clone();
            match std::thread::Builder::new()
                .name("horizon-card-sockets".to_owned())
                .spawn(move || supervise(orders, &base))
            {
                Ok(supervisor) => {
                    self.wanted = Some(wanted);
                    self.supervisor = Some(supervisor);
                }
                Err(error) => {
                    tracing::warn!(%error, "no thread for the card sockets of the machines");
                    return;
                }
            }
        }
        if let Some(wanted) = &self.wanted {
            let _ = wanted.send((sessions.to_vec(), host_instance.to_owned()));
        }
    }

    /// Ends every socket, removes every socket file and joins the supervisor.
    pub(in crate::app) fn stop_all(&mut self) {
        self.wanted = None;
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.join();
        }
    }
}

/// One bound socket of one session: the loop that accepts on it, and its stop.
struct Bound {
    socket: PathBuf,
    stop: tokio::sync::oneshot::Sender<()>,
    accepts: tokio::task::JoinHandle<()>,
}

/// The loop of the supervisor: serve the set that each message names, and end
/// every socket when the window drops its sender.
fn supervise(orders: tokio::sync::mpsc::UnboundedReceiver<Wanted>, base: &Path) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        return;
    };
    let folder = node_cards::local_dir(base);
    runtime.block_on(async {
        let mut bound: BTreeMap<String, Bound> = BTreeMap::new();
        let mut orders = orders;
        while let Some((sessions, host_instance)) = orders.recv().await {
            serve_set(&folder, &sessions, &host_instance, &mut bound).await;
        }
        for (_, socket) in std::mem::take(&mut bound) {
            end(socket).await;
        }
    });
    drop(runtime);
    let _ = std::fs::remove_dir_all(&folder);
}

/// The sockets of exactly `sessions`: a new session binds one, a session that
/// left loses its socket, and every other socket stays.
async fn serve_set(
    folder: &Path,
    sessions: &[(String, String)],
    host_instance: &str,
    bound: &mut BTreeMap<String, Bound>,
) {
    let mut keep = BTreeSet::new();
    for (machine, session) in sessions {
        let Ok(actor) = node_cards::actor(machine, session) else {
            continue;
        };
        keep.insert(actor.clone());
        if bound.contains_key(&actor) {
            continue;
        }
        match bind(folder, machine, session, &actor, host_instance) {
            Ok(socket) => {
                bound.insert(actor, socket);
            }
            Err(error) => tracing::warn!(%error, "a card socket of a session did not bind"),
        }
    }
    let left: Vec<String> = bound.keys().filter(|actor| !keep.contains(*actor)).cloned().collect();
    for actor in left {
        if let Some(socket) = bound.remove(&actor) {
            end(socket).await;
        }
    }
    if bound.is_empty() {
        let _ = std::fs::remove_dir_all(folder);
    }
}

/// One socket, bound and served, for `actor` of this host.
fn bind(folder: &Path, machine: &str, session: &str, actor: &str, host_instance: &str) -> Result<Bound, String> {
    let socket = node_cards::local_socket(folder, machine, session).map_err(|refusal| refusal.message)?;
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let _ = std::fs::remove_file(&socket);
    let listener = tokio::net::UnixListener::bind(&socket).map_err(|error| format!("{}: {error}", socket.display()))?;
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let accepts = tokio::spawn(accept(
        listener,
        (actor.to_owned(), host_instance.to_owned()),
        Arc::new(Semaphore::new(MAX_CONNECTIONS)),
        stopped,
    ));
    Ok(Bound { socket, stop, accepts })
}

/// Accepts on one socket until its stop, and serves each connection with the
/// card contract of the session.
async fn accept(
    listener: tokio::net::UnixListener,
    (actor, host_instance): (String, String),
    permits: Arc<Semaphore>,
    mut stopped: tokio::sync::oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = &mut stopped => break,
            accepted = listener.accept() => {
                let Ok((stream, _address)) = accepted else { break };
                let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                    continue;
                };
                let (actor, host_instance) = (actor.clone(), host_instance.clone());
                tokio::spawn(async move {
                    let _permit = permit;
                    let _ = horizon_browser_mcp::serve_stream(&actor, &host_instance, stream).await;
                });
            }
        }
    }
}

/// Ends one socket: its loop stops, its task ends and its file goes.
async fn end(bound: Bound) {
    let Bound { socket, stop, accepts } = bound;
    let _ = stop.send(());
    let _ = accepts.await;
    let _ = std::fs::remove_file(&socket);
}

#[cfg(test)]
mod tests;
