use super::{RemotePanelStatus, RemotePanelStatusError as Error};
use crate::cloud_run::CloudJobId;
use serde::{Deserialize, Serialize};

pub(super) const RESPONSE_LIMIT: usize = 4096;

pub(super) fn request(runtime: CloudJobId, panel: &str) -> Result<Vec<u8>, Error> {
    #[derive(Serialize)]
    struct Request<'a> {
        version: u8,
        operation: &'static str,
        runtime: CloudJobId,
        panel: &'a str,
    }
    serde_json::to_vec(&Request {
        version: 1,
        operation: "status",
        runtime,
        panel,
    })
    .map_err(|_| Error::QueryFailed)
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
