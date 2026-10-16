mod tcp_owner;
use super::*;
use std::net::IpAddr;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, UnixStream},
    process::Command,
    time::{sleep, timeout},
};

const WAIT: Duration = Duration::from_secs(3);
fn until() -> Instant {
    Instant::now() + WAIT
}
fn pair() -> (UnixStream, UnixStream) {
    UnixStream::pair().unwrap()
}

// Grammar-only fixture; this does not issue an AcceptedPeer or bypass production receipt.
async fn read_header(stream: &UnixStream, until: Instant) -> Result<proxy::Header, Error> {
    let (prefix, _) = tokio::time::timeout_at(until, ancillary::byte(stream, false))
        .await
        .map_err(|_| Error::Deadline)??;
    if prefix != Some(b'P') {
        return Err(Error::Encoding);
    }
    proxy::read(stream, until).await
}

pub(super) async fn prove_proxy(stream: &TcpStream, pid: rustix::process::Pid, until: Instant) -> Result<(), Error> {
    tcp_owner::prove_owned(stream, pid, until).await.map(|_| ())
}

#[tokio::test]
async fn unix_kernel_credentials_refuse_a_different_process_or_nonroot_peer() {
    let (accepted, _client) = pair();
    let other = rustix::process::Pid::from_raw(if std::process::id() == 1 { 2 } else { 1 }).unwrap();
    assert!(daemon::verify_peer(&accepted, other).is_err());
    assert_eq!(
        daemon::verify_peer(&accepted, rustix::process::getpid()).is_ok(),
        rustix::process::geteuid().is_root() && rustix::process::getegid().as_raw() == 0
    );
}

async fn generic_proxies_cannot_reach_backend(producer: &Producer) {
    let source: IpAddr = std::env::var("HORIZON_PRODUCER_EXPECTED_ADDRESS")
        .unwrap()
        .parse()
        .unwrap();
    let identity = producer
        .daemon
        .whois(SocketAddr::new(source, 4567), until())
        .await
        .unwrap();
    assert_eq!(
        identity.stable_id,
        std::env::var("HORIZON_PRODUCER_EXPECTED_PEER").unwrap()
    );
    let destination = producer
        .generation
        .addresses
        .iter()
        .find(|ip| ip.is_ipv4() == source.is_ipv4())
        .unwrap();
    let forged = format!(
        "PROXY {} {source} {destination} 4567 {}\r\nFORGED",
        if source.is_ipv4() { "TCP4" } else { "TCP6" },
        producer.virtual_port()
    );
    for target in [
        "127.0.0.1".to_owned(),
        format!("unix:{}", producer.backend_path().display()),
    ] {
        let mut proxy = timeout(WAIT, TcpStream::connect("127.0.0.1:1055"))
            .await
            .unwrap()
            .unwrap();
        producer.daemon.prove_test_proxy(&proxy, until()).await.unwrap();
        proxy.write_all(&[5, 1, 0]).await.unwrap();
        let mut greeting = [0; 2];
        timeout(WAIT, proxy.read_exact(&mut greeting)).await.unwrap().unwrap();
        assert_eq!(greeting, [5, 0]);
        let mut request = vec![5, 1, 0, 3, u8::try_from(target.len()).unwrap()];
        request.extend_from_slice(target.as_bytes());
        request.extend_from_slice(&producer.backend_port().to_be_bytes());
        request.extend_from_slice(forged.as_bytes());
        proxy.write_all(&request).await.unwrap();
        let mut reply = [0; 2];
        timeout(WAIT, proxy.read_exact(&mut reply)).await.unwrap().unwrap();
        assert_eq!(reply[0], 5);
        assert_ne!(reply[1], 0, "Generic SOCKS must not reach the identity backend");
    }
    let mut proxy = timeout(WAIT, TcpStream::connect("127.0.0.1:1056"))
        .await
        .unwrap()
        .unwrap();
    producer.daemon.prove_test_proxy(&proxy, until()).await.unwrap();
    let request = format!("CONNECT 127.0.0.1:20032 HTTP/1.1\r\nHost: 127.0.0.1:20032\r\n\r\n{forged}");
    proxy.write_all(request.as_bytes()).await.unwrap();
    let response = timeout(WAIT, async {
        let mut response = Vec::new();
        while !response.ends_with(b"\r\n") {
            assert!(response.len() < 1024);
            response.push(proxy.read_u8().await.unwrap());
        }
        response
    })
    .await
    .unwrap();
    assert!(response.starts_with(b"HTTP/1.1 "));
    assert_ne!(
        &response[9..12],
        b"200",
        "Generic HTTP CONNECT must not reach the identity backend"
    );
}

