#![forbid(unsafe_code)]
mod bootstrap;
mod browser;
mod catalog;
mod companions;
mod configuration;
mod controller;
mod queues;
mod remote;
use horizon_browser_protocol::cloud_view::{CloudViewRequest, CloudViewResponse};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};
const ENDPOINT: &str = "127.0.0.1:47280";
const MAX_MESSAGE: u64 = horizon_browser_protocol::cloud_view::MAX_CLOUD_VIEW_BYTES as u64;
type Job = (CloudViewRequest, mpsc::SyncSender<CloudViewResponse>);
fn main() -> std::process::ExitCode {
    let result = match std::env::args().nth(1).as_deref() {
        Some("serve") => serve(),
        Some("connect") => connect(),
        Some("configure-agent-tools") => configuration::run(),
        Some("recover-allocation") => bootstrap::run(),
        Some("companion-control") => companions::run(),
        _ => Err(io::Error::other(
            "Usage: horizon-cloud-worker serve|connect|configure-agent-tools|recover-allocation|companion-control",
        )),
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Cloud worker service: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
fn read_line(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut line = String::new();
    let n = reader.take(MAX_MESSAGE + 1).read_line(&mut line)?;
    if n as u64 > MAX_MESSAGE {
        return Err(io::Error::other("Cloud message too large"));
    }
    Ok((n > 0).then_some(line))
}
fn connect() -> io::Result<()> {
    let mut stream = TcpStream::connect(ENDPOINT)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    while let Some(line) = read_line(&mut stdin)? {
        stream.write_all(line.as_bytes())?;
        let response = read_line(&mut reader)?.ok_or_else(|| io::Error::other("Worker service disconnected"))?;
        stdout.write_all(response.as_bytes())?;
        stdout.flush()?;
    }
    Ok(())
}
fn serve() -> io::Result<()> {
    let listener = TcpListener::bind(ENDPOINT)?;
    let (tx, rx) = mpsc::sync_channel::<Job>(64);
    let clients = Arc::new(AtomicUsize::new(0));
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if clients
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < 32).then_some(n + 1))
                .is_err()
            {
                continue;
            }
            let clients = clients.clone();
            let tx = tx.clone();
            thread::spawn(move || {
                let _ = client(stream, &tx);
                clients.fetch_sub(1, Ordering::Release);
            });
        }
    });
    horizon_browser_control::paths::initialize_from_environment().map_err(io::Error::other)?;
    let mut host = browser::Host::new()?;
    loop {
        host.drain();
        queues::poll(&mut host);
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok((request, reply)) => {
                let mut response = host.request(request);
                controller::observe(&mut response);
                let _ = reply.send(response);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}
fn client(mut stream: TcpStream, tx: &mpsc::SyncSender<Job>) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    while let Some(line) = read_line(&mut reader)? {
        let request = serde_json::from_str(&line).map_err(|_| io::Error::other("Invalid cloud message"))?;
        let (reply, result) = mpsc::sync_channel(1);
        tx.send((request, reply))
            .map_err(|_| io::Error::other("Worker stopped"))?;
        let result = result
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| io::Error::other("Worker request timed out"))?;
        serde_json::to_writer(&mut stream, &result)?;
        stream.write_all(b"\n")?;
    }
    Ok(())
}
