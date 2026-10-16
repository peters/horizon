use super::{
    BUFFER_BYTES,
    state::{Shared, lock},
};
use socket2::SockRef;
use std::{
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

async fn run(flow: Flow, client: Arc<TcpStream>, mut cancel: watch::Receiver<bool>) {
    let id = flow.id;
    let target = {
        let state = lock(&flow.shared);
        let Some(until) = state.deadline(id) else {
            return;
        };
        let Some(connect_until) = Instant::now().checked_add(state.budgets.connect_timeout) else {
            return;
        };
        (state.policy.target_port(), until.min(connect_until))
    };
    let Ok(socket) = TcpSocket::new_v4() else {
        return;
    };
    if super::bounded_socket(&SockRef::from(&socket)).is_err() {
        return;
    }
    let backend = tokio::select! {
        biased;
        _ = cancel.changed() => return,
        () = tokio::time::sleep_until(target.1) => return,
        result = socket.connect((Ipv4Addr::LOCALHOST, target.0).into()) => match result {
            Ok(stream) => Arc::new(stream),
            Err(_) => return,
        },
    };
    if !lock(&flow.shared).attach(id, Arc::clone(&backend)) {
        return;
    }
    let _ = client.set_nodelay(true);
    let _ = backend.set_nodelay(true);
    tokio::join!(
        copy(&flow, Arc::clone(&client), Arc::clone(&backend), cancel.clone()),
        copy(&flow, backend, client, cancel),
    );
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

async fn copy(flow: &Flow, from: Arc<TcpStream>, to: Arc<TcpStream>, mut cancel: watch::Receiver<bool>) {
    let mut buffer = Box::new([0; BUFFER_BYTES]);
    'reading: loop {
        if !ready(flow, &from, false, &mut cancel).await {
            break;
        }
        let count = match from.try_read(&mut buffer[..]) {
            Ok(0) => {
                let _ = SockRef::from(to.as_ref()).shutdown(Shutdown::Write);
                return;
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
            // Exhaustion stops this reader; the opposite direction may still
            // own bytes reserved before the shared budget was exhausted.
            return;
        }
    }
    lock(&flow.shared).cancel_flow(flow.id);
}

#[cfg(test)]
#[path = "tests/budgets.rs"]
mod tests;
