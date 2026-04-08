//! Test certificate generation and fixtures.

use std::sync::Arc;

use rcgen::{
    CertificateParams,
    KeyPair,
};
use rustls::pki_types::{
    CertificateDer,
    PrivateKeyDer,
    PrivatePkcs8KeyDer,
};

/// A self-signed certificate for testing.
pub struct TestCertificate {
    /// Server TLS config using this certificate.
    pub server_config: rustls::ServerConfig,
    /// Client TLS config that trusts this certificate.
    pub client_config: rustls::ClientConfig,
    /// The DER-encoded certificate.
    pub der: CertificateDer<'static>,
}

impl TestCertificate {
    /// Generate a self-signed certificate suitable for testing.
    pub fn generate() -> Self {
        let key_pair = KeyPair::generate().expect("failed to generate key pair");
        let mut params = CertificateParams::new(vec!["localhost".into()]).unwrap();
        params.distinguished_name = rcgen::DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "ngrok-test");

        let cert = params
            .self_signed(&key_pair)
            .expect("failed to self-sign certificate");

        let cert_der = CertificateDer::from(cert.der().to_vec());
        let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));

        // Server config.
        let server_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der.clone()], key_der)
            .expect("failed to build server TLS config");

        // Client config that trusts the self-signed cert.
        let mut root_store = rustls::RootCertStore::empty();
        root_store
            .add(cert_der.clone())
            .expect("failed to add cert to root store");

        let client_config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        Self {
            server_config,
            client_config,
            der: cert_der,
        }
    }
}

/// Create a `rustls::ClientConfig` that accepts any certificate (for testing only).
pub fn danger_accept_any_cert_config() -> rustls::ClientConfig {
    let verifier = Arc::new(NoVerifier);

    rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth()
}

/// A TLS certificate verifier that accepts any certificate.
#[derive(Debug)]
struct NoVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ED448,
        ]
    }
}
