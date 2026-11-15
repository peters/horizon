//! The card sockets of the sessions of the machines of the machine list, and the
//! `ssh` child that carries them from those machines to here.
//!
//! One Unix socket here carries each session, and this module serves the card
//! contract of [`horizon_browser_mcp::serve_stream`] on it, under the identity
//! that this computer minted for the session. One `ssh` child for each machine
//! forwards the socket of every session of that machine to it. A frame only sends
//! the set of machines to one supervisor thread, which binds, joins and removes the
//! sockets and starts, replaces and stops the children; dropping it ends every
//! socket, every child and the thread. The design is
//! `docs/architecture/machine-agent-cards.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use horizon_core::machine_watch::{MAX_REFUSAL_BYTES, WatchRefusal};
use horizon_core::node_cards;
use horizon_core::{RECONNECT_RESET_AFTER, SshConnection, reconnect_delay};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::{Semaphore, oneshot};

/// Most card connections that one socket serves at the same time. A machine that
/// opens more drops them, so one agent cannot fill this window.
const MAX_CONNECTIONS: usize = 4;

/// The program of the `ssh` child of the cards: the `ssh` of the `PATH`.
const SSH_PROGRAM: &str = "ssh";

/// How long the removal of the stale socket files of a machine may take.
const PRUNE_TIMEOUT: Duration = Duration::from_secs(10);

/// One machine that has its cards set up: the login of its `ssh` child, the sessions
/// that the watch reports there and the card link of this window.
#[derive(Clone)]
pub(in crate::app) struct CardMachine {
    pub(in crate::app) name: String,
    pub(in crate::app) login: SshConnection,
    pub(in crate::app) sessions: Vec<String>,
    pub(in crate::app) links: Links,
}

/// Why the card link of each machine whose `ssh` child is gone ended, in its words.
pub(in crate::app) type Links = Arc<Mutex<BTreeMap<String, String>>>;

/// One set of machines to serve, with the identity of the host that serves their
/// sockets.
type Wanted = (Vec<CardMachine>, String);

/// The card sockets of the sessions of the machines of the list, and their
/// `ssh` children.
pub(in crate::app) struct NodeCards {
    /// The temporary folder whose folder holds the sockets of this process.
    pub(in crate::app) base: PathBuf,
    /// The program of the `ssh` children.
    pub(in crate::app) program: String,
    wanted: Option<UnboundedSender<Wanted>>,
    supervisor: Option<JoinHandle<()>>,
    /// The card link of each machine whose child is gone, for the line of the sidebar.
    pub(in crate::app) links: Links,
}

impl Default for NodeCards {
    fn default() -> Self {
        Self {
            base: std::env::temp_dir(),
            program: SSH_PROGRAM.to_owned(),
            wanted: None,
            supervisor: None,
            links: Links::default(),
        }
    }
}

impl Drop for NodeCards {
    fn drop(&mut self) {
        self.stop_all();
    }
}

impl NodeCards {
    /// Serves exactly `machines`, whose every connection is `host_instance` beside
    /// the actor of its session. The frame sends the set and returns; the
    /// supervisor binds, joins, starts and stops.
    pub(in crate::app) fn set_sessions(&mut self, machines: Vec<CardMachine>, host_instance: &str) {
        if self.wanted.is_none() {
            let (wanted, orders) = tokio::sync::mpsc::unbounded_channel();
            let (base, program) = (self.base.clone(), self.program.clone());
            match std::thread::Builder::new()
                .name("horizon-card-sockets".to_owned())
                .spawn(move || supervise(orders, &base, &program))
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
            let _ = wanted.send((machines, host_instance.to_owned()));
        }
    }

    /// The words of the refusal that ended the card link of `machine`, while it waits.
    pub(in crate::app) fn refusal(&self, machine: &str) -> Option<String> {
        let links = self.links.lock().unwrap_or_else(PoisonError::into_inner);
        links.get(machine).cloned()
    }

    /// Ends every socket, stops every child, removes every socket file and joins
    /// the supervisor.
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
    stop: oneshot::Sender<()>,
    accepts: tokio::task::JoinHandle<()>,
}

/// One `ssh` child of one machine: the argv it carries, the loop that restarts it
/// and its stop.
struct Carried {
    args: Vec<String>,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

/// The loop of the supervisor: serve the set that each message names, and end
/// every socket and child when the window drops its sender.
fn supervise(orders: tokio::sync::mpsc::UnboundedReceiver<Wanted>, base: &Path, program: &str) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        return;
    };
    let folder = node_cards::local_dir(base);
    let program = program.to_owned();
    runtime.block_on(async {
        let mut bound: BTreeMap<String, Bound> = BTreeMap::new();
        let mut carried: BTreeMap<String, Carried> = BTreeMap::new();
        let mut orders = orders;
        while let Some((machines, host_instance)) = orders.recv().await {
            apply_set(&folder, &program, &machines, &host_instance, &mut bound, &mut carried).await;
        }
        for (_, socket) in std::mem::take(&mut bound) {
            end(socket).await;
        }
        for (_, child) in std::mem::take(&mut carried) {
            stop(child).await;
        }
    });
    drop(runtime);
    let _ = std::fs::remove_dir_all(&folder);
}

