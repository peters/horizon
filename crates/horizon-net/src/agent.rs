use std::{
    collections::BTreeSet,
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use iroh::{Endpoint, EndpointAddr, RelayMode, Watcher, endpoint::Connection};
use serde::{Deserialize, Serialize};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    task::{JoinHandle, JoinSet},
};

use crate::{
    Controller, Error, Result, Topology,
    model::parse_key,
    store::Store,
    wire::{self, Request, Response},
};

const DEADLINE: Duration = Duration::from_secs(15);
const MAX_CONNECTIONS: usize = 256;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    pub node: String,
    pub secret_key: String,
    pub authority_key: String,
    pub topology: Topology,
    pub relay_urls: Vec<String>,
    #[serde(default)]
    pub relay_only: bool,
}

impl std::fmt::Debug for AgentConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentConfig")
            .field("node", &self.node)
            .field("secret_key", &"[redacted]")
            .field("authority_key", &self.authority_key)
            .field("topology", &self.topology)
            .field("relay_urls", &"[redacted]")
            .field("relay_only", &self.relay_only)
            .finish()
    }
}

pub struct Agent {
    endpoint: Endpoint,
    controller: Controller,
    relays: Arc<BTreeSet<iroh::RelayUrl>>,
    accept_task: Mutex<Option<JoinHandle<()>>>,
    expiry_task: Mutex<Option<JoinHandle<()>>>,
    forward_tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Agent {
    /// # Errors
    /// Returns an error for invalid identity/configuration or unavailable transport.
    /// This in-memory form denies remote policy updates. Use `bind_persistent`
    /// for agents that receive topology changes from the authority.
    pub async fn bind(config: AgentConfig, controller: Controller) -> Result<Self> {
        Self::bind_internal(config, controller, None).await
    }

    /// # Errors
    /// Returns an error for invalid configuration, corrupt state or unavailable transport.
    pub async fn bind_persistent(path: &Path) -> Result<Self> {
        let mut config: AgentConfig = serde_json::from_slice(&crate::store::read_private(path)?)?;
        let directory = path.with_extension("state");
        let store = Store::open(directory, &mut config)?;
        let controller = Controller::new(config.topology.clone())?;
        Self::bind_internal(config, controller, Some(store)).await
    }

    /// # Errors
    /// Returns an error for invalid identity, corrupt state or unavailable transport.
    pub async fn bind_with_store(mut config: AgentConfig, directory: PathBuf) -> Result<Self> {
        let store = Store::open(directory, &mut config)?;
        let controller = Controller::new(config.topology.clone())?;
        Self::bind_internal(config, controller, Some(store)).await
    }

    async fn bind_internal(config: AgentConfig, controller: Controller, store: Option<Store>) -> Result<Self> {
        config.topology.validate()?;
        if config.topology != controller.topology() {
            return Err(Error::InvalidConfiguration(
                "controller topology differs from config".into(),
            ));
        }
        if let Some(store) = &store {
            controller.bind_store(store.clone())?;
        }
        let secret: iroh::SecretKey = config
            .secret_key
            .parse()
            .map_err(|_| Error::InvalidConfiguration("invalid secret key".into()))?;
        let authority = parse_key(&config.authority_key)?;
        match config.topology.nodes.get(&config.node) {
            Some(node) if parse_key(&node.key)? == secret.public() => {}
            Some(_) => {
                return Err(Error::InvalidConfiguration(
                    "local public key does not match identity".into(),
                ));
            }
            None if store.as_ref().map(Store::enrolled_key).transpose()? == Some(secret.public()) => {
                // A persisted withdrawal must remain denied after restart, while
                // the enrolled key can still receive a newer authority update.
            }
            None => return Err(Error::InvalidConfiguration("local node missing".into())),
        }
        let relays = validate_relays(&config.relay_urls)?;
        let configured_relays = Arc::new(relays.iter().cloned().collect());
        let mut builder = iroh::endpoint::Builder::empty()
            .preset(iroh::endpoint::presets::Minimal)
            .secret_key(secret)
            .alpns(vec![wire::ALPN.to_vec()])
            .relay_mode(RelayMode::custom(relays));
        if config.relay_only {
            builder = builder.clear_ip_transports();
        }
        let endpoint = builder.bind().await.map_err(transport)?;
        let accept_endpoint = endpoint.clone();
        let accept_controller = controller.clone();
        let accept_task = tokio::spawn(async move {
            let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
            let mut tasks = JoinSet::new();
            loop {
                tokio::select! {
                    incoming = accept_endpoint.accept() => {
                        let Some(incoming) = incoming else {break};
                        let Ok(permit) = permits.clone().try_acquire_owned() else {
                            incoming.refuse();
                            continue;
                        };
                        let controller = accept_controller.clone();
                        let store = store.clone();
                        let local = accept_endpoint.id();
                        tasks.spawn(async move {
                            let _permit = permit;
                            if let Ok(Ok(connection)) = tokio::time::timeout(DEADLINE, incoming).await {
                                let _ = serve(&connection, local, authority, controller, store).await;
                                connection.close(0_u32.into(), b"request complete");
                            }
                        });
                    }
                    Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
                }
            }
        });
        let expiry_controller = controller.clone();
        let expiry_task = tokio::spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_millis(100));
            loop {
                timer.tick().await;
                expiry_controller.expire();
            }
        });
        Ok(Self {
            endpoint,
            controller,
            relays: configured_relays,
            accept_task: Mutex::new(Some(accept_task)),
            expiry_task: Mutex::new(Some(expiry_task)),
            forward_tasks: Mutex::new(Vec::new()),
        })
    }

    #[must_use]
    pub fn addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }

    #[must_use]
    pub fn id(&self) -> iroh::EndpointId {
        self.endpoint.id()
    }

    #[must_use]
    pub fn controller(&self) -> Controller {
        self.controller.clone()
    }

    /// Check the authenticated remote endpoint without opening a TCP service.
    ///
    /// # Errors
    /// Returns an error for a foreign endpoint, unavailable transport or rejected membership.
    pub async fn probe(&self, destination: EndpointAddr) -> Result<()> {
        let topology = self.controller.topology();
        if !topology
            .nodes
            .values()
            .any(|node| parse_key(&node.key).ok() == Some(destination.id))
        {
            return Err(Error::Denied);
        }
        let key = destination.id;
        let result = async {
            let connection = dial(&self.endpoint, destination, &self.relays).await?;
            let (mut send, mut recv) = connection.open_bi().await.map_err(transport)?;
            wire::write(
                &mut send,
                &Request::Probe {
                    network: topology.network,
                },
            )
            .await?;
            let response: Response = wire::read(&mut recv).await?;
            connection.close(0_u32.into(), b"probe complete");
            if response.accepted { Ok(()) } else { Err(Error::Denied) }
        };
        let result = tokio::time::timeout(DEADLINE, result)
            .await
            .map_err(|_| Error::Timeout)?;
        self.controller.observed(key, result.is_ok());
        result
    }

    /// Return the current transport watcher state, without a historical ready flag.
    #[must_use]
    pub fn relay_connected(&self) -> bool {
        !self.endpoint.is_closed()
            && self
                .endpoint
                .home_relay_status()
                .get()
                .into_iter()
                .any(|status| status.is_connected())
    }

    /// # Errors
    /// Returns a timeout if the endpoint cannot connect to its configured relay.
    pub async fn online(&self) -> Result<()> {
        tokio::time::timeout(DEADLINE, self.endpoint.online())
            .await
            .map_err(|_| Error::Timeout)?;
        Ok(())
    }

    /// # Errors
    /// Returns an error for invalid policy, unavailable transport or rejected authority.
    pub async fn push_topology(&self, destination: EndpointAddr, topology: Topology) -> Result<()> {
        topology.validate()?;
        let connection = dial(&self.endpoint, destination, &self.relays).await?;
        let request = Request::Update { topology };
        let result = tokio::time::timeout(DEADLINE, async {
            let (mut send, mut recv) = connection.open_bi().await.map_err(transport)?;
            wire::write(&mut send, &request).await?;
            let response: Response = wire::read(&mut recv).await?;
            if !response.accepted {
                return Err(Error::Denied);
            }
            Ok(())
        })
        .await
        .map_err(|_| Error::Timeout)?;
        connection.close(0_u32.into(), b"update complete");
        result
    }

    /// The local listener is always loopback. Only the granted named remote
    /// service can be reached; callers cannot supply a destination port.
    ///
    /// # Errors
    /// Returns an error for missing grants, mismatched destination identity or bind failure.
    pub async fn forward(&self, destination: EndpointAddr, service: String, port: u16) -> Result<Forwarder> {
        let authorized = self.controller.authorize(&self.id().to_string(), &service)?;
        let topology = self.controller.topology();
        if topology
            .nodes
            .get(&authorized.node)
            .and_then(|node| parse_key(&node.key).ok())
            != Some(destination.id)
        {
            return Err(Error::Denied);
        }
        validate_destination(&destination, &self.relays)?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
        let address = listener.local_addr()?;
        let endpoint = self.endpoint.clone();
        let controller = self.controller.clone();
        let relays = self.relays.clone();
        let task = tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else {break};
                        let Ok(permit) = permits.clone().try_acquire_owned() else {continue};
                        let endpoint = endpoint.clone();
                        let controller = controller.clone();
                        let destination = destination.clone();
                        let service = service.clone();
                        let relays = relays.clone();
                        tasks.spawn(async move {
                            let _permit = permit;
                            let _ = outbound(endpoint, controller, destination, service, socket, relays).await;
                        });
                    }
                    Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
                }
            }
        });
        let abort = task.abort_handle();
        let mut tasks = self
            .forward_tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tasks.retain(|task| !task.is_finished());
        tasks.push(task);
        Ok(Forwarder { address, abort })
    }

    pub async fn close(self) {
        self.shutdown().await;
    }

    /// Close this endpoint and all of its forwarding listeners and sessions.
    pub async fn shutdown(&self) {
        let accept = self
            .accept_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let expiry = self
            .expiry_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        for task in [accept, expiry].into_iter().flatten() {
            task.abort();
            let _ = task.await;
        }
        let tasks = std::mem::take(
            &mut *self
                .forward_tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
        self.endpoint.close().await;
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        for task in [&mut self.accept_task, &mut self.expiry_task] {
            if let Some(task) = task.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
                task.abort();
            }
        }
        for task in self
            .forward_tasks
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            task.abort();
        }
    }
}

