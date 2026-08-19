//! TLS connect with certificate verification disabled (typical for Cast).

use std::{
    net::TcpStream,
    sync::Arc,
    time::Duration,
};

use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, StreamOwned,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
};

use super::error::ChannelError;

pub type TlsStream = StreamOwned<ClientConnection, TcpStream>;

pub(super) fn connect(host: &str, port: u16) -> Result<TlsStream, ChannelError> {
    let addr = format!("{host}:{port}");
    let tcp = TcpStream::connect(addr)?;
    tcp.set_nodelay(true)?;
    // Short default timeout: heartbeat must not block SET_VOLUME for seconds.
    tcp.set_read_timeout(Some(Duration::from_millis(250)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(5)))?;

    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoCertVerification))
        .with_no_client_auth();

    let server_name = ServerName::try_from(host.to_string())
        .map_err(|e| ChannelError::Tls(format!("ServerName: {e}")))?;
    let conn = ClientConnection::new(Arc::new(config), server_name)
        .map_err(|e| ChannelError::Tls(e.to_string()))?;
    Ok(StreamOwned::new(conn, tcp))
}

#[derive(Debug)]
struct NoCertVerification;

impl ServerCertVerifier for NoCertVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
