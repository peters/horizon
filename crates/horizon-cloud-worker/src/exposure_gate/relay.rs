use super::{
    BUFFER_BYTES,
    state::{Phase, Shared, lock},
};
use socket2::SockRef;
use std::{
    future::{Future, pending},
    io,
    net::{Ipv4Addr, Shutdown},
    sync::Arc,
};
use tokio::{
    net::{TcpSocket, TcpStream},
    sync::watch,
    task::JoinSet,
    time::Instant,
};

struct Flow {
    shared: Shared,
    id: u64,
}
impl Drop for Flow {
    fn drop(&mut self) {
        lock(&self.shared).release(self.id);
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum CopyEnd {
    Eof,
    Budget,
    Failed,
}

pub(super) fn spawn(
    tasks: &mut JoinSet<()>,
    shared: Shared,
    id: u64,
    client: Arc<TcpStream>,
    cancel: watch::Receiver<bool>,
) {
    let flow = Flow { shared, id };
    // Construct before spawn so cancellation before the first poll releases admission too.
    tasks.spawn(run(flow, client, cancel));
}

async fn run(flow: Flow, client: Arc<TcpStream>, cancel: watch::Receiver<bool>) {
    let backend = connect(&flow, cancel.clone()).await;
    if let Some(backend) = backend {
        let _ = client.set_nodelay(true);
        let _ = backend.set_nodelay(true);
        let forwarding = async {
            tokio::join!(
                copy(&flow, Arc::clone(&client), Arc::clone(&backend), cancel.clone()),
                copy(&flow, Arc::clone(&backend), Arc::clone(&client), cancel.clone()),
            )
        };
        coordinate(&flow, &client, Some(&backend), cancel.clone(), forwarding).await;
    } else {
        let phase = lock(&flow.shared).phase(flow.id);
        if let Phase::Budget(until) = phase {
            graceful(&flow, &client, None, until, cancel).await;
        }
    }
}

async fn connect(flow: &Flow, mut cancel: watch::Receiver<bool>) -> Option<Arc<TcpStream>> {
    let (target, mut phase) = {
        let state = lock(&flow.shared);
        if state.phase(flow.id) != Phase::Running {
            return None;
        }
        let until = state.deadline(flow.id)?;
        let connect_until = Instant::now().checked_add(state.budgets.connect_timeout)?;
        (
            (state.policy.target_port(), until.min(connect_until)),
            state.subscribe(flow.id)?,
        )
    };
    let socket = TcpSocket::new_v4().ok()?;
    super::bounded_socket(&SockRef::from(&socket)).ok()?;
    let backend = tokio::select! {
        biased;
        _ = cancel.changed() => return None,
        _ = phase.changed() => return None,
        () = tokio::time::sleep_until(target.1) => return None,
        result = socket.connect((Ipv4Addr::LOCALHOST, target.0).into()) => Arc::new(result.ok()?),
    };
    lock(&flow.shared)
        .attach(flow.id, Arc::clone(&backend))
        .then_some(backend)
}

async fn coordinate(
    flow: &Flow,
    client: &TcpStream,
    backend: Option<&TcpStream>,
    cancel: watch::Receiver<bool>,
    forwarding: impl Future<Output = (CopyEnd, CopyEnd)>,
) {
    // wait owns and drops both copy futures (and their buffers) before graceful
    // allocates drain buffers. A phase check also covers a completed-join race.
    let phase = wait(flow, cancel.clone(), forwarding).await;
    match phase {
        Phase::Running => lock(&flow.shared).complete(flow.id),
        Phase::Budget(until) => graceful(flow, client, backend, until, cancel).await,
        Phase::Cancelled => lock(&flow.shared).cancel_flow(flow.id),
    }
}

async fn wait(
    flow: &Flow,
    mut cancel: watch::Receiver<bool>,
    forwarding: impl Future<Output = (CopyEnd, CopyEnd)>,
) -> Phase {
    let Some(mut phase) = lock(&flow.shared).subscribe(flow.id) else {
        return Phase::Cancelled;
    };
    let running = *phase.borrow() == Phase::Running;
    let completed = if running {
        tokio::select! {
            biased;
            _ = cancel.changed() => None,
            _ = phase.changed() => None,
            result = forwarding => Some(result),
        }
    } else {
        None
    };
    let state = lock(&flow.shared);
    if state.deadline(flow.id).is_none() {
        return Phase::Cancelled;
    }
    match state.phase(flow.id) {
        Phase::Running if completed != Some((CopyEnd::Eof, CopyEnd::Eof)) => Phase::Cancelled,
        phase => phase,
    }
}

async fn graceful(
    flow: &Flow,
    client: &TcpStream,
    backend: Option<&TcpStream>,
    until: Instant,
    mut cancel: watch::Receiver<bool>,
) {
    let Some(mut phase) = lock(&flow.shared).subscribe(flow.id) else {
        return;
    };
    let shutdown = SockRef::from(client)
        .shutdown(Shutdown::Write)
        .and_then(|()| backend.map_or(Ok(()), |stream| SockRef::from(stream).shutdown(Shutdown::Write)));
    let permitted = shutdown.is_ok() && *phase.borrow() != Phase::Cancelled && !*cancel.borrow();
    let drained = if permitted {
        let inputs = async {
            tokio::try_join!(drain(client), async {
                match backend {
                    Some(stream) => drain(stream).await,
                    None => Ok(()),
                }
            })
        };
        tokio::select! {
            biased;
            _ = cancel.changed() => false,
            _ = phase.changed() => false,
            () = tokio::time::sleep_until(until) => false,
            result = inputs => result.is_ok(),
        }
    } else {
        false
    };
    let mut state = lock(&flow.shared);
    if drained {
        state.complete(flow.id);
    } else {
        state.cancel_flow(flow.id);
    }
}

async fn drain(stream: &TcpStream) -> io::Result<()> {
    let mut buffer = Box::new([0; BUFFER_BYTES]);
    let mut remaining = 64 * 1024;
    while remaining > 0 {
        stream.readable().await?;
        let limit = remaining.min(buffer.len());
        match stream.try_read(&mut buffer[..limit]) {
            Ok(0) => return Ok(()),
            Ok(count) => remaining -= count,
            Err(error) if retry(&error) => {}
            Err(error) => return Err(error),
        }
    }
    // Reaching the discard cap is not EOF. Keep the fixed grace deadline and
    // cancellation responsive without reading unlimited peer-supplied data.
    pending().await
}

async fn ready(flow: &Flow, stream: &TcpStream, writing: bool, cancel: &mut watch::Receiver<bool>) -> bool {
    loop {
        let Some(until) = lock(&flow.shared).deadline(flow.id) else {
            return false;
        };
        let available = async {
            if writing {
                stream.writable().await
            } else {
                stream.readable().await
            }
        };
        tokio::select! {
            biased;
            _ = cancel.changed() => return false,
            () = tokio::time::sleep_until(until) => {},
            result = available => return result.is_ok(),
        }
    }
}

fn retry(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted)
}

async fn copy(flow: &Flow, from: Arc<TcpStream>, to: Arc<TcpStream>, mut cancel: watch::Receiver<bool>) -> CopyEnd {
    let mut buffer = Box::new([0; BUFFER_BYTES]);
    'reading: loop {
        if !ready(flow, &from, false, &mut cancel).await {
            break;
        }
        let count = match from.try_read(&mut buffer[..]) {
            Ok(0) => {
                if SockRef::from(to.as_ref()).shutdown(Shutdown::Write).is_err() {
                    break;
                }
                return CopyEnd::Eof;
            }
            Ok(count) => count,
            Err(error) if retry(&error) => continue,
            Err(_) => break,
        };
        let allowed = lock(&flow.shared).reserve(flow.id, count);
        let mut written = 0;
        while written < allowed {
            if !ready(flow, &to, true, &mut cancel).await {
                break 'reading;
            }
            match to.try_write(&buffer[written..allowed]) {
                Ok(0) => break 'reading,
                Ok(count) => {
                    written += count;
                    lock(&flow.shared).wrote(flow.id, count);
                }
                Err(error) if retry(&error) => {}
                Err(_) => break 'reading,
            }
        }
        if allowed < count {
            return CopyEnd::Budget;
        }
    }
    lock(&flow.shared).cancel_flow(flow.id);
    CopyEnd::Failed
}

#[cfg(test)]
#[path = "tests/budgets.rs"]
mod tests;