fn status(id: &str, suffix: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"BackendState":"Running","Self":{"ID":id,
        "DNSName":format!("worker.{suffix}."),"TailscaleIPs":["100.64.0.2","fd7a:115c:a1e0::2"]},
        "CurrentTailnet":{"MagicDNSSuffix":suffix}}))
    .unwrap()
}
fn peer() -> serde_json::Value {
    serde_json::json!({"Node":{"StableID":"nPeer123","Name":"peer.example.ts.net.",
        "Addresses":["100.64.0.1/32","fd7a:115c:a1e0::1/128"],"Tags":["tag:test"]}})
}

#[tokio::test]
async fn fragmented_unix_proxy_header_preserves_application_bytes_and_virtual_port() {
    let (mut accepted, mut client) = pair();
    let writer = tokio::spawn(async move {
        client.write_all(b"PROXY TCP4 100.64.0.1 ").await.unwrap();
        tokio::task::yield_now().await;
        client.write_all(b"100.64.0.2 4567 20000\r\nPING").await.unwrap();
    });
    let header = read_header(&accepted, until()).await.unwrap();
    assert_eq!(header.destination, "100.64.0.2:20000".parse().unwrap());
    let generation = whois::generation(&status("nOriginal", "example.ts.net")).unwrap();
    verify_destination(header.destination, 20000, &generation).unwrap();
    let mut bytes = [0; 4];
    accepted.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"PING");
    writer.await.unwrap();
}

#[test]
fn proxy_destination_rejects_tcp_backend_port_wrong_virtual_port_and_wrong_self() {
    let generation = whois::generation(&status("nOriginal", "example.ts.net")).unwrap();
    for wrong in ["100.64.0.2:20032", "100.64.0.2:20001", "100.64.0.9:20000"] {
        assert!(verify_destination(wrong.parse().unwrap(), 20000, &generation).is_err());
    }
}

#[test]
fn proxy_requires_one_strict_v1_tcp_header_with_finite_valid_endpoints() {
    for valid in [
        b"PROXY TCP4 100.64.0.1 100.64.0.2 4567 20000\r\n".as_slice(),
        b"PROXY TCP6 fd7a:115c:a1e0::1 fd7a:115c:a1e0::2 4567 20000\r\n",
    ] {
        assert!(proxy::parse(valid).is_ok());
    }
    for bad in [
        "PROXY UNKNOWN\r\n",
        "PROXY  TCP4 100.64.0.1 100.64.0.2 1 20000\r\n",
        "PROXY TCP4 ::1 ::1 1 20000\r\n",
        "PROXY TCP4 100.64.0.1 100.64.0.2 01 20000\r\n",
        "PROXY TCP4 100.64.0.1 100.64.0.2 0 20000\r\n",
        "PROXY TCP4 100.64.0.1 100.64.0.2 1 65536\r\n",
        "PROXY TCP6 ::ffff:100.64.0.1 ::1 1 20000\r\n",
        "PROXY TCP4 0.0.0.0 100.64.0.2 1 20000\r\n",
        "PROXY TCP4 100.64.0.1 100.64.0.2 1 20000 extra\r\n",
        "\r\n\r\n\0\r\nQUIT\n",
    ] {
        assert!(proxy::parse(bad.as_bytes()).is_err(), "{bad:?}");
    }
    assert!(proxy::parse(&[b'x'; 109]).is_err());
}

