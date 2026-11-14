use std::time::{Duration, Instant};

use horizon_browser_control::block::parse_document;
use horizon_browser_control::manifest::block::{self, Operation, Outcome};
use horizon_browser_control::manifest::block_keys;
use horizon_browser_control::manifest::block_machines;
use horizon_browser_control::manifest::block_preset;
use horizon_browser_control::manifest::block_terminal;

use super::BrowserController;

/// The bound a caller waits beyond the wait the operation itself asks for, as
/// the start of the coordination worker and the answer write are included.
const CALLER_HEADROOM: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

impl BrowserController {
    pub(crate) async fn block_panel(&self, operation: Operation) -> Result<Outcome, String> {
        let identity = self.identity();
        if !identity.workspace_scoped()
            || identity
                .host_instance
                .is_none_or(|host| !horizon_browser_control::manifest::valid_host_instance(host))
        {
            return Ok(Outcome::failed(
                "caller_unavailable",
                "block_panel requires an agent launched inside Horizon with its host identity",
            ));
        }
        if let Some(refusal) = refused_document(&operation) {
            return Ok(refusal);
        }
        if let Some(refusal) = refused_terminal(&operation) {
            return Ok(refusal);
        }
        if let Some(refusal) = refused_key(&operation) {
            return Ok(refusal);
        }
        if let Some(refusal) = refused_panel(&operation) {
            return Ok(refusal);
        }
        // A `next_click` holds its request open for the wait it asks for, so
        // both the request deadline and the caller's bound grow with it.
        let wait = operation
            .wait_timeout_millis()
            .map_or(Duration::ZERO, Duration::from_millis);
        let caller_timeout = block::HOST_APPLY_TIMEOUT + wait + CALLER_HEADROOM;
        let started = Instant::now();
        let request_timeout = block::request_timeout(&operation);
        let request = block::enqueue(self.identity(), operation, request_timeout).map_err(|error| error.to_string())?;
        loop {
            if let Some(result) = block::take_result(&request).map_err(|error| error.to_string())? {
                return Ok(result);
            }
            if started.elapsed() >= caller_timeout {
                return Ok(Outcome::failed(
                    "host_timeout",
                    &format!(
                        "The Horizon host did not answer within {} seconds. List the cards before retrying a mutation; it may have completed. Older hosts do not support block_panel.",
                        caller_timeout.as_secs()
                    ),
                ));
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// The document a request carries, checked before the queue holds it: a card is
/// untrusted agent text, and a refusal here names the offending path at once.
fn refused_document(operation: &Operation) -> Option<Outcome> {
    let document = match operation {
        Operation::Create { document, .. } | Operation::Update { document, .. } => document,
        Operation::List
        | Operation::Inspect { .. }
        | Operation::NextClick { .. }
        | Operation::Visibility { .. }
        | Operation::Close { .. }
        | Operation::OpenTerminal { .. }
        | Operation::OpenPanel { .. }
        | Operation::MachineSet { .. }
        | Operation::MachineDelete { .. }
        | Operation::Machines
        | Operation::MachineKey { .. }
        | Operation::MachineKeys
        | Operation::DeleteMachineKey { .. }
        | Operation::ServerLinkRequest { .. }
        | Operation::BrowserHostRequest { .. } => {
            return None;
        }
    };
    let encoded = serde_json::to_string(document).ok()?;
    parse_document(&encoded)
        .err()
        .map(|error| Outcome::failed(error.code(), &error.to_string()))
}

/// The target and the remote command of an `open_terminal`, and the route of a
/// `machine_set`, checked before the queue holds them: the answer names the
/// field at once and no request file is written for a terminal or a record that
/// Horizon would refuse.
fn refused_terminal(operation: &Operation) -> Option<Outcome> {
    let (ssh, remote_command, machine) = match operation {
        Operation::OpenTerminal {
            ssh,
            remote_command,
            machine,
            ..
        } => (ssh.as_ref(), remote_command.as_deref(), machine.as_deref()),
        Operation::MachineSet { ssh, .. } => (Some(ssh), None, None),
        _ => return None,
    };
    if machine.is_some()
        && let Err(refusal) = block_machines::check_single_target(ssh.is_some(), true)
    {
        return Some(Outcome::failed(refusal.code, &refusal.message));
    }
    if matches!(operation, Operation::OpenTerminal { .. })
        && let Err(refusal) = block_machines::check_single_target(ssh.is_some(), machine.is_some())
    {
        return Some(Outcome::failed(refusal.code, &refusal.message));
    }
    let ssh = ssh?;
    block_terminal::check(ssh, remote_command)
        .err()
        .map(|refusal| Outcome::failed(refusal.code, &refusal.message))
}

/// The machine name and the key name of an operation that carries one, checked
/// before the queue holds it: a name becomes a file name under the Horizon home,
/// and a refusal here writes no request file.
fn refused_key(operation: &Operation) -> Option<Outcome> {
    if let Operation::MachineSet { name, key, session, .. } = operation {
        if let Err(refusal) = block_machines::check_machine_name(name) {
            return Some(Outcome::failed(refusal.code, &refusal.message));
        }
        if let Some(refusal) = session
            .as_deref()
            .and_then(|session| block_machines::check_session_name(session).err())
        {
            return Some(Outcome::failed(refusal.code, &refusal.message));
        }
        return key
            .as_deref()
            .and_then(|key| block_keys::check_name(key).err())
            .map(|refusal| Outcome::failed(refusal.code, &refusal.message));
    }
    if let Operation::MachineDelete { name } = operation {
        return block_machines::check_machine_name(name)
            .err()
            .map(|refusal| Outcome::failed(refusal.code, &refusal.message));
    }
    if let Operation::OpenTerminal {
        machine: Some(machine), ..
    } = operation
    {
        return block_machines::check_machine_name(machine)
            .err()
            .map(|refusal| Outcome::failed(refusal.code, &refusal.message));
    }
    let name = match operation {
        Operation::MachineKey { name } | Operation::DeleteMachineKey { name } => name,
        Operation::OpenTerminal { key: Some(key), .. } => key,
        Operation::Create { .. }
        | Operation::Update { .. }
        | Operation::List
        | Operation::Inspect { .. }
        | Operation::NextClick { .. }
        | Operation::Visibility { .. }
        | Operation::Close { .. }
        | Operation::OpenTerminal { key: None, .. }
        | Operation::OpenPanel { .. }
        | Operation::MachineSet { .. }
        | Operation::MachineDelete { .. }
        | Operation::Machines
        | Operation::ServerLinkRequest { .. }
        | Operation::BrowserHostRequest { .. }
        | Operation::MachineKeys => {
            return None;
        }
    };
    block_keys::check_name(name)
        .err()
        .map(|refusal| Outcome::failed(refusal.code, &refusal.message))
}

/// The preset name, the directory and the one prompt line of an `open_panel`,
/// checked before the queue holds them: a path that cannot be an absolute folder
/// of this computer, a name that cannot be a name of the settings and a line that
/// cannot be one line of text are refused at once, and no request file is written.
/// The host resolves the preset itself, because the settings of the person are the
/// only place that keeps the names.
fn refused_panel(operation: &Operation) -> Option<Outcome> {
    let Operation::OpenPanel {
        preset,
        directory,
        prompt,
        ..
    } = operation
    else {
        return None;
    };
    block_preset::check_name(preset)
        .and_then(|_| block_preset::check_directory(directory))
        .and_then(|_| prompt.as_deref().map_or(Ok(""), block_preset::check_prompt))
        .err()
        .map(|refusal| Outcome::failed(refusal.code, &refusal.message))
}
