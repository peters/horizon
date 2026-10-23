//! A rented builder: a small Hetzner server with Docker on which setup proofs build their
//! images and run the CI, so the person's own computer does not. Horizon logs in as root
//! with a key it made for this builder only, and the server's SSH host key is made by
//! Horizon too, so the first connection already checks it. The server holds no secret of
//! the person, and Horizon deletes it, with its key, when no setup uses it.
use super::{
    Hetzner,
    servers::{Placement, Server, ServerRequest},
};
use crate::{Cancellation, CloudError, CreateState, Reason};
use serde::Serialize;

/// The Hetzner image with Docker installed.
const IMAGE: &str = "docker-ce";

/// The SSH host key Horizon made for a builder, in OpenSSH form.
pub struct HostKey<'a> {
    pub private: &'a str,
    pub public: &'a str,
}

/// What Horizon rents a builder with.
pub struct Request<'a> {
    pub operation_id: &'a str,
    /// Tried in order; only a capacity refusal moves on to the next one.
    pub placements: &'a [Placement],
    /// The only public key that logs in, as root.
    pub login_key: &'a str,
    pub host_key: HostKey<'a>,
}

/// Named as cloud-init names its keys.
#[derive(Serialize)]
#[allow(clippy::struct_field_names)]
struct CloudConfig<'a> {
    ssh_pwauth: bool,
    ssh_deletekeys: bool,
    ssh_genkeytypes: [&'a str; 0],
    ssh_keys: SshKeys<'a>,
}

#[derive(Serialize)]
struct SshKeys<'a> {
    ed25519_private: &'a str,
    ed25519_public: &'a str,
}

/// The builder's user data: its SSH server uses Horizon's host key and no other, and
/// takes no password.
/// # Errors
/// Refuses keys that are not Ed25519 and user data past Hetzner's limit.
pub fn user_data(host_key: &HostKey<'_>) -> Result<zeroize::Zeroizing<String>, CloudError> {
    let public = host_key.public.trim();
    if !public.starts_with("ssh-ed25519 ")
        || public.contains('\n')
        || !host_key.private.contains("BEGIN OPENSSH PRIVATE KEY")
    {
        return Err(CloudError::Invalid(
            "A builder's host key must be an Ed25519 key in OpenSSH form",
        ));
    }
    let config = CloudConfig {
        ssh_pwauth: false,
        ssh_deletekeys: true,
        ssh_genkeytypes: [],
        ssh_keys: SshKeys {
            ed25519_private: host_key.private,
            ed25519_public: public,
        },
    };
    let yaml =
        serde_yaml::to_string(&config).map_err(|_| CloudError::Invalid("Cannot write the builder's user data"))?;
    let text = zeroize::Zeroizing::new(format!("#cloud-config\n{yaml}"));
    if text.len() > crate::host::USER_DATA_LIMIT {
        return Err(CloudError::Invalid("The builder's user data is too large"));
    }
    Ok(text)
}