pub struct Forwarder {
    address: SocketAddr,
    abort: tokio::task::AbortHandle,
}

impl Forwarder {
    #[must_use]
    pub fn address(&self) -> SocketAddr {
        self.address
    }
}

impl Drop for Forwarder {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

async fn serve(
    connection: &Connection,
    local: iroh::EndpointId,
    authority: iroh::EndpointId,
    controller: Controller,
    store: Option<Store>,
) -> Result<()> {
    let (mut send, mut recv) = tokio::time::timeout(DEADLINE, connection.accept_bi())
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(transport)?;
    let request: Request = tokio::time::timeout(DEADLINE, wire::read(&mut recv))
        .await
        .map_err(|_| Error::Timeout)??;
    let peer = connection.remote_id();
    match request {
        Request::Probe { network } => {
            let topology = controller.topology();
            let accepted = topology.network == network
                && topology
                    .nodes
                    .values()
                    .any(|node| parse_key(&node.key).ok() == Some(peer))
                && topology
                    .nodes
                    .values()
                    .any(|node| parse_key(&node.key).ok() == Some(local));
            controller.observed(peer, accepted);
            respond(&mut send, accepted, true).await?;
            Ok(())
        }
        Request::Update { topology } => {
            let accepted = peer == authority
                && store.is_some()
                && controller
                    .plan(topology)
                    .and_then(|plan| controller.apply(&plan))
                    .is_ok();
            respond(&mut send, accepted, true).await?;
            Ok(())
        }
        Request::Connect { network, service } => {
            if network != controller.topology().network {
                return reject(&mut send).await;
            }
            let Ok(authorized) = controller.authorize(&peer.to_string(), &service) else {
                return reject(&mut send).await;
            };
            let Ok(id) = controller.register(peer, local, &service, authorized.port, connection.clone()) else {
                return reject(&mut send).await;
            };
            let _session = SessionGuard {
                controller: controller.clone(),
                id,
            };
            let result = async {
                let socket = tokio::select! {
                    biased;
                    _ = connection.closed() => return Err(Error::Denied),
                    result = tokio::time::timeout(DEADLINE, TcpStream::connect((Ipv4Addr::LOCALHOST, authorized.port))) => {
                        result.map_err(|_| Error::Timeout)??
                    }
                };
                let socket = controller.attach_socket(id, socket)?;
                respond(&mut send, true, false).await?;
                controller.observed(peer, true);
                controller.confirmed(id);
                pump(connection, socket, send, recv).await
            }
            .await;
            controller.unregister(id);
            result
        }
    }
}

async fn outbound(
    endpoint: Endpoint,
    controller: Controller,
    destination: EndpointAddr,
    service: String,
    socket: TcpStream,
    relays: Arc<BTreeSet<iroh::RelayUrl>>,
) -> Result<()> {
    let authorized = controller.authorize(&endpoint.id().to_string(), &service)?;
    let connection = dial(&endpoint, destination.clone(), &relays).await?;
    let id = controller.register(
        endpoint.id(),
        destination.id,
        &service,
        authorized.port,
        connection.clone(),
    )?;
    let _session = SessionGuard {
        controller: controller.clone(),
        id,
    };
    let socket = controller.attach_socket(id, socket)?;
    let result = async {
        let request = Request::Connect {
            network: controller.topology().network,
            service,
        };
        let (send, recv) = tokio::time::timeout(DEADLINE, async {
            let (mut send, mut recv) = connection.open_bi().await.map_err(transport)?;
            wire::write(&mut send, &request).await?;
            let response: Response = wire::read(&mut recv).await?;
            if !response.accepted {
                return Err(Error::Denied);
            }
            Ok((send, recv))
        })
        .await
        .map_err(|_| Error::Timeout)??;
        controller.observed(destination.id, true);
        controller.confirmed(id);
        pump(&connection, socket, send, recv).await
    }
    .await;
    connection.close(0_u32.into(), b"forward complete");
    controller.unregister(id);
    result
}

async fn pump(
    connection: &Connection,
    socket: TcpStream,
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
) -> Result<()> {
    let (mut read, mut write) = socket.into_split();
    let upstream = async {
        tokio::io::copy(&mut read, &mut send).await?;
        send.finish().map_err(transport)?;
        send.stopped().await.map_err(transport)?;
        Result::Ok(())
    };
    let downstream = async {
        tokio::io::copy(&mut recv, &mut write).await?;
        tokio::io::AsyncWriteExt::shutdown(&mut write).await?;
        Result::Ok(())
    };
    tokio::select! {
        biased;
        _ = connection.closed() => Err(Error::Denied),
        result = async {tokio::try_join!(upstream, downstream).map(|_| ())} => result,
    }
}

async fn respond(send: &mut iroh::endpoint::SendStream, accepted: bool, complete: bool) -> Result<()> {
    bounded_response(
        async {
            wire::write(send, &Response { accepted }).await?;
            if complete {
                send.finish().map_err(transport)?;
                send.stopped().await.map_err(transport)?;
            }
            Result::Ok(())
        },
        DEADLINE,
    )
    .await?
}

async fn reject(send: &mut iroh::endpoint::SendStream) -> Result<()> {
    respond(send, false, true).await?;
    Err(Error::Denied)
}

fn validate_destination(destination: &EndpointAddr, relays: &BTreeSet<iroh::RelayUrl>) -> Result<()> {
    for address in &destination.addrs {
        match address {
            iroh::TransportAddr::Relay(relay) if relays.contains(relay) => {}
            iroh::TransportAddr::Ip(_) => {}
            _ => return Err(Error::Denied),
        }
    }
    Ok(())
}

async fn dial(endpoint: &Endpoint, destination: EndpointAddr, relays: &BTreeSet<iroh::RelayUrl>) -> Result<Connection> {
    validate_destination(&destination, relays)?;
    tokio::time::timeout(DEADLINE, endpoint.connect(destination, wire::ALPN))
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(transport)
}

fn validate_relays(urls: &[String]) -> Result<Vec<iroh::RelayUrl>> {
    if urls.is_empty() {
        return Err(Error::InvalidConfiguration(
            "at least one explicit self-hosted relay is required".into(),
        ));
    }
    urls.iter()
        .map(|url| {
            let relay: iroh::RelayUrl = url
                .parse()
                .map_err(|_| Error::InvalidConfiguration("invalid relay URL".into()))?;
            if !relay.username().is_empty()
                || relay.password().is_some()
                || relay.query().is_some()
                || relay.fragment().is_some()
            {
                return Err(Error::InvalidConfiguration(
                    "relay URLs must not contain credentials, query strings or fragments".into(),
                ));
            }
            if relay.scheme() != "https"
                && !(relay.scheme() == "http"
                    && relay
                        .host_str()
                        .is_some_and(|host| host == "127.0.0.1" || host == "localhost" || host == "[::1]"))
            {
                return Err(Error::InvalidConfiguration(
                    "relay requires HTTPS; loopback HTTP is permitted for isolated tests".into(),
                ));
            }
            Ok(relay)
        })
        .collect()
}

fn transport(error: impl std::fmt::Display) -> Error {
    Error::Transport(error.to_string())
}

struct SessionGuard {
    controller: Controller,
    id: u64,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.controller.unregister(self.id);
    }
}

async fn bounded_response<T>(completion: impl std::future::Future<Output = T>, duration: Duration) -> Result<T> {
    tokio::time::timeout(duration, completion)
        .await
        .map_err(|_| Error::Timeout)
}

#[cfg(test)]
mod tests;
