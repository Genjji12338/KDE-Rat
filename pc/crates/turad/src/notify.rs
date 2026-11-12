//! Desktop notifications (org.freedesktop.Notifications) with optional
//! "open this file/folder" actions.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Result;
use futures_util::StreamExt;
use zbus::{Connection, MatchRule, MessageStream};

const DEST: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

pub struct Notifier {
    /// Cached session-bus connection; reset on error because the session bus
    /// is replaced on every logout/login while turad keeps running.
    conn: Mutex<Option<Connection>>,
    /// Notification id → file to open when its action is clicked.
    open_on_click: Mutex<HashMap<u32, PathBuf>>,
    /// Notification id → (phone fingerprint, its notification key, title) for "Reply".
    reply_on_click: Mutex<HashMap<u32, (String, String, String)>>,
}

/// Notification servers accept a little markup in the body.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Runs `tunnelrat-app --reply …` (the PC app's small reply window).
fn open_reply_window(device: &str, key: &str, title: &str) {
    let app = crate::settings::which("tunnelrat-app")
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".local/bin/tunnelrat-app")).filter(|p| p.exists()))
        .unwrap_or_else(|| PathBuf::from("tunnelrat-app"));
    if let Err(e) = tokio::process::Command::new(app).args(["--reply", "--device", device, "--key", key, "--title", title]).spawn() {
        tracing::warn!("reply window: {e} (is the Tunnelrat app installed? pc/app/install.sh)");
    }
}

impl Notifier {
    pub fn new() -> Self {
        Self { conn: Mutex::new(None), open_on_click: Mutex::new(HashMap::new()), reply_on_click: Mutex::new(HashMap::new()) }
    }

    async fn conn(&self) -> Result<Connection> {
        if let Some(c) = self.conn.lock().unwrap().clone() {
            return Ok(c);
        }
        let c = Connection::session().await?;
        *self.conn.lock().unwrap() = Some(c.clone());
        Ok(c)
    }

    fn reset(&self) {
        *self.conn.lock().unwrap() = None;
    }

    /// Shows (or replaces, if `replaces` ≠ 0) a notification; returns its id.
    pub async fn show(&self, app: &str, icon: &str, summary: &str, body: &str, replaces: u32) -> Result<u32> {
        self.notify(app, icon, summary, body, replaces, &[]).await
    }

    /// Like [`show`] with an "Open" action that opens `path`.
    pub async fn show_file(&self, app: &str, summary: &str, body: &str, path: PathBuf) -> Result<u32> {
        let id = self.notify(app, "folder-download-symbolic", summary, body, 0, &["default", "Open", "open", "Open"]).await?;
        self.open_on_click.lock().unwrap().insert(id, path);
        Ok(id)
    }

    /// A phone notification with a "Reply" button that opens Tunnelrat's reply window.
    #[allow(clippy::too_many_arguments)]
    pub async fn show_reply(&self, app: &str, summary: &str, body: &str, replaces: u32, device: &str, key: &str, title: &str) -> Result<u32> {
        let id = self.notify(app, "phone-symbolic", summary, body, replaces, &["reply", "Reply"]).await?;
        let mut map = self.reply_on_click.lock().unwrap();
        if map.len() > 300 {
            map.clear();
        }
        map.insert(id, (device.to_owned(), key.to_owned(), title.to_owned()));
        Ok(id)
    }

    async fn notify(&self, app: &str, icon: &str, summary: &str, body: &str, replaces: u32, actions: &[&str]) -> Result<u32> {
        let args = (app, replaces, icon, summary, escape(body), actions, HashMap::<&str, zbus::Variant>::new(), -1i32);
        let reply = match self.conn().await?.call_method(Some(DEST), PATH, Some(DEST), "Notify", &args).await {
            Ok(r) => r,
            Err(_) => {
                // Stale connection after a re-login: reconnect once and retry.
                self.reset();
                self.conn().await?.call_method(Some(DEST), PATH, Some(DEST), "Notify", &args).await?
            }
        };
        Ok(reply.body().deserialize::<(u32,)>()?.0)
    }

    pub async fn close(&self, id: u32) {
        if let Ok(conn) = self.conn().await {
            let _ = conn.call_method(Some(DEST), PATH, Some(DEST), "CloseNotification", &(id,)).await;
        }
    }

    /// Runs forever: opens files when their notification is clicked.
    /// Re-subscribes after the session bus is replaced (logout/login).
    pub async fn run_actions(&self) {
        loop {
            if let Ok(conn) = self.conn().await
                && let Ok(rule) = MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(DEST).member("ActionInvoked").build()
                && let Ok(mut stream) = MessageStream::for_match_rule(rule, &conn, None).await
            {
                while let Some(Ok(msg)) = stream.next().await {
                    let Ok((id, action)) = msg.body().deserialize::<(u32, String)>() else { continue };
                    if action == "reply" {
                        let target = self.reply_on_click.lock().unwrap().get(&id).cloned();
                        if let Some((device, key, title)) = target {
                            open_reply_window(&device, &key, &title);
                        }
                        continue;
                    }
                    let path = self.open_on_click.lock().unwrap().remove(&id);
                    if let Some(path) = path {
                        let _ = tokio::process::Command::new("gio").arg("open").arg(&path).spawn();
                    }
                }
            }
            self.reset();
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    }
}
