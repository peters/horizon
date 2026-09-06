use super::{RemotePanelStatus, RemotePanelStatusError as Error};
use crate::{PanelKind, cloud_run::CloudJobId, remote_workspace::RemoteWorkspaceSpec};
use serde::{Deserialize, Serialize};

pub(super) const RESPONSE_LIMIT: usize = 4096;
const REQUEST_LIMIT: usize = 512 * 1024;
const REQUEST_VERSION: u8 = 1;

pub(super) fn request(runtime: CloudJobId, panel: &str) -> Result<Vec<u8>, Error> {
    #[derive(Serialize)]
    struct Request<'a> {
        version: u8,
        operation: &'static str,
        runtime: CloudJobId,
        panel: &'a str,
    }
    encode_request(&Request {
        version: REQUEST_VERSION,
        operation: "status",
        runtime,
        panel,
    })
}

pub(super) fn intent_request(runtime: CloudJobId, spec: &RemoteWorkspaceSpec, panel: &str) -> Result<Vec<u8>, Error> {
    #[derive(Serialize)]
    struct Request<'a> {
        version: u8,
        operation: &'static str,
        runtime: CloudJobId,
        panel: &'a str,
        directory: &'a str,
        argv: Vec<&'a str>,
    }
    let binding = spec
        .panels
        .iter()
        .find(|binding| binding.panel_local_id == panel)
        .ok_or(Error::UnknownPanel)?;
    if !matches!(binding.kind, PanelKind::Shell | PanelKind::Command)
        || binding.task_handoff.is_some()
        || binding.agent_session_id.is_some()
    {
        return Err(Error::UnsupportedIntent);
    }
    let command = binding.command.as_ref().ok_or(Error::UnsupportedIntent)?;
    let directory = binding.working_directory.as_deref().unwrap_or(&spec.working_directory);
    let mut argv = Vec::with_capacity(1 + command.args.len());
    argv.push(command.program.as_str());
    argv.extend(command.args.iter().map(String::as_str));
    encode_request(&Request {
        version: REQUEST_VERSION,
        operation: "verify",
        runtime,
        panel,
        directory,
        argv,
    })
}

fn encode_request(request: &impl Serialize) -> Result<Vec<u8>, Error> {
    let bytes = serde_json::to_vec(request).map_err(|_| Error::QueryFailed)?;
    if bytes.len() > REQUEST_LIMIT {
        return Err(Error::UnsupportedIntent);
    }
    Ok(bytes)
}

#[derive(Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum Response {
    Running {
        panel: String,
        pid: u32,
        #[serde(deserialize_with = "Option::deserialize")]
        exit_status: Option<u8>,
    },
    Exited {
        panel: String,
        pid: u32,
        #[serde(deserialize_with = "Option::deserialize")]
        exit_status: Option<u8>,
    },
    Unavailable {
        panel: String,
    },
}

pub(super) fn response(bytes: &[u8], expected: &str) -> Result<RemotePanelStatus, Error> {
    if bytes.len() > RESPONSE_LIMIT {
        return Err(Error::InvalidResponse);
    }
    match serde_json::from_slice::<Response>(bytes).map_err(|_| Error::InvalidResponse)? {
        Response::Running {
            panel,
            pid,
            exit_status: None,
        } if panel == expected && pid != 0 => Ok(RemotePanelStatus::Running { pid }),
        Response::Exited {
            panel,
            pid,
            exit_status,
        } if panel == expected && pid != 0 => Ok(RemotePanelStatus::Exited { pid, exit_status }),
        Response::Unavailable { panel } if panel == expected => Ok(RemotePanelStatus::Unavailable),
        _ => Err(Error::InvalidResponse),
    }
}
