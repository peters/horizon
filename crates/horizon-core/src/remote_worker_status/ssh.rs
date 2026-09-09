use super::{RemotePanelStatusError as Error, protocol::RESPONSE_LIMIT};
use crate::{
    cloud_run::interactive_worker::InteractiveWorkerSshEndpoint,
    remote_ssh_identity::RemoteSshIdentity,
    remote_worker_ssh::{known_hosts, prepared_command, query},
};
use std::time::Duration;

const DEADLINE: Duration = Duration::from_secs(10);

pub(super) fn request(
    identity: &RemoteSshIdentity,
    endpoint: &InteractiveWorkerSshEndpoint,
    input: &[u8],
) -> Result<Vec<u8>, Error> {
    let known_hosts = known_hosts(identity, endpoint)?;
    let command = prepared_command(identity.private_key_path(), known_hosts.path(), endpoint)?;
    query::run(command, input, DEADLINE, RESPONSE_LIMIT).map_err(Error::from)
}

impl From<query::Error> for Error {
    fn from(error: query::Error) -> Self {
        match error {
            query::Error::ClientUnavailable => Self::ClientUnavailable,
            query::Error::QueryFailed => Self::QueryFailed,
            query::Error::Deadline => Self::Deadline,
            query::Error::OutputLimit => Self::InvalidResponse,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_failures_preserve_the_public_panel_error_contract() {
        for (transport, expected) in [
            (query::Error::ClientUnavailable, Error::ClientUnavailable),
            (query::Error::QueryFailed, Error::QueryFailed),
            (query::Error::Deadline, Error::Deadline),
            (query::Error::OutputLimit, Error::InvalidResponse),
        ] {
            assert_eq!(Error::from(transport), expected);
        }
    }
}