#[tokio::test]
async fn slow_and_oversized_headers_do_not_wait_or_allocate_without_a_bound() {
    let (accepted, mut client) = pair();
    assert!(matches!(
        proxy::read(&accepted, Instant::now() + Duration::from_millis(30)).await,
        Err(Error::Deadline)
    ));
    client.write_all(b"P").await.unwrap();
    client.write_all(&[b'x'; 109]).await.unwrap();
    assert!(matches!(read_header(&accepted, until()).await, Err(Error::Encoding)));
}

#[test]
fn original_running_self_id_suffix_and_address_generation_cannot_be_substituted() {
    let original = whois::generation(&status("nOriginal", "example.ts.net")).unwrap();
    assert_ne!(
        original,
        whois::generation(&status("nReplacement", "example.ts.net")).unwrap()
    );
    assert_ne!(
        original,
        whois::generation(&status("nOriginal", "another.ts.net")).unwrap()
    );
    for change in ["Starting", "NeedsLogin", "Stopped"] {
        let mut value: serde_json::Value = serde_json::from_slice(&status("nOriginal", "example.ts.net")).unwrap();
        value["BackendState"] = change.into();
        assert!(whois::generation(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    assert!(whois::generation(&status("", "example.ts.net")).is_err());
}

#[test]
fn identity_uses_stable_id_full_name_direct_source_and_actual_tags_only() {
    let ip: IpAddr = "100.64.0.1".parse().unwrap();
    let identity = whois::identity(&serde_json::to_vec(&peer()).unwrap(), ip).unwrap();
    assert_eq!(identity.stable_id, "nPeer123");
    assert_eq!(identity.name, "peer.example.ts.net");
    assert_eq!(identity.tags, ["tag:test"]);
    for (field, value) in [
        ("StableID", serde_json::json!("")),
        ("Name", serde_json::json!("display-only")),
        ("Expired", serde_json::json!(true)),
        ("Tags", serde_json::json!(["tag:invalid/slash"])),
        ("Addresses", serde_json::json!(["100.64.0.0/24"])),
    ] {
        let mut p = peer();
        p["Node"][field] = value;
        assert!(whois::identity(&serde_json::to_vec(&p).unwrap(), ip).is_err());
    }
    assert!(whois::identity(&serde_json::to_vec(&peer()).unwrap(), "100.64.0.9".parse().unwrap()).is_err());
    assert!(whois::identity(&vec![b'x'; 65537].into_boxed_slice(), ip).is_err());
}

fn python(script: &str) -> Command {
    let mut command = Command::new("/usr/bin/python3");
    command.args(["-c", script]);
    command
}
#[tokio::test]
async fn process_output_error_and_shared_deadline_are_bounded() {
    let slots = Arc::new(Semaphore::new(1));
    assert_eq!(
        whois::run(python("print('ok',end='')"), until(), Arc::clone(&slots))
            .await
            .unwrap(),
        b"ok"
    );
    assert!(matches!(
        whois::run(python("print('x'*65537)"), until(), Arc::clone(&slots)).await,
        Err(Error::Encoding)
    ));
    assert!(matches!(
        whois::run(
            python("import sys; sys.stderr.write('x'*8193)"),
            until(),
            Arc::clone(&slots)
        )
        .await,
        Err(Error::Encoding)
    ));
    assert!(
        whois::run(python("raise SystemExit(2)"), until(), Arc::clone(&slots))
            .await
            .is_err()
    );
    assert!(matches!(
        whois::run(
            python("import time; time.sleep(30)"),
            Instant::now() + Duration::from_millis(50),
            Arc::clone(&slots)
        )
        .await,
        Err(Error::Deadline)
    ));
    assert!(matches!(
        whois::run(python("print('never')"), Instant::now(), Arc::clone(&slots)).await,
        Err(Error::Deadline)
    ));
    assert_eq!(slots.available_permits(), 1);
}

#[tokio::test]
async fn dropped_command_future_kills_reaps_and_retains_capacity_until_terminal() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("pid");
    let mut command = python(
        "import os,sys,time; p=sys.argv[1]; open(p+'.pending','w').write(str(os.getpid())); os.replace(p+'.pending',p); time.sleep(30)",
    );
    command.arg(&marker);
    let slots = Arc::new(Semaphore::new(1));
    let task = tokio::spawn(whois::run(command, until(), Arc::clone(&slots)));
    timeout(WAIT, async {
        while !marker.exists() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let pid: u32 = std::fs::read_to_string(marker).unwrap().parse().unwrap();
    assert_eq!(slots.available_permits(), 0);
    assert!(matches!(
        whois::run(python("print('never')"), until(), Arc::clone(&slots)).await,
        Err(Error::Busy)
    ));
    task.abort();
    let _ = task.await;
    timeout(WAIT, async {
        while slots.available_permits() != 1 {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
}

#[tokio::test]
async fn dropping_identification_transport_shuts_the_original_stream() {
    let (accepted, mut client) = pair();
    let retained = accepted.into_std().unwrap().try_clone().unwrap();
    let stream = UnixStream::from_std(retained.try_clone().unwrap()).unwrap();
    drop(Closing::new(stream));
    assert_eq!(
        timeout(WAIT, client.read_u8()).await.unwrap().unwrap_err().kind(),
        io::ErrorKind::UnexpectedEof
    );
}

#[tokio::test]
async fn dropping_unpolled_identification_shuts_even_a_retained_socket_descriptor() {
    let (accepted, mut client) = pair();
    let retained = accepted.into_std().unwrap();
    let stream = UnixStream::from_std(retained.try_clone().unwrap()).unwrap();
    let pending = Arc::new(Semaphore::new(1));
    let identification = begin_identification(stream, Arc::clone(&pending));
    drop(identification);
    assert_eq!(pending.available_permits(), 1);
    assert_eq!(
        timeout(WAIT, client.read_u8()).await.unwrap().unwrap_err().kind(),
        io::ErrorKind::UnexpectedEof
    );
    assert!(retained.peer_addr().is_ok());
}

#[tokio::test]
async fn capacity_refusal_shuts_the_original_socket_before_returning_busy() {
    let (accepted, mut client) = pair();
    let retained = accepted.into_std().unwrap();
    let stream = UnixStream::from_std(retained.try_clone().unwrap()).unwrap();
    let pending = Arc::new(Semaphore::new(0));
    let mut identification = Box::pin(begin_identification(stream, Arc::clone(&pending)));
    assert!(matches!(identification.as_mut().await, Err(Error::Busy)));
    assert_eq!(pending.available_permits(), 0);
    assert_eq!(
        timeout(WAIT, client.read_u8()).await.unwrap().unwrap_err().kind(),
        io::ErrorKind::UnexpectedEof
    );
    assert!(retained.peer_addr().is_ok());
}

#[test]
fn proc_reads_refuse_truncation_and_do_not_adopt_caller_files_as_authority() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("table");
    std::fs::write(&file, b"12345").unwrap();
    assert!(procfs::bounded(&file, 4).is_err());
    assert_eq!(procfs::bounded(&file, 5).unwrap(), b"12345");
}

#[tokio::test]
async fn normal_constructor_never_grants_a_nonroot_controller_authority() {
    if !rustix::process::geteuid().is_root() {
        assert!(matches!(
            Producer::pin(Runtime::Worker, 20000, 20032).await,
            Err(Error::Root)
        ));
    }
}

#[tokio::test]
#[ignore = "Requires an approved task-owned original root daemon and separate tailnet client"]
async fn actual_task_owned_producer() {
    let runtime = match std::env::var("HORIZON_PRODUCER_COMPONENT").unwrap().as_str() {
        "task-owned-worker" => Runtime::Worker,
        "task-owned-kernel" => Runtime::Kernel,
        _ => panic!("Explicit approved component opt-in is required"),
    };
    let producer = Producer::pin(runtime, 20000, 20032).await.unwrap();
    println!("horizon-producer-component-ready=20032");
    let forged = timeout(Duration::from_secs(20), producer.accept())
        .await
        .unwrap()
        .unwrap();
    assert!(producer.identify(forged).await.is_err());
    println!("horizon-producer-component-forgery-refused=1");
    if matches!(runtime, Runtime::Worker) {
        generic_proxies_cannot_reach_backend(&producer).await;
        println!("horizon-producer-component-same-daemon-proxies-refused=1");
    }
    let stream = timeout(Duration::from_secs(20), producer.accept())
        .await
        .unwrap()
        .unwrap();
    let identified = producer.identify(stream).await;
    let peer = identified.unwrap();
    assert_eq!(
        peer.stable_id(),
        std::env::var("HORIZON_PRODUCER_EXPECTED_PEER").unwrap()
    );
    assert_eq!(
        peer.full_dns_name(),
        std::env::var("HORIZON_PRODUCER_EXPECTED_DNS")
            .unwrap()
            .trim_end_matches('.')
            .to_ascii_lowercase()
    );
    let tags: Vec<String> = serde_json::from_str(&std::env::var("HORIZON_PRODUCER_EXPECTED_TAGS").unwrap()).unwrap();
    assert_eq!(peer.tags(), tags);
    let mut stream = peer.into_stream();
    let mut bytes = [0; 4];
    timeout(WAIT, stream.read_exact(&mut bytes)).await.unwrap().unwrap();
    assert_eq!(&bytes, b"PING");
    let half_close = std::env::var("HORIZON_PRODUCER_HALF_CLOSE").as_deref() == Ok("1");
    if half_close {
        let mut extra = [0];
        assert_eq!(timeout(WAIT, stream.read(&mut extra)).await.unwrap().unwrap(), 0);
    }
    stream.write_all(b"PONG").await.unwrap();
    if !half_close {
        timeout(WAIT, stream.read_exact(&mut bytes)).await.unwrap().unwrap();
        assert_eq!(&bytes, b"DONE");
        let mut extra = [0];
        assert_eq!(timeout(WAIT, stream.read(&mut extra)).await.unwrap().unwrap(), 0);
    }
    stream.shutdown().await.unwrap();
    timeout(WAIT, stream.finish()).await.unwrap().unwrap();
    if half_close {
        println!("horizon-producer-component-half-close=drained-original-unix-peer");
    }
    drop(stream);
    producer.retire().await.unwrap();
    println!("horizon-producer-component-closed=1");
}

#[tokio::test]
async fn producer_retirement_waits_for_cancelled_supervisor_kill_and_reap() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("retiring-pid");
    let mut command = python(
        "import os,sys,time; p=sys.argv[1]; open(p+'.pending','w').write(str(os.getpid())); os.replace(p+'.pending',p); time.sleep(30)",
    );
    command.arg(&marker);
    let children = Arc::new(Semaphore::new(MAX_PENDING));
    let task = tokio::spawn(whois::run(command, until(), Arc::clone(&children)));
    timeout(WAIT, async {
        while !marker.exists() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let pid: u32 = std::fs::read_to_string(marker).unwrap().parse().unwrap();
    assert_eq!(children.available_permits(), MAX_PENDING - 1);
    task.abort();
    let _ = task.await;
    daemon::drain_children(Arc::clone(&children), until()).await.unwrap();
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    let held = Arc::clone(&children).try_acquire_owned().unwrap();
    assert!(matches!(
        daemon::drain_children(children, Instant::now() + Duration::from_millis(20)).await,
        Err(Error::Deadline)
    ));
    drop(held);
}