impl Hetzner {
    /// Rents the builder of `request.operation_id`, or finds the one it already rented.
    /// `state` and `persist` journal the creation as for a worker, so a server whose
    /// creation was cut off is found and deleted later.
    ///
    /// A placement the project refuses, as when its shared cores are all in use, does not
    /// refuse the rent: the next placement is asked for instead. A person's project can be
    /// allowed one server type and not another, so a cheaper type that the project cannot
    /// order must not stop the builder.
    /// # Errors
    /// As [`Hetzner::ensure_ssh_key`] and [`Hetzner::ensure_server`], with the last
    /// refusal when every placement was refused.
    pub fn rent_builder(
        &self,
        request: &Request<'_>,
        state: &mut CreateState,
        cancel: &Cancellation,
        mut persist: impl FnMut(&CreateState) -> Result<(), CloudError>,
    ) -> Result<Server, CloudError> {
        let user_data = user_data(&request.host_key)?;
        let key = self.ensure_ssh_key(request.operation_id, request.login_key, cancel)?;
        let mut refusal = None;
        for placement in request.placements {
            let asked = [placement.clone()];
            let server = ServerRequest {
                operation_id: request.operation_id,
                placements: &asked,
                image: IMAGE,
                user_data: &user_data,
                volume: None,
                ssh_key: Some(&key),
                network: None,
            };
            match self.ensure_server(&server, state, cancel, &mut persist, |_| {}) {
                Ok(server) => return Ok(server),
                // Only a placement that made no server is followed by the next one: a
                // server the provider made and then failed to finish is bound to the
                // journal, so its reason is the rent's answer and its cleanup stays.
                Err(error @ (CloudError::Capacity(_) | CloudError::Rejected(_))) if *state == CreateState::Prepared => {
                    refusal = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(refusal.unwrap_or(CloudError::Capacity(Reason::default())))
    }

    /// Deletes the builder of `operation_id` and its login key, and proves both gone. A
    /// server whose create request had no answer is looked up by its operation label;
    /// none is found only once the request is `settled`, so long ago that a server it
    /// made would be listed.
    /// # Errors
    /// As [`Hetzner::delete_server`] and [`Hetzner::delete_ssh_key`];
    /// [`CloudError::DuplicateWorkers`] when the label names more than one server and
    /// [`CloudError::CreationUnresolved`] when it names none before the request settled.
    pub fn return_builder(
        &self,
        operation_id: &str,
        state: &mut CreateState,
        settled: bool,
        cancel: &Cancellation,
        mut persist: impl FnMut(&CreateState) -> Result<(), CloudError>,
    ) -> Result<(), CloudError> {
        if *state == CreateState::Requested {
            let found = self.find_servers(operation_id, cancel)?;
            match found.as_slice() {
                [] if settled => {}
                [] => return Err(CloudError::CreationUnresolved),
                [server] => {
                    server.verify(operation_id)?;
                    let bound = CreateState::Bound {
                        worker_id: server.id.to_string(),
                    };
                    persist(&bound)?;
                    *state = bound;
                }
                _ => return Err(CloudError::DuplicateWorkers),
            }
        }
        if matches!(state, CreateState::Bound { .. } | CreateState::Terminated { .. }) {
            self.delete_server(operation_id, state, cancel, &mut persist, |_| {})?;
        }
        self.delete_ssh_key(operation_id, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_data_pins_horizons_host_key_and_turns_passwords_off() {
        let key = HostKey {
            private: "-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----\n",
            public: "ssh-ed25519 AAAAC3Nza builder\n",
        };
        let text = user_data(&key).unwrap();
        assert!(text.starts_with("#cloud-config\n"));
        let parsed: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        assert_eq!(parsed["ssh_pwauth"], serde_yaml::Value::Bool(false));
        assert_eq!(parsed["ssh_deletekeys"], serde_yaml::Value::Bool(true));
        assert_eq!(
            parsed["ssh_genkeytypes"].as_sequence().map(Vec::len),
            Some(0),
            "no other host key"
        );
        assert_eq!(
            parsed["ssh_keys"]["ed25519_public"].as_str(),
            Some("ssh-ed25519 AAAAC3Nza builder")
        );
        assert!(
            parsed["ssh_keys"]["ed25519_private"]
                .as_str()
                .unwrap()
                .contains("BEGIN OPENSSH")
        );
    }

    #[test]
    fn only_an_ed25519_host_key_in_openssh_form_is_written() {
        let rsa = HostKey {
            private: "-----BEGIN OPENSSH PRIVATE KEY-----\n",
            public: "ssh-rsa AAAA",
        };
        assert!(user_data(&rsa).is_err());
        let two_lines = HostKey {
            private: "-----BEGIN OPENSSH PRIVATE KEY-----\n",
            public: "ssh-ed25519 AAAA\nssh-ed25519 BBBB",
        };
        assert!(user_data(&two_lines).is_err());
    }
}
