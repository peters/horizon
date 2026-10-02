use crate::{
    Error, PairingCredentials, Result,
    cancellation::Cancellation,
    connection::Transport,
    crypto, srp,
    tlv::{self, Message},
};
use ring::{
    agreement,
    rand::SystemRandom,
    signature::{self, KeyPair},
};
use std::{
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
};
use zeroize::Zeroizing;

const SCREEN_CAPTURE_ACL: &[u8] = b"\xe1\x57com.apple.ScreenCapture\x01";

/// A pending modern screen-capture pairing. Dropping it closes the connection.
/// The caller must show a PIN-required prompt immediately after `begin` succeeds.
pub struct Pairing {
    transport: Transport,
    identity: String,
    signing: signature::Ed25519KeyPair,
    credentials: PairingCredentials,
    address: SocketAddr,
    reservation: Reservation,
}

/// An authenticated receiver connection; credentials are kept only in memory.
pub struct PairedReceiver {
    pub(crate) transport: Transport,
    pub(crate) shared: Zeroizing<Vec<u8>>,
    pub(crate) address: SocketAddr,
    pub(crate) identity: String,
    pub(crate) credentials: PairingCredentials,
    _reservation: Reservation,
}

impl Pairing {
    /// Ask the receiver to display a fresh four-digit pairing code.
    ///
    /// # Errors
    /// Returns transport, receiver rejection or key-generation errors.
    pub fn begin(address: SocketAddr) -> Result<Self> {
        Self::begin_cancellable(address, None)
    }
    pub(crate) fn begin_cancellable(address: SocketAddr, cancellation: Option<Arc<Cancellation>>) -> Result<Self> {
        let reservation = Reservation::acquire(address.ip())?;
        let credentials = PairingCredentials::new()?;
        let mut pairing = Self {
            reservation,
            transport: Transport::connect_cancellable(address, cancellation)?,
            identity: credentials.client_id.clone(),
            signing: credentials.signing()?,
            credentials,
            address,
        };
        let headers = pairing.headers(false) + "X-Apple-SupportedPINLengths: 4\r\n";
        pairing.transport.request("POST", "/pair-pin-start", &headers, &[])?;
        Ok(pairing)
    }
    fn headers(&self, verify: bool) -> String {
        let mut headers = format!(
            "Content-Type: application/octet-stream\r\nX-Apple-HKP: 5\r\nX-Apple-Client-Name: Horizon\r\nX-Apple-Client-ID: {}\r\n",
            self.identity
        );
        if verify {
            headers.push_str("X-Apple-PD: 1\r\n");
        }
        headers
    }
    fn exchange(&mut self, verify: bool, fields: &[(u8, &[u8])], state: u8) -> Result<Message> {
        let path = if verify { "/pair-verify" } else { "/pair-setup" };
        let headers = self.headers(verify);
        let response = self.transport.request("POST", path, &headers, &tlv::encode(fields))?;
        let response = Message::parse(&response.body)?;
        response.state(state, if verify { "pair-verify" } else { "pair-setup" })?;
        Ok(response)
    }
    /// Complete pairing and verify the receiver before enabling encryption.
    /// The PIN is consumed and zeroed; it is never logged or persisted.
    ///
    /// # Errors
    /// Returns authentication errors for an incorrect PIN, missing proof or signature.
    pub fn finish(mut self, pin: Zeroizing<String>) -> Result<PairedReceiver> {
        let challenge = self.exchange(false, &[(0, &[0]), (6, &[1])], 2)?;
        let proof = srp::exchange(&pin, challenge.get(2)?, challenge.get(3)?)?;
        drop(pin);
        let response = self.exchange(false, &[(6, &[3]), (3, &proof.public), (4, &proof.proof)], 4)?;
        // Compare MAC-sized proofs through ring's constant-time HMAC verification.
        let compare_key = ring::hmac::Key::new(ring::hmac::HMAC_SHA512, &proof.expected);
        let expected = ring::hmac::sign(&compare_key, &proof.expected);
        ring::hmac::verify(&compare_key, response.get(4)?, expected.as_ref()).map_err(|_| Error::Authentication)?;
        let encrypt =
            crypto::key(crypto::derive(&proof.secret, "Pair-Setup-Encrypt-Salt", "Pair-Setup-Encrypt-Info")?.as_ref())?;
        let prefix = crypto::derive(
            &proof.secret,
            "Pair-Setup-Controller-Sign-Salt",
            "Pair-Setup-Controller-Sign-Info",
        )?;
        let signed = [
            prefix.as_ref(),
            self.identity.as_bytes(),
            self.signing.public_key().as_ref(),
        ]
        .concat();
        let signature = self.signing.sign(&signed);
        // OPACK dictionary {"com.apple.ScreenCapture": true}, required by HKP type 5.
        let inner = tlv::encode(&[
            (1, self.identity.as_bytes()),
            (3, self.signing.public_key().as_ref()),
            (10, signature.as_ref()),
            (18, SCREEN_CAPTURE_ACL),
        ]);
        let encrypted = crypto::seal(&encrypt, *b"PS-Msg05", &[], inner)?;
        let response = self.exchange(false, &[(6, &[5]), (5, &encrypted)], 6)?;
        let decrypted = crypto::open(&encrypt, *b"PS-Msg06", &[], response.get(5)?.to_vec())?;
        let receiver = Message::parse(&decrypted)?;
        let identifier = receiver.get(1)?;
        let public = receiver.get(3)?;
        let receiver_prefix = crypto::derive(
            &proof.secret,
            "Pair-Setup-Accessory-Sign-Salt",
            "Pair-Setup-Accessory-Sign-Info",
        )?;
        verify(
            public,
            &[receiver_prefix.as_ref(), identifier, public].concat(),
            receiver.get(10)?,
        )?;
        self.credentials.receiver_id = identifier.to_vec();
        self.credentials.receiver_key = public.to_vec();
        self.verify(identifier, public)
    }
    fn verify(mut self, identifier: &[u8], public: &[u8]) -> Result<PairedReceiver> {
        let ephemeral = agreement::EphemeralPrivateKey::generate(&agreement::X25519, &SystemRandom::new())
            .map_err(|_| Error::Authentication)?;
        let client = ephemeral.compute_public_key().map_err(|_| Error::Authentication)?;
        let response = self.exchange(true, &[(6, &[1]), (3, client.as_ref())], 2)?;
        let remote = response.get(3)?;
        let shared = agreement::agree_ephemeral(
            ephemeral,
            &agreement::UnparsedPublicKey::new(&agreement::X25519, remote),
            |shared| Zeroizing::new(shared.to_vec()),
        )
        .map_err(|_| Error::Authentication)?;
        let encryption =
            crypto::key(crypto::derive(&shared, "Pair-Verify-Encrypt-Salt", "Pair-Verify-Encrypt-Info")?.as_ref())?;
        let received = crypto::open(&encryption, *b"PV-Msg02", &[], response.get(5)?.to_vec())?;
        let received = Message::parse(&received)?;
        if received.get(1)? != identifier {
            return Err(Error::Authentication);
        }
        verify(
            public,
            &[remote, identifier, client.as_ref()].concat(),
            received.get(10)?,
        )?;
        let signature = self
            .signing
            .sign(&[client.as_ref(), self.identity.as_bytes(), remote].concat());
        let signed = tlv::encode(&[(1, self.identity.as_bytes()), (10, signature.as_ref())]);
        let sealed = crypto::seal(&encryption, *b"PV-Msg03", &[], signed)?;
        self.exchange(true, &[(6, &[3]), (5, &sealed)], 4)?;
        self.transport.encrypt(&shared, false)?;
        Ok(PairedReceiver {
            transport: self.transport,
            shared,
            address: self.address,
            identity: self.identity,
            credentials: self.credentials,
            _reservation: self.reservation,
        })
    }
}
fn verify(public: &[u8], data: &[u8], signature: &[u8]) -> Result<()> {
    signature::UnparsedPublicKey::new(&signature::ED25519, public)
        .verify(data, signature)
        .map_err(|_| Error::Authentication)
}

