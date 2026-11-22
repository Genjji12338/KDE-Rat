//! Desktop clipboard bridge using the standard wl-clipboard tools on Wayland
//! or xclip on X11. Device status is retained for API compatibility; GNOME
//! top-bar indicators are intentionally not part of this desktop-neutral path.

use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::Path,
    process::Stdio,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use tokio::{io::AsyncWriteExt, process::Command, sync::broadcast, time::timeout};

const POLL: Duration = Duration::from_millis(600);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy)]
enum Backend {
    Wayland,
    X11,
}

#[derive(Clone)]
struct DeviceStatus {
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    kind: &'static str,
    #[allow(dead_code)]
    battery: Option<u8>,
    #[allow(dead_code)]
    charging: bool,
}

pub struct Shell {
    /// Text last seen on either side, to break PC ↔ phone echo loops.
    last_clip: Mutex<String>,
    /// PC clipboard changes, for phone sessions.
    clip_tx: broadcast::Sender<String>,
    /// Last device status, retained for future desktop panel integrations.
    status: Mutex<HashMap<String, DeviceStatus>>,
    hidden: Mutex<HashSet<String>>,
    /// True while a desktop clipboard backend is responding.
    active: AtomicBool,
}

impl Shell {
    pub fn new() -> Self {
        Self {
            last_clip: Mutex::new(String::new()),
            clip_tx: broadcast::channel(8).0,
            status: Mutex::new(HashMap::new()),
            hidden: Mutex::new(HashSet::new()),
            active: AtomicBool::new(false),
        }
    }

    pub fn clipboard_changes(&self) -> broadcast::Receiver<String> {
        self.clip_tx.subscribe()
    }

    /// Passes a device's clipboard on to every other connected device.
    pub fn relay(&self, text: &str) {
        let _ = self.clip_tx.send(text.to_owned());
    }

    /// Phone → PC clipboard. Returns Ok(false) if it was already there.
    pub async fn set_clipboard(&self, text: &str) -> Result<bool> {
        if *self.last_clip.lock().unwrap_or_else(|p| p.into_inner()) == text {
            return Ok(false);
        }
        let backend = clipboard_backend().context(
            "PC clipboard unavailable: install wl-clipboard (Wayland) or xclip (X11) and make sure the desktop session is active",
        )?;
        write_clipboard(backend, text).await?;
        *self.last_clip.lock().unwrap_or_else(|p| p.into_inner()) = text.to_owned();
        Ok(true)
    }

    /// Records device status. A panel indicator is desktop-specific and is not
    /// exposed by this backend.
    pub async fn set_device_status(
        &self,
        id: &str,
        name: &str,
        kind: &'static str,
        battery: Option<u8>,
        charging: bool,
        online: bool,
    ) {
        let mut status = self.status.lock().unwrap_or_else(|p| p.into_inner());
        if online {
            status.insert(
                id.to_owned(),
                DeviceStatus {
                    name: name.to_owned(),
                    kind,
                    battery,
                    charging,
                },
            );
        } else {
            status.remove(id);
        }
    }

    pub fn active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    pub async fn set_hidden(&self, ids: &[String]) {
        *self.hidden.lock().unwrap_or_else(|p| p.into_inner()) = ids.iter().cloned().collect();
    }

    /// Watches for desktop clipboard changes. Polling is used because the
    /// wl-clipboard and xclip command-line interfaces do not provide a common
    /// cross-desktop change signal.
    pub async fn run(&self) {
        let mut was_active = false;
        loop {
            let Some(backend) = clipboard_backend() else {
                self.active.store(false, Ordering::Relaxed);
                if was_active {
                    tracing::info!("desktop clipboard backend unavailable; clipboard sync paused");
                    was_active = false;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            };

            match read_clipboard(backend).await {
                Ok(text) => {
                    self.active.store(true, Ordering::Relaxed);
                    if !was_active {
                        tracing::info!("desktop clipboard connected");
                        was_active = true;
                    }
                    if let Some(text) = text {
                        let changed = {
                            let mut last = self.last_clip.lock().unwrap_or_else(|p| p.into_inner());
                            if *last == text {
                                false
                            } else {
                                *last = text.clone();
                                true
                            }
                        };
                        if changed {
                            let _ = self.clip_tx.send(text);
                        }
                    }
                }
                Err(e) => {
                    self.active.store(false, Ordering::Relaxed);
                    if was_active {
                        tracing::warn!("desktop clipboard disconnected: {e:#}");
                        was_active = false;
                    }
                }
            }
            tokio::time::sleep(POLL).await;
        }
    }
}

fn executable(name: &str) -> Option<OsString> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(name);
        if candidate.is_file() && is_executable(&candidate) {
            return Some(candidate.into_os_string());
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn clipboard_backend() -> Option<Backend> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some()
        && executable("wl-paste").is_some()
        && executable("wl-copy").is_some()
    {
        return Some(Backend::Wayland);
    }
    if std::env::var_os("DISPLAY").is_some() && executable("xclip").is_some() {
        return Some(Backend::X11);
    }
    None
}

async fn read_clipboard(backend: Backend) -> Result<Option<String>> {
    let (program, args): (OsString, &[&str]) = match backend {
        Backend::Wayland => (
            executable("wl-paste").context("wl-paste missing")?,
            &["--no-newline", "--type", "text/plain"],
        ),
        Backend::X11 => (
            executable("xclip").context("xclip missing")?,
            &["-selection", "clipboard", "-o"],
        ),
    };
    let output = timeout(
        COMMAND_TIMEOUT,
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .context("clipboard read timed out")??;
    if !output.status.success() {
        // wl-paste uses exit status 1 for the normal empty-selection state.
        // Keep the bridge healthy, but do not overwrite the other devices'
        // clipboards with an empty string.
        if matches!(backend, Backend::Wayland)
            && String::from_utf8_lossy(&output.stderr).trim() == "Nothing is copied"
        {
            return Ok(None);
        }
        bail!("clipboard reader exited with {}", output.status);
    }
    String::from_utf8(output.stdout)
        .map(Some)
        .context("clipboard is not valid UTF-8 text")
}

async fn write_clipboard(backend: Backend, text: &str) -> Result<()> {
    let (program, args): (OsString, &[&str]) = match backend {
        Backend::Wayland => (
            executable("wl-copy").context("wl-copy missing")?,
            &["--type", "text/plain"],
        ),
        Backend::X11 => (
            executable("xclip").context("xclip missing")?,
            &["-selection", "clipboard", "-i"],
        ),
    };
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("starting clipboard writer")?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .await
            .context("writing clipboard text")?;
    }
    let status = timeout(COMMAND_TIMEOUT, child.wait())
        .await
        .context("clipboard write timed out")??;
    if !status.success() {
        bail!("clipboard writer exited with {status}");
    }
    Ok(())
}
