//! Journal exact additional panel intent; explicit retries never start a task.

use super::{
    Error, input,
    storage::{self, Context},
};
use horizon_core::{
    PanelKind,
    cloud_run::interactive_worker::InteractiveWorker,
    remote_workspace::{
        RemotePanelBinding, RemotePanelCommand,
        panels::{self, RemoteShellPanelDraft},
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    operation_id: uuid::Uuid,
    command: RemotePanelCommand,
    directory: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u8,
    operation_id: uuid::Uuid,
    session: String,
    workspace: String,
    generation: u64,
    original_revision: u64,
    worker: InteractiveWorker,
    binding: RemotePanelBinding,
}

#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum Claim {
    Current(Box<Journal>),
    Legacy { panel: String, operation_id: uuid::Uuid },
}

fn same_intent(binding: &RemotePanelBinding, request: &Request) -> bool {
    binding.kind == PanelKind::Shell
        && binding.command.as_ref() == Some(&request.command)
        && binding.working_directory.as_deref() == Some(request.directory.as_str())
        && binding.task_handoff.is_none()
        && binding.agent_session_id.is_none()
}

pub(super) fn add(context: &Context) -> Result<Value, Error> {
    let request: Request = serde_json::from_str(&input(65_536)?).map_err(|_| Error::Input)?;
    add_request(context, &request)
}

