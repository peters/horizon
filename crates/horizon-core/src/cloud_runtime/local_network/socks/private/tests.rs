use super::*;
use crate::cloud_runtime::local_network::{Destination, socks::Gate};
use horizon_cloud_protocol::local_network::Reply;
use std::{
    fs,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    sync::Arc,
    time::Duration,
};

struct Refuse;
impl Gate for Refuse {
    fn admit(&self, _: &Destination) -> Result<Vec<SocketAddr>, Reply> {
        Err(Reply::NotAllowed)
    }
}

fn start() -> Proxy {
    Proxy::start_private(Arc::new(Refuse)).unwrap()
}

#[test]
fn old_streamlocal_target_cannot_reach_a_successor_at_the_old_numeric_port() {
    let mut old = start();
    let stale = old.private_path().unwrap().to_owned();
    // Model the crash residue: the listener dies, but its pathname remains.
    let _residue = old.private.take().unwrap();
    let numeric = (Ipv4Addr::LOCALHOST, old.port());
    drop(old);
    assert!(stale.exists());
    let reused = TcpListener::bind(numeric).unwrap();
    let new = start();
    assert_ne!(stale, new.private_path().unwrap());
    assert!(TcpStream::connect(reused.local_addr().unwrap()).is_ok());
    assert!(UnixStream::connect(&stale).is_err());
    let mut current = UnixStream::connect(new.private_path().unwrap()).unwrap();
    current.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    current.write_all(&[5, 1, 0]).unwrap();
    let mut reply = [0; 2];
    current.read_exact(&mut reply).unwrap();
    assert_eq!(reply, [5, 0]);
}

#[test]
fn private_socket_modes_and_reserved_numeric_receipt_have_no_tcp_ingress() {
    let proxy = start();
    let path = proxy.private_path().unwrap();
    assert_eq!(fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(
        fs::symlink_metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let mut numeric = TcpStream::connect((Ipv4Addr::LOCALHOST, proxy.port())).unwrap();
    numeric.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    numeric.write_all(&[5, 1, 0]).unwrap();
    let mut reply = [0; 2];
    assert!(numeric.read_exact(&mut reply).is_err());
    assert_eq!(proxy.counters().connections, 0);
}

#[test]
fn private_socket_permissions_do_not_depend_on_inherited_umask() {
    let executable = std::env::current_exe().unwrap();
    for mask in ["022", "000", "077"] {
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", "umask \"$1\"; shift; exec \"$@\"", "private-socket-umask", mask])
            .arg(&executable)
            .args([
                "--exact",
                "cloud_runtime::local_network::socks::private::tests::private_socket_modes_and_reserved_numeric_receipt_have_no_tcp_ingress",
                "--nocapture",
                "--test-threads=1",
            ])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "umask {mask}: {stdout}\n{stderr}");
        assert!(stdout.contains("1 passed; 0 failed"), "umask {mask}: {stdout}");
    }
}

#[test]
fn revoke_closes_the_old_unix_stream_and_never_adopts_it_into_a_successor() {
    let old = start();
    let path = old.private_path().unwrap().to_owned();
    let mut stream = UnixStream::connect(&path).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    stream.write_all(&[5, 1, 0]).unwrap();
    let mut reply = [0; 2];
    stream.read_exact(&mut reply).unwrap();
    old.close();
    let mut byte = [0];
    assert!(matches!(stream.read(&mut byte), Ok(0) | Err(_)));
    let new = start();
    assert_ne!(path, new.private_path().unwrap());
    assert_eq!(new.counters().connections, 0);
}
