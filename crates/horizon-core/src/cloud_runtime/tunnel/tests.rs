use super::*;
use std::io::Write;

#[cfg(unix)]
use std::sync::{Arc, atomic::Ordering};

#[test]
fn silent_and_trickled_banners_share_one_deadline() {
    for trickle in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            if trickle {
                for byte in b"RFB 003.008\n" {
                    if socket.write_all(&[*byte]).is_err() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(60));
                }
            } else {
                let _ = socket.read(&mut [0]);
            }
        });
        let started = Instant::now();
        assert!(wait_ready(address, &Cancellation::default(), Duration::from_millis(180)).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        server.join().unwrap();
    }
}

#[test]
fn readiness_observes_cancellation_while_the_peer_is_silent() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let cancel = Cancellation::default();
    let trigger = cancel.clone();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        trigger.cancel();
        socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let _ = socket.read(&mut [0]);
    });
    let started = Instant::now();
    assert!(wait_ready(address, &cancel, READY_TIMEOUT).is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
    server.join().unwrap();
}

#[cfg(unix)]
fn echo_transport() -> Command {
    let mut command = Command::new("sh");
    command.args(["-c", "printf 'RFB 003.008\\n'; exec cat"]);
    command
}

#[cfg(unix)]
fn connect_banner(endpoint: SocketAddr) -> TcpStream {
    let mut socket = TcpStream::connect(endpoint).unwrap();
    socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut banner = [0; 12];
    socket.read_exact(&mut banner).unwrap();
    assert_eq!(&banner, b"RFB 003.008\n");
    socket
}

#[test]
#[cfg(unix)]
fn listener_ownership_idle_transport_capacity_and_disconnect_cleanup() {
    let tunnel = DesktopTunnel::listen(echo_transport).unwrap();
    assert!(TcpListener::bind(tunnel.endpoint).is_err());
    wait_ready(tunnel.endpoint, &Cancellation::default(), READY_TIMEOUT).unwrap();
    // The readiness connection must release its slot before viewer connections arrive.
    thread::sleep(Duration::from_millis(100));
    let mut clients = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        clients.push(connect_banner(tunnel.endpoint));
    }
    let mut excess = TcpStream::connect(tunnel.endpoint).unwrap();
    excess.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    assert_eq!(excess.read(&mut [0]).unwrap(), 0);
    thread::sleep(Duration::from_millis(150));
    clients[0].write_all(b"ping").unwrap();
    let mut echo = [0; 4];
    clients[0].read_exact(&mut echo).unwrap();
    assert_eq!(&echo, b"ping");
    assert!(TcpListener::bind(tunnel.endpoint).is_err());
    drop(clients.pop());
    thread::sleep(Duration::from_millis(100));
    clients.push(connect_banner(tunnel.endpoint));
    // Parallel tests bind 127.0.0.1:0 and can be handed this ephemeral port as
    // soon as the listener closes. Shutdown is proven by the accept thread
    // releasing its socket, which stays true when that port is reused.
    let listener_released = Arc::clone(&tunnel.listener_released);
    let started = Instant::now();
    drop(tunnel);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(
        listener_released.load(Ordering::Acquire),
        "desktop tunnel listener was still held after shutdown"
    );
    for mut socket in clients {
        assert_eq!(socket.read(&mut [0]).unwrap(), 0);
    }
}

#[test]
#[cfg(unix)]
fn failed_process_start_releases_connection_capacity() {
    let tunnel = DesktopTunnel::listen(|| Command::new("/nonexistent/horizon-test-ssh")).unwrap();
    for _ in 0..MAX_CONNECTIONS + 2 {
        let mut socket = TcpStream::connect(tunnel.endpoint).unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        assert_eq!(socket.read(&mut [0]).unwrap(), 0);
    }
}
