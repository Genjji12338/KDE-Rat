//! rustls server config with *mandatory* client certificates.
//!
//! Phones use self-signed certs, so there's no CA to verify against. The
//! verifier accepts any well-formed client cert but still checks the handshake
//! signature (proof the phone holds the private key). Whether that cert is
//! trusted is decided afterwards by fingerprint in `pairing`.

use std::sync::Arc;

use anyhow::Result;
use rustls::client::danger::HandshakeSignatureValid;
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, ServerConfig, SignatureScheme};
use rustls_pki_types::{CertificateDer, UnixTime};

use crate::identity::Identity;

#[derive(Debug)]
struct AnySelfSignedClient {
    algs: WebPkiSupportedAlgorithms,
}

impl ClientCertVerifier for AnySelfSignedClient {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(&self, _end: &CertificateDer<'_>, _chain: &[CertificateDer<'_>], _now: UnixTime) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(&self, msg: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(msg, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(&self, msg: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(msg, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }
}

pub fn server_config(id: &Identity) -> Result<Arc<ServerConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(AnySelfSignedClient { algs: provider.signature_verification_algorithms });
    let cfg = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![id.cert.clone()], id.key.clone_key())?;
    Ok(Arc::new(cfg))
}
