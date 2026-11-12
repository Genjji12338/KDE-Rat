//! turad's long-lived TLS identity: a self-signed certificate stored in
//! `~/.config/tunnelrat/`. Phones pin its SHA-256 fingerprint from the QR code,
//! so no CA is involved.

use std::fs;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use sha2::{Digest, Sha256};

pub struct Identity {
    pub cert: CertificateDer<'static>,
    pub key: PrivateKeyDer<'static>,
    /// Lowercase hex SHA-256 of the certificate DER.
    pub fingerprint: String,
}

pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    base.join("tunnelrat")
}

pub fn fingerprint(cert_der: &[u8]) -> String {
    hex::encode(Sha256::digest(cert_der))
}

pub fn load_or_create(dir: &Path, hostname: &str) -> Result<Identity> {
    let cert_path = dir.join("turad-cert.pem");
    let key_path = dir.join("turad-key.pem");

    if !cert_path.exists() || !key_path.exists() {
        fs::create_dir_all(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        let mut params = rcgen::CertificateParams::new(vec![hostname.to_owned()])?;
        params.distinguished_name.push(rcgen::DnType::CommonName, format!("turad on {hostname}"));
        // Long-lived on purpose: the pin is the trust anchor, not the expiry.
        params.not_after = rcgen::date_time_ymd(2099, 1, 1);
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let cert = params.self_signed(&key)?;
        write_private(&key_path, key.serialize_pem().as_bytes())?;
        fs::write(&cert_path, cert.pem())?;
        tracing::info!(path = %cert_path.display(), "generated new TLS identity");
    }

    let cert = CertificateDer::from_pem_file(&cert_path).with_context(|| format!("reading {}", cert_path.display()))?;
    let key = PrivateKeyDer::from_pem_file(&key_path).with_context(|| format!("reading {}", key_path.display()))?;
    let fingerprint = fingerprint(&cert);
    Ok(Identity { cert, key, fingerprint })
}

fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(data)?;
    Ok(())
}