impl PairedReceiver {
    /// Verify a new connection using a saved long-term identity, without PIN setup.
    /// # Errors
    /// Returns an error if the TV revoked the pairing, changed identity or is unavailable.
    pub fn connect(address: SocketAddr, credentials: PairingCredentials) -> Result<Self> {
        Self::connect_cancellable(address, credentials, None)
    }
    pub(crate) fn connect_cancellable(
        address: SocketAddr,
        credentials: PairingCredentials,
        cancellation: Option<Arc<Cancellation>>,
    ) -> Result<Self> {
        let reservation = Reservation::acquire(address.ip())?;
        let receiver_id = credentials.receiver_id.clone();
        let receiver_key = credentials.receiver_key.clone();
        Pairing {
            transport: Transport::connect_cancellable(address, cancellation)?,
            identity: credentials.client_id.clone(),
            signing: credentials.signing()?,
            credentials,
            address,
            reservation,
        }
        .verify(&receiver_id, &receiver_key)
    }
    /// Query receiver information over the authenticated encrypted connection.
    ///
    /// # Errors
    /// Returns transport, authentication, status or property-list errors.
    pub fn info(&mut self) -> Result<plist::Value> {
        let response = self.transport.request("GET", "/info", "", &[])?;
        Ok(plist::Value::from_reader(std::io::Cursor::new(response.body))?)
    }
}

