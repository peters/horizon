use std::time::{Duration, Instant};

use horizon_browser_control::block::parse_sent_document;
use horizon_browser_control::manifest::block::{self, Operation, Outcome};
use horizon_browser_control::manifest::block_keys;
use horizon_browser_control::manifest::block_machines;
use horizon_browser_control::manifest::block_preset;
use horizon_browser_control::manifest::block_proposal;
use horizon_browser_control::manifest::block_terminal;
use horizon_browser_control::manifest::block_workers;

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
        if !operation.is_card() && self.cards_only {
            return Ok(Outcome::failed(
                block::CARDS_ONLY,
                "A card socket of a machine session serves the card operations of block_panel and no other operation",
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
        if let Some(refusal) = refused_worker(&operation) {
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
        | Operation::ServerStatus
        | Operation::BrowserHostStatus
        | Operation::SecretNames
        | Operation::Presets
        | Operation::Machines
        | Operation::MachineKey { .. }
        | Operation::MachineKeys
        | Operation::DeleteMachineKey { .. }
        | Operation::ServerLinkRequest { .. }
        | Operation::BrowserHostRequest { .. }
        | Operation::WorkItems
        | Operation::WorkItemMessage { .. }
        | Operation::WorkerStart { .. }
        | Operation::Workers
        | Operation::WorkerMessage { .. }
        | Operation::WorkerReport { .. } => {
            return None;
        }
        Operation::WorkItemPropose { items } => {
            // The items of a proposal are text of a program that becomes a
            // workspace, so their bounds are checked before the queue holds them.
            return block_proposal::check_items(items)
                .err()
                .map(|refusal| Outcome::failed(refusal.code, &refusal.message));
        }
    };
    parse_sent_document(document)
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
        | Operation::ServerStatus
        | Operation::BrowserHostStatus
        | Operation::SecretNames
        | Operation::Presets
        | Operation::Machines
        | Operation::ServerLinkRequest { .. }
        | Operation::BrowserHostRequest { .. }
        | Operation::WorkItems
        | Operation::WorkItemPropose { .. }
        | Operation::WorkItemMessage { .. }
        | Operation::WorkerStart { .. }
        | Operation::Workers
        | Operation::WorkerMessage { .. }
        | Operation::WorkerReport { .. }
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

/// The task, the message and the report of one worker operation, checked before
/// the queue holds them: each is text of a program that becomes one line of a
/// terminal or one turn of a conversation, so a text outside its bound is refused
/// at once and no request file is written.
fn refused_worker(operation: &Operation) -> Option<Outcome> {
    let checked = match operation {
        Operation::WorkerStart { task, .. } => block_workers::check_task(task),
        Operation::WorkerMessage { text, .. } => block_workers::check_message(text),
        Operation::WorkerReport { text } => block_workers::check_report(text),
        _ => return None,
    };
    checked
        .err()
        .map(|refusal| Outcome::failed(refusal.code, &refusal.message))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn created(document: serde_json::Value) -> Operation {
        Operation::Create { title: None, document }
    }

    fn code(outcome: Option<Outcome>) -> String {
        match outcome {
            Some(Outcome::Failed { code, .. }) => code,
            other => panic!("expected a refusal: {other:?}"),
        }
    }

    /// The document of a card socket of a machine session is read here, before
    /// the queue holds it, exactly as the host reads it later.
    #[test]
    fn a_document_that_arrives_as_the_text_of_the_document_is_the_document() {
        let document = serde_json::json!({
            "components": [{"type": "heading", "text": "note"}]
        });
        let text = serde_json::Value::String(serde_json::to_string(&document).expect("the text of a document"));
        assert!(refused_document(&created(document)).is_none(), "the object form");
        assert!(
            refused_document(&created(text)).is_none(),
            "the text of the document is the document"
        );
        assert_eq!(
            code(refused_document(&created(serde_json::Value::String(
                "not a document".to_owned()
            )))),
            "invalid_document",
            "a text that is no document is still refused"
        );
    }

    #[test]
    fn a_worker_text_outside_its_bound_is_refused_before_the_queue_holds_it() {
        let start = |task: &str| Operation::WorkerStart {
            program: "Codex".into(),
            task: task.into(),
        };
        assert!(refused_worker(&start("fix it")).is_none());
        assert_eq!(
            code(refused_worker(&start(" "))),
            "invalid_request",
            "an empty task starts no worker"
        );
        let long = "t".repeat(block_workers::MAX_WORKER_TEXT_CHARS + 1);
        assert_eq!(code(refused_worker(&start(&long))), "invalid_request");
        assert_eq!(
            code(refused_worker(&Operation::WorkerMessage {
                worker: "panel".into(),
                text: String::new(),
            })),
            "invalid_request"
        );
        assert_eq!(
            code(refused_worker(&Operation::WorkerReport { text: long })),
            "invalid_request"
        );
        assert!(
            refused_worker(&Operation::Workers).is_none(),
            "the read of the workers takes no text"
        );
    }
}
