//! Paired phones (persisted) and one-time pairing tokens (in memory).
//!
//! A phone is identified by the SHA-256 fingerprint of its TLS client
//! certificate. Pairing = presenting a valid one-time token over a TLS
//! connection; the certificate used for that connection gets stored.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const TOKEN_TTL: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    pub name: String,
    pub fingerprint: String,
    pub paired_at: u64,
    /// "phone" or "pc", learned from its Hello.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

pub struct Pairing {
    path: PathBuf,
    devices: Mutex<Vec<PairedDevice>>,
    tokens: Mutex<Vec<(String, Instant)>>,
}

impl Pairing {
    pub fn load(path: PathBuf) -> Result<Self> {
        let devices = match fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(e) => return Err(e.into()),
        };
        Ok(Self { path, devices: Mutex::new(devices), tokens: Mutex::new(vec![]) })
    }

    pub fn is_paired(&self, fingerprint: &str) -> Option<String> {
        self.devices.lock().unwrap().iter().find(|d| d.fingerprint == fingerprint).map(|d| d.name.clone())
    }

    pub fn list(&self) -> Vec<PairedDevice> {
        self.devices.lock().unwrap().clone()
    }

    /// Removes the device matching `selector`: its name (case-insensitive) or
    /// a fingerprint prefix of at least 4 hex chars. Refuses ambiguous matches.
    pub fn remove(&self, selector: &str) -> Result<PairedDevice> {
        let sel = selector.trim().to_lowercase();
        if sel.is_empty() {
            bail!("give a device name or fingerprint prefix (see `tura devices`)");
        }
        let mut devices = self.devices.lock().unwrap();
        let by_fp = sel.len() >= 4 && sel.chars().all(|c| c.is_ascii_hexdigit());
        let hits: Vec<usize> = devices
            .iter()
            .enumerate()
            .filter(|(_, d)| d.name.to_lowercase() == sel || (by_fp && d.fingerprint.starts_with(&sel)))
            .map(|(i, _)| i)
            .collect();
        match hits.as_slice() {
            [] => bail!("no paired device matches {selector:?}"),
            [i] => {
                let removed = devices.remove(*i);
                self.persist(&devices)?;
                Ok(removed)
            }
            many => {
                let names: Vec<String> = many.iter().map(|&i| format!("{} ({})", devices[i].name, devices[i].fingerprint.get(..12).unwrap_or(&devices[i].fingerprint))).collect();
                bail!("{selector:?} matches several devices: {} — use a longer fingerprint prefix", names.join(", "))
            }
        }
    }

    /// Removes the device with exactly this fingerprint (phone-initiated).
    pub fn remove_fingerprint(&self, fingerprint: &str) -> Result<Option<PairedDevice>> {
        let mut devices = self.devices.lock().unwrap();
        let Some(i) = devices.iter().position(|d| d.fingerprint == fingerprint) else { return Ok(None) };
        let removed = devices.remove(i);
        self.persist(&devices)?;
        Ok(Some(removed))
    }

    fn persist(&self, devices: &[PairedDevice]) -> Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(devices)?)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Creates a fresh one-time token valid for [`TOKEN_TTL`].
    pub fn new_token(&self) -> String {
        let token = hex::encode(rand::random::<[u8; 16]>());
        let mut tokens = self.tokens.lock().unwrap();
        tokens.retain(|(_, exp)| *exp > Instant::now());
        tokens.push((token.clone(), Instant::now() + TOKEN_TTL));
        token
    }

    /// "phone" / "pc", if known.
    pub fn kind(&self, fingerprint: &str) -> Option<String> {
        self.devices.lock().unwrap().iter().find(|d| d.fingerprint == fingerprint).and_then(|d| d.kind.clone())
    }

    /// Remembers whether a device is a phone or a PC.
    pub fn set_kind(&self, fingerprint: &str, kind: &str) {
        let mut devices = self.devices.lock().unwrap();
        let Some(d) = devices.iter_mut().find(|d| d.fingerprint == fingerprint) else { return };
        if d.kind.as_deref() == Some(kind) {
            return;
        }
        d.kind = Some(kind.to_owned());
        let _ = self.persist(&devices);
    }

    /// Consumes `token` and trusts `fingerprint`. Constant-time compare so the
    /// token can't be guessed byte by byte.
    pub fn redeem(&self, token: &str, fingerprint: &str, name: &str) -> Result<bool> {
        let valid = {
            let mut tokens = self.tokens.lock().unwrap();
            tokens.retain(|(_, exp)| *exp > Instant::now());
            match tokens.iter().position(|(t, _)| ct_eq(t.as_bytes(), token.as_bytes())) {
                Some(i) => {
                    tokens.remove(i);
                    true
                }
                None => false,
            }
        };
        if !valid {
            return Ok(false);
        }

        let mut devices = self.devices.lock().unwrap();
        devices.retain(|d| d.fingerprint != fingerprint);
        let paired_at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        devices.push(PairedDevice { name: name.to_owned(), fingerprint: fingerprint.to_owned(), paired_at, kind: None });
        self.persist(&devices)?;
        Ok(true)
    }
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (Pairing, PathBuf) {
        let path = std::env::temp_dir().join(format!("tura-pair-test-{}.json", rand::random::<u64>()));
        (Pairing::load(path.clone()).unwrap(), path)
    }

    #[test]
    fn token_is_one_time_and_persists_device() {
        let (p, path) = store();
        let t = p.new_token();
        assert!(!p.redeem("wrong", "fp1", "phone").unwrap());
        assert!(p.redeem(&t, "fp1", "phone").unwrap());
        assert!(!p.redeem(&t, "fp2", "other").unwrap(), "token reused");
        assert_eq!(p.is_paired("fp1").as_deref(), Some("phone"));
        assert!(p.is_paired("fp2").is_none());
        assert!(Pairing::load(path.clone()).unwrap().is_paired("fp1").is_some());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn remove_by_name_or_prefix_refuses_ambiguity() {
        let (p, path) = store();
        let t = p.new_token();
        p.redeem(&t, "abcd1111", "Pixel").unwrap();
        let t = p.new_token();
        p.redeem(&t, "abcd2222", "Tablet").unwrap();

        assert!(p.remove("abc").is_err(), "3-char prefix must not match");
        assert!(p.remove("abcd").unwrap_err().to_string().contains("several"));
        assert!(p.remove("nope").is_err());
        assert_eq!(p.remove("pixel").unwrap().fingerprint, "abcd1111");
        assert_eq!(p.remove("ABCD2").unwrap().name, "Tablet");
        assert!(p.list().is_empty());
        assert!(Pairing::load(path.clone()).unwrap().list().is_empty(), "removal persisted");
        let _ = fs::remove_file(path);
    }
}