static RECEIVERS: Mutex<Vec<IpAddr>> = Mutex::new(Vec::new());
struct Reservation(IpAddr);
impl Reservation {
    fn acquire(address: IpAddr) -> Result<Self> {
        let address = address.to_canonical();
        let mut receivers = RECEIVERS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if receivers.contains(&address) {
            return Err(Error::AlreadyCasting);
        }
        receivers.push(address);
        Ok(Self(address))
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        RECEIVERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|address| *address != self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_interrupts_control_setup_before_video_and_releases_receiver() {
        use std::{
            io::{BufRead, Read, Write},
            net::TcpListener,
            sync::{atomic::AtomicBool, mpsc},
            time::{Duration, Instant},
        };
        let listener = TcpListener::bind("127.0.0.96:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let cancel = Arc::new(Cancellation::new(Arc::new(AtomicBool::new(false))));
        let mut credentials = PairingCredentials::new().expect("identity");
        credentials.receiver_id = b"synthetic".to_vec();
        credentials.receiver_key = vec![3; 32];
        let receiver = PairedReceiver {
            transport: Transport::connect_cancellable(address, Some(cancel.clone())).expect("transport"),
            shared: Zeroizing::new(vec![9; 32]),
            address,
            identity: credentials.client_id.clone(),
            credentials,
            _reservation: Reservation::acquire(address.ip()).expect("reservation"),
        };
        let (send, blocked) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (socket, _) = listener.accept().expect("accept");
            socket.set_read_timeout(Some(Duration::from_secs(2))).expect("timeout");
            let mut socket = std::io::BufReader::new(socket);
            let mut first = String::new();
            socket.read_line(&mut first).expect("info");
            assert!(first.starts_with("GET /info "));
            loop {
                let mut line = String::new();
                socket.read_line(&mut line).expect("header");
                if line == "\r\n" {
                    break;
                }
            }
            let mut info = plist::Dictionary::new();
            info.insert("model".into(), "AppleTV14,1".into());
            info.insert("features".into(), (1u64 << 41).into());
            let mut body = Vec::new();
            plist::Value::Dictionary(info)
                .to_writer_binary(&mut body)
                .expect("info body");
            write!(
                socket.get_mut(),
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .expect("response");
            socket.get_mut().write_all(&body).expect("info bytes");
            first.clear();
            socket.read_line(&mut first).expect("setup");
            assert!(first.starts_with("SETUP /stream "));
            let mut length = 0;
            loop {
                let mut line = String::new();
                socket.read_line(&mut line).expect("header");
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.strip_prefix("Content-Length: ") {
                    length = value.trim().parse().expect("length");
                }
            }
            socket.read_exact(&mut vec![0; length]).expect("setup body");
            send.send(()).expect("blocked setup");
            assert_eq!(socket.read(&mut [0]).expect("cancel closes setup"), 0);
        });
        let worker = std::thread::spawn(move || receiver.mirror(crate::VideoFormat::default()));
        blocked.recv_timeout(Duration::from_secs(2)).expect("setup blocked");
        let started = Instant::now();
        cancel.stop();
        while !worker.is_finished() {
            assert!(started.elapsed() < Duration::from_secs(1));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(worker.join().expect("worker").is_err());
        assert!(Reservation::acquire(address.ip()).is_ok());
        server.join().expect("server");
    }

    #[test]
    fn receiver_reservations_are_independent_and_released() {
        let first = "192.0.2.1".parse().expect("fixture");
        let second = "192.0.2.2".parse().expect("fixture");
        let reservation = Reservation::acquire(first).expect("first TV");
        let other = Reservation::acquire(second).expect("independent TV");
        assert!(matches!(Reservation::acquire(first), Err(Error::AlreadyCasting)));
        assert!(matches!(
            Reservation::acquire("::ffff:192.0.2.1".parse().expect("mapped")),
            Err(Error::AlreadyCasting)
        ));
        drop(reservation);
        assert!(Reservation::acquire(first).is_ok());
        assert!(matches!(Reservation::acquire(second), Err(Error::AlreadyCasting)));
        drop(other);
    }
}
