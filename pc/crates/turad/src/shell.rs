//! Bridge to the Tunnelrat GNOME Shell extension (`dev.tunnelrat.Shell`):
//! clipboard read/write/watch and the top-bar phone indicator.
//!
//! The extension may be missing (not installed / not enabled yet / other
//! desktop); everything here degrades to "no clipboard sync" and retries.

use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use tokio::sync::broadcast;
use zbus::{Connection, MatchRule, MessageStream, fdo::DBusProxy};

const BUS_NAME: &str = "dev.tunnelrat.Shell";
const PATH: &str = "/dev/tunnelrat/Shell";
const IFACE: &str = "dev.tunnelrat.Shell";
const RETRY: Duration = Duration::from_secs(10);

pub struct Shell {
    conn: Mutex<Option<Connection>>,
    /// Text last seen on either side, to break PC ↔ phone echo loops.
    last_clip: Mutex<String>,
    /// PC clipboard changes, for phone sessions.
    clip_tx: broadcast::Sender<String>,
    /// Last indicator state, replayed when the extension (re)starts — at
    /// login it comes up hidden and would stay so until the battery changed.
    status: Mutex<std::collections::HashMap<String, DeviceStatus>>,
    /// Device ids whose chip is hidden (settings).
    hidden: Mutex<std::collections::HashSet<String>>,
    /// The extension is running and we're subscribed (status page).
    active: std::sync::atomic::AtomicBool,
}

#[derive(Clone)]
struct DeviceStatus {
    name: String,
    /// "phone" or "pc" (icon in the top bar).
    kind: &'static str,
    battery: Option<u8>,
    charging: bool,
}

impl Shell {
    pub fn new() -> Self {
        Self {
            conn: Mutex::new(None),
            last_clip: Mutex::new(String::new()),
            clip_tx: broadcast::channel(8).0,
            status: Mutex::new(std::collections::HashMap::new()),
            hidden: Mutex::new(Default::default()),
            active: Default::default(),
        }
    }

    pub fn clipboard_changes(&self) -> broadcast::Receiver<String> {
        self.clip_tx.subscribe()
    }

    fn conn(&self) -> Option<Connection> {
        self.conn.lock().unwrap().clone()
    }

    /// Passes a device's clipboard on to every other connected device (they
    /// ignore text they just sent), so phone, this PC and joined PCs share one.
    pub fn relay(&self, text: &str) {
        let _ = self.clip_tx.send(text.to_owned());
    }

    /// Phone → PC clipboard. Returns Ok(false) if it was already there.
    pub async fn set_clipboard(&self, text: &str) -> Result<bool> {
        {
            let mut last = self.last_clip.lock().unwrap();
            if *last == text {
                return Ok(false);
            }
            *last = text.to_owned();
        }
        let conn = self.conn().context("PC clipboard unavailable: enable the Tunnelrat GNOME extension")?;
        conn.call_method(Some(BUS_NAME), PATH, Some(IFACE), "SetClipboard", &(text,))
            .await
            .context("PC clipboard unavailable: enable the Tunnelrat GNOME extension")?;
        Ok(true)
    }

    /// Updates this device's chip in the top bar (keyed by `id`, e.g. its
    /// fingerprint). Remembered and replayed if the extension restarts.
    /// Silently does nothing without the extension.
    pub async fn set_device_status(&self, id: &str, name: &str, kind: &'static str, battery: Option<u8>, charging: bool, online: bool) {
        {
            let mut st = self.status.lock().unwrap();
            if online {
                st.insert(id.to_owned(), DeviceStatus { name: name.to_owned(), kind, battery, charging });
            } else {
                st.remove(id);
            }
        }
        self.push_status(id, name, kind, battery, charging, online).await;
    }

