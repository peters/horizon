//! A busy or slow loopback client cannot extend the sign-in deadline.
use super::*;

#[test]
fn an_expired_attempt_does_not_accept_an_already_queued_callback() {
    let root = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let attempt = Attempt::prepare(root.path(), listener.local_addr().unwrap().port()).unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    write!(
        client,
        "GET {CALLBACK_PATH}?error=access_denied&state={} HTTP/1.1\r\n\r\n",
        attempt.state
    )
    .unwrap();
    assert!(matches!(
        wait_for_callback(
            &listener,
            &attempt,
            &Cancellation::default(),
            Instant::now().checked_sub(Duration::from_secs(1)).unwrap()
        ),
        Err(Error::Provider(_))
    ));
    // The expired loop never consumed the ready connection.
    assert!(listener.accept().is_ok());
}

#[test]
fn a_queued_slow_client_cannot_extend_the_deadline_by_the_socket_timeout() {
    let root = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let attempt = Attempt::prepare(root.path(), listener.local_addr().unwrap().port()).unwrap();
    let mut clients = Vec::new();
    for _ in 0..3 {
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client.write_all(b"GET / HTTP/1.1\r\n").unwrap();
        clients.push(client);
    }
    let before = Instant::now();
    assert!(matches!(
        wait_for_callback(
            &listener,
            &attempt,
            &Cancellation::default(),
            before + Duration::from_millis(100)
        ),
        Err(Error::Provider(_))
    ));
    assert!(before.elapsed() < Duration::from_secs(2));
    assert_eq!(clients.len(), 3);
    assert!(
        listener.accept().is_ok(),
        "queued traffic remains when the deadline ends"
    );
}

#[test]
fn an_incomplete_callback_is_not_accepted_after_the_client_closes() {
    let root = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let attempt = Attempt::prepare(root.path(), listener.local_addr().unwrap().port()).unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (stream, _) = listener.accept().unwrap();
    write!(
        client,
        "GET {CALLBACK_PATH}?error=access_denied&state={} HTTP/1.1\r\n",
        attempt.state
    )
    .unwrap();
    client.shutdown(std::net::Shutdown::Write).unwrap();
    assert!(answer(stream, &attempt, Instant::now() + TIMEOUT).is_none());
}
