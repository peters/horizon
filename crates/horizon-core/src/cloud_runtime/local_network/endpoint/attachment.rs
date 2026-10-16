//! One exact gateway through the unchanged worker bridge protocol. This host
//! primitive grants no owner approval; a trusted caller must authorize it first.
use super::{Error, Gateway, Receipt, State, Status};
use crate::cloud_runtime::{
    Cancellation,
    local_network::{self, Answers, Shared, session},
    ssh::Connection,
};
use horizon_cloud_protocol::local_network::{
    Nonce, PREPARE_COMMAND, Subnet,
    discovery::{Answer, Hello, Request},
};
use std::{
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex, PoisonError},
    thread::{self, JoinHandle},
};

/// Cached local gateway and actual existing helper confirmation; not service reachability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusSnapshot {
    pub gateway: Status,
    pub worker: local_network::State,
}

pub struct Attached {
    gateway: Arc<Gateway>,
    shared: Arc<Shared>,
    cancel: Cancellation,
    supervisor: Option<JoinHandle<()>>,
}

struct PrivateSsh {
    connection: Connection,
    socket: PathBuf,
    gateway: Arc<Gateway>,
}

impl session::Transport for PrivateSsh {
    fn prepare(&self) -> Command {
        self.connection.pinned_command(PREPARE_COMMAND)
    }
    fn hold(&self, nonce: &Nonce, subnet: Subnet, _unused_port: u16) -> Command {
        let mut command = Command::new("ssh");
        command
            .args(["-T", "-a", "-x", "-o", "ExitOnForwardFailure=yes", "-R"])
            .arg(format!("{}:{}", nonce.bridge_socket(), self.socket.display()))
            .args(self.connection.pinned_args())
            .arg(nonce.hold_command(subnet));
        command
    }
    fn current(&self) -> bool {
        matches!(self.gateway.status().state, State::Active)
    }
    fn bounded_input(&self) -> bool {
        true
    }
}

impl Gateway {
    /// Starts only the original pinned SSH connection against a fresh private
    /// Unix target. A caller must already hold explicit owner authorization.
    /// # Errors
    /// Refuses unsupported hosts, a revoked gateway or thread creation failure.
    pub fn attach(self, connection: Connection) -> Result<Attached, Error> {
        let socket = self.proxy.private_path().ok_or(Error::UnsupportedSource)?.to_owned();
        if !matches!(self.status().state, State::Active) {
            return Err(Error::BindingChanged);
        }
        let std::net::IpAddr::V4(ip) = self.gate.prepared.binding.address().ip() else {
            return Err(Error::UnsupportedSource);
        };
        // Legacy helper status requires a subnet but does not enforce it. Every
        // request still reaches the exact endpoint gate, never this descriptive subnet.
        let subnet = Subnet::containing(ip, 30).map_err(|_| Error::UnsupportedSource)?;
        let gateway = Arc::new(self);
        let transport = PrivateSsh {
            connection,
            socket,
            gateway: Arc::clone(&gateway),
        };
        Attached::start(gateway, transport, subnet)
    }
}

impl Attached {
    fn start(gateway: Arc<Gateway>, transport: impl session::Transport, subnet: Subnet) -> Result<Self, Error> {
        let shared = Arc::new(Shared {
            state: Mutex::new(local_network::State::Starting),
        });
        let cancel = Cancellation::default();
        let answers: Arc<dyn Answers> = Arc::new(NoDiscovery);
        let supervisor = {
            let shared = Arc::clone(&shared);
            let cancel = cancel.clone();
            thread::Builder::new()
                .name("one-service-bridge".into())
                .spawn(move || session::supervise(&transport, subnet, 0, &answers, &shared, &cancel))
                .map_err(|_| Error::StartFailed)?
        };
        Ok(Attached {
            gateway,
            shared,
            cancel,
            supervisor: Some(supervisor),
        })
    }
}

impl Attached {
    #[must_use]
    pub fn status(&self) -> StatusSnapshot {
        let gateway = self.gateway.status();
        let worker = if matches!(gateway.state, State::Active) {
            match &*self.shared.state.lock().unwrap_or_else(PoisonError::into_inner) {
                local_network::State::Starting => local_network::State::Starting,
                local_network::State::Active { proxy } => local_network::State::Active { proxy: *proxy },
                local_network::State::Reconnecting { .. } => local_network::State::Reconnecting {
                    error: "The approved endpoint's worker connection is reconnecting".into(),
                },
                local_network::State::Failed { .. } => local_network::State::Failed {
                    error: "The approved endpoint's worker connection is unavailable".into(),
                },
            }
        } else {
            local_network::State::Failed {
                error: "The original endpoint admission ended".into(),
            }
        };
        StatusSnapshot { gateway, worker }
    }

    /// # Errors
    /// A stale generation never stops this gateway or the held SSH process.
    pub fn close(&self, receipt: &Receipt) -> Result<(), Error> {
        self.gateway.revoke(receipt)?;
        self.cancel.cancel();
        Ok(())
    }
}

impl Drop for Attached {
    fn drop(&mut self) {
        let _ = self.close(&self.gateway.status().receipt);
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.join();
        }
    }
}

struct NoDiscovery;
impl Answers for NoDiscovery {
    fn hello(&self) -> Hello {
        Hello {
            discovery: 0,
            sources: Vec::new(),
            note: None,
        }
    }
    fn answer(&self, _: Request) -> Answer {
        Answer::Refused("Only the approved endpoint is shared".into())
    }
}

#[cfg(all(test, target_os = "linux"))]
pub(super) mod tests;

#[cfg(all(test, not(unix)))]
mod unsupported_tests {
    use super::*;

    #[test]
    fn non_unix_engine_remains_available_but_ssh_attachment_is_unsupported() {
        let gateway = Gateway::start(super::super::tests::prepared("127.0.0.1:8123".parse().unwrap())).unwrap();
        assert_eq!(gateway.status().state, State::Active);
        let connection = Connection {
            host: "192.0.2.1".into(),
            port: 22,
            identity: "/unused/key".into(),
            known_hosts: "/unused/hosts".into(),
            host_key_alias: "unused-worker".into(),
            proxy_command: None,
        };
        assert!(matches!(gateway.attach(connection), Err(Error::UnsupportedSource)));
    }
}
