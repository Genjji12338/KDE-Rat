//! Receiving files shared from a phone into the user's Downloads folder.
//!
//! Data lands in `<name>.tunnelrat-part` and is renamed to a free
//! `<name>`, `<name> (1)`, … only when complete, so a cut connection never
//! leaves a half file under the real name and nothing is overwritten.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::Engine;
use tokio::io::AsyncWriteExt;
use tura_proto::MAX_FILE;

struct Incoming {
    file: tokio::fs::File,
    part: PathBuf,
    name: String,
    size: u64,
    received: u64,
}

/// Per-connection transfers in progress.
#[derive(Default)]
pub struct Receiver {
    open: HashMap<u32, Incoming>,
}

/// The XDG download dir ("Downloads", or localized), falling back to ~/Downloads.
pub fn download_dir() -> PathBuf {
    if let Ok(out) = std::process::Command::new("xdg-user-dir").arg("DOWNLOAD").output() {
        let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        if p.is_absolute() && p.is_dir() {
            return p;
        }
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Downloads")
}

/// Keeps only a plain file name: no directories, no control characters,
/// no leading dots (hidden files), bounded length.
pub fn sanitize(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim();
    let mut out: String = cleaned.chars().take(180).collect();
    if out.is_empty() {
        out = "shared-file".into();
    }
    out
}

/// `photo.jpg` → `photo (1).jpg`, `photo (2).jpg`, … until unused.
fn free_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("infinite iterator")
}

impl Receiver {
    pub async fn start(&mut self, id: u32, name: &str, size: u64) -> Result<()> {
        if size > MAX_FILE {
            bail!("file too large ({} MB)", size / 1_000_000);
        }
        if self.open.len() >= 8 {
            bail!("too many transfers at once");
        }
        let dir = download_dir();
        tokio::fs::create_dir_all(&dir).await?;
        let name = sanitize(name);
        let part = dir.join(format!(".{name}.{id}.tunnelrat-part"));
        let file = tokio::fs::File::create(&part).await.with_context(|| format!("creating {}", part.display()))?;
        if let Some(old) = self.open.insert(id, Incoming { file, part, name, size, received: 0 }) {
            let _ = tokio::fs::remove_file(old.part).await;
        }
        Ok(())
    }

    pub async fn chunk(&mut self, id: u32, data_b64: &str) -> Result<()> {
        let Some(inc) = self.open.get_mut(&id) else { bail!("unknown transfer {id}") };
        let data = base64::engine::general_purpose::STANDARD.decode(data_b64).context("bad chunk encoding")?;
        if inc.received + data.len() as u64 > inc.size {
            let inc = self.open.remove(&id).expect("present");
            let _ = tokio::fs::remove_file(&inc.part).await;
            bail!("{} sent more data than announced", inc.name);
        }
        inc.file.write_all(&data).await?;
        inc.received += data.len() as u64;
        Ok(())
    }

    /// Finishes a transfer; returns the final path.
    pub async fn end(&mut self, id: u32) -> Result<PathBuf> {
        let Some(mut inc) = self.open.remove(&id) else { bail!("unknown transfer {id}") };
        inc.file.flush().await?;
        drop(inc.file);
        if inc.received != inc.size {
            let _ = tokio::fs::remove_file(&inc.part).await;
            bail!("{} is incomplete ({} of {} bytes)", inc.name, inc.received, inc.size);
        }
        let dest = free_path(inc.part.parent().expect("in download dir"), &inc.name);
        tokio::fs::rename(&inc.part, &dest).await?;
        Ok(dest)
    }

    /// Deletes partial files of transfers cut off by a disconnect.
    pub async fn abort_all(&mut self) {
        for (_, inc) in self.open.drain() {
            drop(inc.file);
            let _ = tokio::fs::remove_file(inc.part).await;
        }
    }
}

/// Only plain http(s) links without whitespace/control chars are opened.
pub fn valid_url(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://"))
        && url.len() <= 4096
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize("../../.bashrc"), "bashrc");
        assert_eq!(sanitize("C:\\x\\photo.jpg"), "photo.jpg");
        assert_eq!(sanitize("a\nb.txt"), "ab.txt");
        assert_eq!(sanitize("..."), "shared-file");
        assert_eq!(sanitize(""), "shared-file");
    }

    #[test]
    fn urls() {
        assert!(valid_url("https://example.com/a?b=c"));
        assert!(!valid_url("file:///etc/passwd"));
        assert!(!valid_url("javascript:alert(1)"));
        assert!(!valid_url("https://x.com/a b"));
    }

    #[tokio::test]
    async fn receives_and_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("tura-files-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "old").unwrap();
        assert_eq!(free_path(&dir, "a.txt"), dir.join("a (1).txt"));
        assert_eq!(free_path(&dir, "b"), dir.join("b"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
