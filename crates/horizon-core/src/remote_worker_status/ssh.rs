use super::{RemotePanelStatusError as Error, command};
use crate::{
    cloud_run::interactive_worker::InteractiveWorkerSshEndpoint,
    remote_ssh_identity::RemoteSshIdentity,
    remote_worker_ssh::{HOST_ALIAS, prepared_command},
};
use std::{io::Write, time::Duration};

const DEADLINE: Duration = Duration::from_secs(10);

pub(super) fn request(
    identity: &RemoteSshIdentity,
    endpoint: &InteractiveWorkerSshEndpoint,
    input: &[u8],
) -> Result<Vec<u8>, Error> {
    // The recovered key's private parent avoids ambient temporary-directory trust.
    let parent = identity.private_key_path().parent().ok_or(Error::UnsupportedPath)?;
    let mut known_hosts = tempfile::NamedTempFile::new_in(parent).map_err(|_| Error::TrustStorage)?;
    writeln!(known_hosts, "{HOST_ALIAS} {}", endpoint.host_key).map_err(|_| Error::TrustStorage)?;
    let command = prepared_command(identity.private_key_path(), known_hosts.path(), endpoint)?;
    command::run(command, input, DEADLINE)
}