    pub fn active(&self) -> bool {
        self.active.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Which devices' chips to hide; applied right away.
    pub async fn set_hidden(&self, ids: &[String]) {
        *self.hidden.lock().unwrap() = ids.iter().cloned().collect();
        self.replay_status().await;
    }

    async fn push_status(&self, id: &str, name: &str, kind: &str, battery: Option<u8>, charging: bool, online: bool) {
        let Some(conn) = self.conn() else { return };
        let online = online && !self.hidden.lock().unwrap().contains(id);
        let level: i32 = battery.map(i32::from).unwrap_or(-1);
        let _ = conn.call_method(Some(BUS_NAME), PATH, Some(IFACE), "SetDeviceStatus", &(id, name, kind, level, charging, online)).await;
    }

    async fn replay_status(&self) {
        let all: Vec<(String, DeviceStatus)> = self.status.lock().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (id, d) in all {
            self.push_status(&id, &d.name, d.kind, d.battery, d.charging, true).await;
        }
    }

    /// Runs forever: keeps a subscription to the extension and forwards its
    /// ClipboardChanged signals to `clipboard_changes()` subscribers.
    pub async fn run(&self) {
        loop {
            let res = self.session().await;
            // Drop the (possibly dead) connection so callers don't use it meanwhile.
            *self.conn.lock().unwrap() = None;
            self.active.store(false, std::sync::atomic::Ordering::Relaxed);
            if let Err(e) = res {
                tracing::warn!("GNOME extension bridge: {e:#} (retrying)");
            }
            tokio::time::sleep(RETRY).await;
        }
    }

    async fn session(&self) -> Result<()> {
        let conn = Connection::session().await?;
        *self.conn.lock().unwrap() = Some(conn.clone());
        let dbus = DBusProxy::new(&conn).await?;

        let rule = MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(IFACE).member("ClipboardChanged").build()?;
        let mut signals = MessageStream::for_match_rule(rule, &conn, None).await?;
        let mut owners = dbus.receive_name_owner_changed().await?;

        loop {
            // (Re)subscribe whenever the extension is (re)started.
            let owner = match dbus.get_name_owner(BUS_NAME.try_into()?).await {
                Ok(o) => o.to_string(),
                // Not running yet: wait for it to appear (or retry later).
                Err(zbus::fdo::Error::NameHasNoOwner(_)) => {
                    tokio::select! {
                        // A closed stream means the session bus went away (logout):
                        // bail so run() reconnects instead of spinning.
                        ev = owners.next() => match ev {
                            Some(_) => continue,
                            None => anyhow::bail!("session bus closed"),
                        },
                        _ = tokio::time::sleep(RETRY) => continue,
                    }
                }
                Err(e) => return Err(e).context("GetNameOwner"),
            };
            conn.call_method(Some(BUS_NAME), PATH, Some(IFACE), "Subscribe", &()).await.context("Subscribe")?;
            tracing::info!("GNOME extension connected: clipboard sync active");
            self.active.store(true, std::sync::atomic::Ordering::Relaxed);
            self.replay_status().await;

            loop {
                tokio::select! {
                    Some(Ok(msg)) = signals.next() => {
                        // Only trust signals from the current extension owner.
                        if msg.header().sender().map(|s| s.as_str()) != Some(owner.as_str()) {
                            continue;
                        }
                        let Ok((text,)) = msg.body().deserialize::<(String,)>() else { continue };
                        let changed = {
                            let mut last = self.last_clip.lock().unwrap();
                            let changed = *last != text;
                            if changed {
                                *last = text.clone();
                            }
                            changed
                        };
                        if changed {
                            let _ = self.clip_tx.send(text);
                        }
                    }
                    Some(ev) = owners.next() => {
                        if ev.args().is_ok_and(|a| a.name().as_str() == BUS_NAME) {
                            self.active.store(false, std::sync::atomic::Ordering::Relaxed);
                            tracing::info!("GNOME extension restarted; resubscribing");
                            break;
                        }
                    }
                    else => anyhow::bail!("session bus streams ended"),
                }
            }
        }
    }
}
