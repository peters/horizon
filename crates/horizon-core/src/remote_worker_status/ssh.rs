use super::{RemotePanelStatusError as Error, command};
use crate::{
    cloud_run::interactive_worker::InteractiveWorkerSshEndpoint,
    remote_ssh_identity::RemoteSshIdentity,
    remote_worker_ssh::{known_hosts, prepared_command},
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
    command::run(command, input, DEADLINE)
}
