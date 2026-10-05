//! TLS transport to a receiver's Cast v2 port.
use crate::Result;
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use std::{
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};

pub(crate) type TlsStream = StreamOwned<ClientConnection, TcpStream>;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Opens the TLS channel. `poll` bounds each blocking read so a single I/O
/// thread can interleave reads, queued writes and heartbeats.
pub(crate) fn connect(address: SocketAddr, poll: Duration) -> Result<TlsStream> {
    let socket = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT)?;
    socket.set_nodelay(true)?;
    socket.set_write_timeout(Some(CONNECT_TIMEOUT))?;
    socket.set_read_timeout(Some(CONNECT_TIMEOUT))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(ReceiverCertificate(provider)))
        .with_no_client_auth();
    let connection = ClientConnection::new(Arc::new(config), ServerName::IpAddress(address.ip().into()))?;
    let mut stream = StreamOwned::new(connection, socket);
    while stream.conn.is_handshaking() {
        stream.conn.complete_io(&mut stream.sock)?;
    }
    stream.sock.set_read_timeout(Some(poll))?;
    Ok(stream)
}

/// Receivers present a self-signed certificate generated on the device, so no
/// chain can be built. The handshake signature is still verified, which keeps
/// the session bound to the key that was presented. The channel carries no
/// credentials; receiver authenticity checks are outside this crate's scope.
#[derive(Debug)]
struct ReceiverCertificate(Arc<CryptoProvider>);

impl ServerCertVerifier for ReceiverCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
