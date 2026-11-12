//! Shows another device's screen here: runs `tura-viewer` (a small GTK
//! window), feeds it the device's H.264 frames and forwards the window's
//! input back: touch gestures for a phone that mirrors itself, or pointer,
//! button, wheel and key events for a PC whose screen we cast.

use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{info, warn};
use tura_proto::Message;

/// Frames buffered for the viewer before we skip to the next keyframe.
const QUEUE: usize = 8;

pub struct Mirror {
    _viewer: Child,
    frames: mpsc::Sender<Vec<u8>>,
    skipping: bool,
    /// Set when we close the viewer ourselves: then it's not "closed by the user".
    dropped: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for Mirror {
    fn drop(&mut self) {
        self.dropped.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// `tura-viewer` next to turad, else on $PATH.
pub fn viewer_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| Some(p.parent()?.join("tura-viewer")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("tura-viewer"))
}

impl Mirror {
    /// Opens the viewer window for `name`'s `width`×`height` screen. Input
    /// from it goes to `out`; closing it sends `MirrorStop` there.
    pub fn start(name: &str, width: u32, height: u32, out: mpsc::UnboundedSender<WsMessage>, pc: bool) -> Result<Mirror> {
        let mut viewer = Command::new(viewer_path())
            .args(["--title", name, "--width", &width.to_string(), "--height", &height.to_string(), "--mode", if pc { "pc" } else { "phone" }])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("starting tura-viewer (install it next to turad)")?;
        let mut stdin = viewer.stdin.take().context("viewer stdin")?;
        let stdout = viewer.stdout.take().context("viewer stdout")?;
        if let Some(err) = viewer.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(err).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    if !l.contains("VK_SUBOPTIMAL_KHR") {
                        warn!("viewer: {l}");
                    }
                }
            });
        }

        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(QUEUE);
        tokio::spawn(async move {
            while let Some(au) = rx.recv().await {
                let len = (au.len() as u32).to_be_bytes();
                if stdin.write_all(&len).await.is_err() || stdin.write_all(&au).await.is_err() {
                    break;
                }
            }
        });

        let text = |m: Message| WsMessage::Text(m.to_json().into());
        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let by_us = dropped.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // Only input for the phone passes; anything else is ignored.
                match Message::from_json(&line) {
                    Ok(m @ (Message::PhoneGesture { .. } | Message::PhoneTouch { .. } | Message::PhoneNav { .. } | Message::PhoneText { .. })) if !pc => {
                        if out.send(text(m)).is_err() {
                            return;
                        }
                    }
                    Ok(m @ (Message::PointerAbs { .. } | Message::PointerButton { .. } | Message::Scroll { .. } | Message::Key { .. })) if pc => {
                        if out.send(text(m)).is_err() {
                            return;
                        }
                    }
                    _ => warn!("viewer sent something unexpected"),
                }
            }
            // The user closed the window: tell the phone to stop.
            if !by_us.load(std::sync::atomic::Ordering::SeqCst) {
                let _ = out.send(text(if pc { Message::CastStop } else { Message::MirrorStop }));
            }
        });
        info!(width, height, "showing {name}'s screen");
        Ok(Mirror { _viewer: viewer, frames: tx, skipping: false, dropped })
    }

    /// One access unit from the phone. If the viewer falls behind, skips to
    /// the next keyframe (later frames depend on the skipped ones).
    pub fn feed(&mut self, au: &[u8]) {
        if self.skipping && !crate::cast::is_keyframe(au) {
            return;
        }
        match self.frames.try_send(au.to_vec()) {
            Ok(()) => self.skipping = false,
            Err(mpsc::error::TrySendError::Full(_)) => self.skipping = true,
            Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }
}
