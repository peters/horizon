//! Only a genuine opaque peer may transfer its original stream into named admission.
use super::{
    Admission, Budgets, Error, Gate, bind as listener, relay,
    state::{Phase, Shared, lock},
};
use crate::trusted_tcp_producer::{AcceptedPeer, AcceptedStream, PendingCancellation, PendingStream, Producer};
use horizon_cloud_protocol::exposure::{
    ExposureMode, ExposureRequest, RESERVED_INGRESS_END, RESERVED_INGRESS_START, producer_backend_port,
};
use std::{
    net::{Ipv4Addr, Ipv6Addr},
    sync::Arc,
};
use tokio::{sync::watch, task::JoinSet};

pub(super) fn bind(port: u16, producer: Producer, request: &ExposureRequest, budgets: Budgets) -> Result<Gate, Error> {
    let policy = request.validate()?;
    let budgets = budgets.validate()?;
    if policy.mode() != ExposureMode::Restricted {
        return Err(horizon_cloud_protocol::exposure::Error::IdentityUnavailable.into());
    }
    if !rustix::process::geteuid().is_root() {
        return Err(Error::Root);
    }
    ports(port, producer.virtual_port(), producer.backend_port())?;
    let listeners = vec![
        listener((Ipv4Addr::UNSPECIFIED, port).into())?,
        listener((Ipv6Addr::UNSPECIFIED, port).into())?,
    ];
    Gate::start(
        listeners,
        policy,
        budgets,
        Admission {
            producer: Some(Arc::new(producer)),
        },
    )
}

pub(super) fn ports(public: u16, virtual_port: u16, backend: u16) -> Result<(), Error> {
    if !(RESERVED_INGRESS_START..=RESERVED_INGRESS_END).contains(&public)
        || public == virtual_port
        || producer_backend_port(virtual_port) != Some(backend)
    {
        return Err(Error::IngressPort);
    }
    Ok(())
}

pub(super) fn same_accept(original: &PendingCancellation, stream: &AcceptedStream) -> bool {
    stream.matches_pending(original)
}

pub(super) fn spawn(
    tasks: &mut JoinSet<()>,
    shared: Shared,
    id: u64,
    client: PendingStream,
    producer: Arc<Producer>,
    cancel: watch::Receiver<bool>,
) {
    // The registered socket and Flow exist before spawn, including an unpolled abort.
    let flow = relay::Flow { shared, id };
    tasks.spawn(async move {
        let Some(peer) = identify_pending(&flow, cancel.clone(), producer.identify(client)).await else {
            return;
        };
        let client = lock(&flow.shared).identify(id, peer);
        if let Some(client) = client {
            relay::run(flow, client, cancel).await;
        }
    });
}

pub(super) async fn identify_pending(
    flow: &relay::Flow,
    mut cancel: watch::Receiver<bool>,
    identifying: impl Future<Output = Result<AcceptedPeer, crate::trusted_tcp_producer::Error>>,
) -> Option<AcceptedPeer> {
    let (until, mut phase) = {
        let state = lock(&flow.shared);
        if state.phase(flow.id) != Phase::Running {
            return None;
        }
        (state.deadline(flow.id)?, state.subscribe(flow.id)?)
    };
    tokio::select! {
        biased;
        _ = cancel.changed() => None,
        _ = phase.changed() => None,
        () = tokio::time::sleep_until(until) => None,
        result = identifying => if let Ok(peer) = result { Some(peer) } else {
            lock(&flow.shared).refuse();
            None
        },
    }
}