/// One pass over the machines: a new session binds a socket, a machine whose
/// forwards changed ends its child before the next one starts, and a session or a
/// machine that left loses its socket or its child.
async fn apply_set(
    folder: &Path,
    program: &str,
    machines: &[CardMachine],
    host_instance: &str,
    bound: &mut BTreeMap<String, Bound>,
    carried: &mut BTreeMap<String, Carried>,
) {
    let (mut actors, mut names) = (BTreeSet::new(), BTreeSet::new());
    for machine in machines {
        names.insert(machine.name.as_str());
        let mut forwards = Vec::new();
        for session in &machine.sessions {
            let Ok(actor) = node_cards::actor(&machine.name, session) else {
                continue;
            };
            let (Ok(remote), Ok(local)) = (
                node_cards::remote_socket(&machine.name, session),
                node_cards::local_socket(folder, &machine.name, session),
            ) else {
                continue;
            };
            forwards.push((remote, local));
            if !bound.contains_key(&actor) {
                match bind(folder, &machine.name, session, &actor, host_instance) {
                    Ok(socket) => {
                        bound.insert(actor.clone(), socket);
                    }
                    Err(error) => tracing::warn!(%error, "a card socket of a session did not bind"),
                }
            }
            actors.insert(actor);
        }
        let args = node_cards::forward_args(&machine.login, &forwards);
        if carried.get(&machine.name).is_some_and(|child| child.args == args) {
            continue;
        }
        if let Some(child) = carried.remove(&machine.name) {
            stop(child).await;
        }
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(carry(machine.clone(), program.to_owned(), args.clone(), stopped));
        carried.insert(machine.name.clone(), Carried { args, stop, task });
    }
    let left: Vec<String> = bound.keys().filter(|actor| !actors.contains(*actor)).cloned().collect();
    for actor in left {
        if let Some(socket) = bound.remove(&actor) {
            end(socket).await;
        }
    }
    let left: Vec<String> = carried
        .keys()
        .filter(|name| !names.contains(name.as_str()))
        .cloned()
        .collect();
    for name in left {
        if let Some(child) = carried.remove(&name) {
            stop(child).await;
        }
    }
    if bound.is_empty() {
        let _ = std::fs::remove_dir_all(folder);
    }
}

/// Runs the `ssh` child of one machine until its stop: an end of the child waits
/// the delay of [`reconnect_delay`] and starts the next attempt, and the words of
/// `ssh` are read for the refusal that they name.
async fn carry(machine: CardMachine, program: String, args: Vec<String>, mut stop: oneshot::Receiver<()>) {
    let remotes: Vec<String> = machine
        .sessions
        .iter()
        .filter_map(|session| node_cards::remote_socket(&machine.name, session).ok())
        .collect();
    let prune = node_cards::prune_args(&machine.login, &remotes);
    let mut attempts = 0u32;
    loop {
        {
            let mut lost = machine.links.lock().unwrap_or_else(PoisonError::into_inner);
            lost.remove(&machine.name);
        }
        // The child before this one left a socket file for each of these paths.
        prune_stale(&program, &prune).await;
        let started = Instant::now();
        let mut child = match tokio::process::Command::new(&program)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                tracing::warn!(machine = %machine.name, %error, "no ssh child for the card sockets of a machine");
                return;
            }
        };
        let words = child.stderr.take().map(|pipe| {
            tokio::spawn(async move {
                let mut words = Vec::new();
                let _ = pipe.take(MAX_REFUSAL_BYTES as u64).read_to_end(&mut words).await;
                String::from_utf8_lossy(&words).into_owned()
            })
        });
        tokio::select! {
            _ = &mut stop => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return;
            }
            _ = child.wait() => {}
        }
        // A child that ends is a lost link of the cards of that machine, and the
        // words of `ssh` name why. No card of it is served while it is gone.
        let words = match words {
            Some(handle) => handle.await.unwrap_or_default(),
            None => String::new(),
        };
        let refused = WatchRefusal::of_ssh(&words);
        {
            let mut lost = machine.links.lock().unwrap_or_else(PoisonError::into_inner);
            lost.insert(machine.name.clone(), refused.sentence_with(&words));
        }
        tracing::warn!(machine = %machine.name, refusal = ?refused, words = %words, "the card link of a machine ended");
        if started.elapsed() >= RECONNECT_RESET_AFTER {
            attempts = 0;
        }
        attempts = attempts.saturating_add(1);
        tokio::select! {
            _ = &mut stop => return,
            () = tokio::time::sleep(reconnect_delay(attempts)) => {}
        }
    }
}

/// Ends one socket: its loop stops, its task ends and its file goes; a child that
/// carries it kills it and returns.
async fn end(bound: Bound) {
    let Bound { socket, stop, accepts } = bound;
    let _ = stop.send(());
    let _ = accepts.await;
    let _ = std::fs::remove_file(&socket);
}

/// Removes the socket files that the child of a machine is about to bind, in one
/// bounded call of the login of that machine. A machine that does not answer opens its
/// child all the same: the bind of that child then names the refusal.
async fn prune_stale(program: &str, args: &[String]) {
    let mut call = tokio::process::Command::new(program);
    call.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if tokio::time::timeout(PRUNE_TIMEOUT, call.status()).await.is_err() {
        tracing::warn!("the socket files of a machine were not removed before its child");
    }
}

/// Ends one `ssh` child, so no child outlives the stop of this window.
async fn stop(child: Carried) {
    let Carried { stop, task, .. } = child;
    let _ = stop.send(());
    let _ = task.await;
}

/// One socket, bound and served, for `actor` of this host.
fn bind(folder: &Path, machine: &str, session: &str, actor: &str, host_instance: &str) -> Result<Bound, String> {
    let socket = node_cards::local_socket(folder, machine, session).map_err(|refusal| refusal.message)?;
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let _ = std::fs::remove_file(&socket);
    let listener = tokio::net::UnixListener::bind(&socket).map_err(|error| format!("{}: {error}", socket.display()))?;
    let (stop, stopped) = oneshot::channel();
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

#[cfg(test)]
mod tests;