fn add_request(context: &Context, request: &Request) -> Result<Value, Error> {
    if request.operation_id.is_nil() {
        return Err(Error::Input);
    }
    let store = context.store()?;
    let saved = context.saved()?;
    let expected = saved.environment_summary();
    let path = context.receipt.root.join(format!("add-{}.json", request.operation_id));
    let existing = match std::fs::read(&path) {
        Ok(bytes) if bytes.len() <= 131_072 => {
            Some(serde_json::from_slice::<Claim>(&bytes).map_err(|_| Error::Claimed)?)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        _ => return Err(Error::Claimed),
    };
    let draft = || RemoteShellPanelDraft {
        command: request.command.clone(),
        working_directory: Some(request.directory.clone()),
    };
    let prepared = match existing {
        Some(Claim::Legacy { panel, operation_id }) => {
            if operation_id != request.operation_id {
                return Err(Error::Claimed);
            }
            let binding = saved
                .state()
                .spec
                .panels
                .iter()
                .find(|p| p.panel_local_id == panel)
                .ok_or(Error::Claimed)?;
            if !same_intent(binding, request) {
                return Err(Error::Claimed);
            }
            return Ok(
                json!({"panel": panel, "revision": expected.revision, "status": "panel_saved", "already_saved": true}),
            );
        }
        Some(Claim::Current(journal)) => {
            validate_journal(&journal, context, &saved, request)?;
            if let Some(existing) = saved
                .state()
                .spec
                .panels
                .iter()
                .find(|p| p.panel_local_id == journal.binding.panel_local_id)
            {
                if existing != &journal.binding {
                    return Err(Error::Claimed);
                }
                return Ok(
                    json!({"panel": existing.panel_local_id, "revision": expected.revision, "status": "panel_saved", "already_saved": true}),
                );
            }
            let id = uuid::Uuid::parse_str(&journal.binding.panel_local_id).map_err(|_| Error::Claimed)?;
            let prepared =
                panels::prepare_remote_shell_panel_with_id(&store, &context.receipt.session, &expected, draft(), id)
                    .map_err(|error| Error::Remote(error.to_string()))?;
            if prepared.panel() != &journal.binding {
                return Err(Error::Claimed);
            }
            prepared
        }
        None => {
            let prepared = panels::prepare_remote_shell_panel(&store, &context.receipt.session, &expected, draft())
                .map_err(|error| Error::Remote(error.to_string()))?;
            let runtime = saved.state().runtime.as_ref().ok_or(Error::Operation)?;
            let journal = Journal {
                version: 1,
                operation_id: request.operation_id,
                session: context.receipt.session.clone(),
                workspace: context.receipt.workspace.clone(),
                generation: runtime.generation,
                original_revision: expected.revision,
                worker: runtime.worker.clone().ok_or(Error::Operation)?,
                binding: prepared.panel().clone(),
            };
            storage::publish_journal(&path, &serde_json::to_vec(&journal).map_err(|_| Error::Storage)?)
                .map_err(|_| Error::Claimed)?;
            prepared
        }
    };
    let added = panels::add_remote_shell_panel(&store, &context.receipt.session, &expected, prepared)
        .map_err(|error| Error::Remote(error.to_string()))?;
    Ok(
        json!({"panel": added.panel_id, "revision": added.environment.revision, "status": "panel_saved", "already_saved": false}),
    )
}

fn validate_journal(
    journal: &Journal,
    context: &Context,
    saved: &horizon_core::cloud_run::StoredRemoteWorkspace,
    request: &Request,
) -> Result<(), Error> {
    let runtime = saved.state().runtime.as_ref().ok_or(Error::Claimed)?;
    if journal.version != 1
        || journal.operation_id != request.operation_id
        || journal.session != context.receipt.session
        || journal.workspace != context.receipt.workspace
        || journal.generation != runtime.generation
        || Some(&journal.worker) != runtime.worker.as_ref()
        || journal.original_revision > saved.environment_summary().revision
        || !same_intent(&journal.binding, request)
    {
        return Err(Error::Claimed);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use horizon_core::{
        HorizonHome,
        cloud_run::{CloudWorkflowStore, interactive_worker::*},
        remote_workspace::RemoteWorkspaceState,
    };

    fn fixture(root: &std::path::Path) -> Context {
        let home = HorizonHome::from_root(root.join("home"));
        let store = CloudWorkflowStore::open(&home).unwrap();
        let state: RemoteWorkspaceState = serde_json::from_value(json!({"version":1, "spec":{
            "workspace_local_id":"workspace", "working_directory":"nested", "generation":0,
            "target":{"provider":"local_docker", "profile":"development", "disk_gib":20,
                "lifetime":"persistent", "image":format!("example/worker@sha256:{}", "a".repeat(64))},
            "repository":{"repository":"example/project", "commit":"b".repeat(40), "branch":"work/example"},
            "panels":[{"panel_local_id":"original", "kind":"shell", "command":{"program":"/bin/sh", "args":[]}}]
        }}))
        .unwrap();
        let session = "00000000-0000-4000-8000-000000000001";
        let saved = store.create_remote_workspace(session, &state).unwrap();
        let allocation = store.allocate_remote_runtime(&saved, i64::MAX).unwrap();
        let mut bytes = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
        bytes.extend([7; 32]);
        let public_key = format!("ssh-ed25519 {}", STANDARD.encode(bytes));
        let allocation = store.reserve_remote_worker_request(&allocation, &public_key).unwrap();
        let request = allocation.worker_request().unwrap();
        let status = InteractiveWorkerStatus {
            worker: InteractiveWorker {
                identity: InteractiveWorkerIdentity {
                    provider: request.target.provider,
                    workflow_id: request.workflow_id,
                    job_id: request.job_id,
                    resource_id: "synthetic-worker".into(),
                },
                target: request.target,
                ssh_public_key: request.ssh_public_key,
                lifetime: InteractiveWorkerLifetime::Persistent,
            },
            lifecycle: InteractiveWorkerLifecycle::Provisioning,
            ssh: Some(InteractiveWorkerSshEndpoint {
                host: "127.0.0.1".into(),
                port: 2222,
                username: "horizon".into(),
                host_key: public_key,
            }),
        };
        let mut recovered = allocation.workspace().state().clone();
        let runtime = recovered.runtime.as_mut().unwrap();
        runtime.worker = Some(status.worker);
        runtime.ssh = status.ssh;
        runtime.phase = horizon_core::remote_workspace::RemoteRuntimePhase::Reconciling;
        store
            .replace_remote_workspace(allocation.workspace(), &recovered)
            .unwrap();
        let receipt = storage::Receipt {version:1, root:root.into(), session:session.into(),workspace:"workspace".into(),panel:"original".into(),
            intent: serde_json::from_value(json!({"config":{},"target":state.spec.target,"repository":state.spec.repository,
            "working_directory":"nested","command":{"program":"/bin/sh","args":[]},"setup_expires_at_millis":1,"issue":"fixture"})).unwrap()};
        Context::new(root, receipt, storage::lock(root).unwrap())
    }

    fn request(id: uuid::Uuid) -> Request {
        Request {
            operation_id: id,
            command: RemotePanelCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "printf independent".into()],
            },
            directory: "nested/second".into(),
        }
    }

    #[test]
    fn interrupted_save_recovers_original_identity_and_retries_observe_it() {
        let root = tempfile::tempdir().unwrap();
        let context = fixture(root.path());
        let id = uuid::Uuid::new_v4();
        let intent = request(id);
        let saved = context.saved().unwrap();
        let prepared = panels::prepare_remote_shell_panel(
            &context.store().unwrap(),
            &context.receipt.session,
            &saved.environment_summary(),
            RemoteShellPanelDraft {
                command: intent.command.clone(),
                working_directory: Some(intent.directory.clone()),
            },
        )
        .unwrap();
        let runtime = saved.state().runtime.as_ref().unwrap();
        let journal = Journal {
            version: 1,
            operation_id: id,
            session: context.receipt.session.clone(),
            workspace: context.receipt.workspace.clone(),
            generation: runtime.generation,
            original_revision: saved.environment_summary().revision,
            worker: runtime.worker.clone().unwrap(),
            binding: prepared.panel().clone(),
        };
        storage::write_new(
            &root.path().join(format!("add-{id}.json")),
            &serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();
        // Simulate interruption after durable journal but before the local database CAS.
        assert_eq!(context.saved().unwrap().state().spec.panels.len(), 1);
        let result = add_request(&context, &request(id)).unwrap();
        assert_eq!(result["panel"], journal.binding.panel_local_id);
        assert_eq!(result["already_saved"], false);
        assert_eq!(add_request(&context, &request(id)).unwrap()["already_saved"], true);
        assert_eq!(context.saved().unwrap().state().spec.panels.len(), 2);
        let mut different = request(id);
        different.directory = "other".into();
        assert!(matches!(add_request(&context, &different), Err(Error::Claimed)));
        let mut wrong = journal;
        wrong.generation += 1;
        assert!(matches!(
            validate_journal(&wrong, &context, &context.saved().unwrap(), &request(id)),
            Err(Error::Claimed)
        ));
        assert_eq!(context.saved().unwrap().state().spec.panels.len(), 2);
    }

    #[test]
    fn legacy_claim_can_only_observe_an_existing_matching_panel() {
        let root = tempfile::tempdir().unwrap();
        let context = fixture(root.path());
        let id = uuid::Uuid::new_v4();
        let result = add_request(&context, &request(id)).unwrap();
        let legacy = uuid::Uuid::new_v4();
        let path = root.path().join(format!("add-{legacy}.json"));
        storage::write_new(
            &path,
            &serde_json::to_vec(&json!({"operation_id":legacy,"panel":result["panel"]})).unwrap(),
        )
        .unwrap();
        assert_eq!(add_request(&context, &request(legacy)).unwrap()["already_saved"], true);
        let missing = uuid::Uuid::new_v4();
        storage::write_new(
            &root.path().join(format!("add-{missing}.json")),
            &serde_json::to_vec(&json!({"operation_id":missing,"panel":"missing"})).unwrap(),
        )
        .unwrap();
        assert!(matches!(add_request(&context, &request(missing)), Err(Error::Claimed)));
        assert_eq!(context.saved().unwrap().state().spec.panels.len(), 2);
    }
}
