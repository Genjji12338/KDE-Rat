//! Bluetooth link: phones that can't reach this PC over the network connect
//! over Bluetooth (RFCOMM) instead. BlueZ hands us each connection as a
//! socket, and it runs the exact same mutual-TLS + WebSocket session as TCP,
//! so pairing, identity and every feature work unchanged (screens are just
//! too much for Bluetooth's bandwidth).
//!
//! Security comes from the TLS layer, not Bluetooth pairing: the phone needn't
//! be paired with the PC in the system's Bluetooth settings.
//!
//! SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, info, warn};
use zbus::Connection;
use zbus::{ObjectPath, OwnedObjectPath, OwnedVariant as OwnedValue, Variant as Value};

use crate::server::Ctx;

/// Tunnelrat's RFCOMM service; the phone app connects to the same UUID.
pub const UUID: &str = "7e1a7a75-9a4e-4b1c-8d2f-74756e6e6c72";
const PATH: &str = "/dev/tunnelrat/bluetooth";
/// RFCOMM channels to listen on, first free one wins (the phone finds it via SDP).
const CHANNELS: [u16; 4] = [27, 25, 23, 21];

struct Profile {
    tx: mpsc::UnboundedSender<(OwnedObjectPath, std::os::fd::OwnedFd)>,
}

#[zbus::interface(name = "org.bluez.Profile1")]
impl Profile {
    async fn release(&self) {}

    async fn new_connection(&self, device: OwnedObjectPath, fd: zbus::OwnedFd, _props: HashMap<String, OwnedValue>) {
        let _ = self.tx.send((device, fd.into()));
    }

    async fn request_disconnection(&self, _device: OwnedObjectPath) {}
}

/// Accepts phones over Bluetooth for as long as turad runs; waits for an
/// adapter / bluetoothd to appear and re-registers when bluetoothd restarts.
pub async fn run(acceptor: TlsAcceptor, ctx: Arc<Ctx>) {
    let mut said = false;
    loop {
        match serve(&acceptor, &ctx).await {
            Ok(()) => info!("Bluetooth went away; waiting for it to return"),
            Err(e) if !said => {
                debug!("no Bluetooth link: {e:#}");
                said = true;
            }
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

async fn serve(acceptor: &TlsAcceptor, ctx: &Arc<Ctx>) -> Result<()> {
    let conn = Connection::system().await?;
    adapter_address(&conn).await.context("no powered Bluetooth adapter")?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    conn.object_server().at(PATH, Profile { tx }).await?;
    // BlueZ only listens (and puts a channel in the SDP record) for a custom
    // UUID when given an RFCOMM channel; try a few rarely used ones.
    let mut registered = None;
    for channel in CHANNELS {
        let mut opts: HashMap<&str, Value> = HashMap::new();
        opts.insert("Name", Value::from("Tunnelrat"));
        opts.insert("Role", Value::from("server"));
        opts.insert("Channel", Value::from(channel));
        opts.insert("RequireAuthentication", Value::from(false));
        opts.insert("RequireAuthorization", Value::from(false));
        opts.insert("AutoConnect", Value::from(false));
        match conn.call_method(Some("org.bluez"), "/org/bluez", Some("org.bluez.ProfileManager1"), "RegisterProfile", &(ObjectPath::try_from(PATH)?, UUID, opts)).await {
            Ok(_) => {
                registered = Some(channel);
                break;
            }
            Err(e) => debug!(channel, "RFCOMM channel unavailable: {e}"),
        }
    }
    let channel = registered.context("registering the Bluetooth service: no free RFCOMM channel")?;
    info!(channel, "accepting phones over Bluetooth too");

    let dbus = zbus::fdo::DBusProxy::new(&conn).await?;
    let mut owners = dbus.receive_name_owner_changed().await?;
    loop {
        tokio::select! {
            Some((device, fd)) = rx.recv() => {
                let (acceptor, ctx) = (acceptor.clone(), ctx.clone());
                let label = format!("Bluetooth {}", mac_of(&device));
                tokio::spawn(async move {
                    match stream(fd) {
                        Ok(s) => crate::server::accept_phone(&acceptor, s, label, ctx).await,
                        Err(e) => warn!("{label}: {e:#}"),
                    }
                });
            }
            Some(sig) = owners.next() => {
                if let Ok(a) = sig.args()
                    && a.name().as_str() == "org.bluez"
                {
                    // bluetoothd restarted or stopped: our profile is gone.
                    return Ok(());
                }
            }
            else => bail!("lost the system bus"),
        }
    }
}

/// The RFCOMM socket as a tokio stream (plain read/write on the fd).
fn stream(fd: std::os::fd::OwnedFd) -> Result<tokio::net::UnixStream> {
    let s = std::os::unix::net::UnixStream::from(fd);
    s.set_nonblocking(true)?;
    Ok(tokio::net::UnixStream::from_std(s)?)
}

/// "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF" → "AA:BB:CC:DD:EE:FF".
fn mac_of(device: &OwnedObjectPath) -> String {
    device.as_str().rsplit('/').next().unwrap_or("").trim_start_matches("dev_").replace('_', ":")
}

/// The first powered adapter's address, e.g. "AA:BB:CC:DD:EE:FF".
pub async fn adapter_address(conn: &Connection) -> Option<String> {
    let reply = conn
        .call_method(Some("org.bluez"), "/", Some("org.freedesktop.DBus.ObjectManager"), "GetManagedObjects", &())
        .await
        .ok()?;
    let objects: HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>> = reply.body().deserialize().ok()?;
    let mut adapters: Vec<_> = objects.iter().filter_map(|(path, ifaces)| ifaces.get("org.bluez.Adapter1").map(|a| (path.as_str(), a))).collect();
    adapters.sort_by_key(|(p, _)| *p);
    adapters.iter().find_map(|(_, a)| {
        let powered = a.get("Powered").and_then(|v| bool::try_from(v).ok()).unwrap_or(false);
        let addr = a.get("Address").and_then(|v| String::try_from(v.try_clone().ok()?).ok())?;
        powered.then_some(addr)
    })
}

/// For `PcInfo`: where the phone can find us over Bluetooth, if anywhere.
pub async fn address() -> Option<String> {
    let conn = Connection::system().await.ok()?;
    tokio::time::timeout(Duration::from_secs(2), adapter_address(&conn)).await.ok().flatten()
}

// ── Pairing: the phone asks over the (authenticated) network link ──────────

const AGENT_PATH: &str = "/dev/tunnelrat/agent";
/// How long the PC accepts the phone's Bluetooth pairing.
const PAIR_WINDOW: Duration = Duration::from_secs(60);
static PAIRING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// BlueZ pairing agent, registered only while a pairing window is open:
/// accepts one device, if it carries the name the phone told us.
struct Agent {
    conn: Connection,
    name: String,
}

impl Agent {
    async fn check(&self, device: &OwnedObjectPath) -> zbus::fdo::Result<()> {
        let name = async {
            let reply = self
                .conn
                .call_method(Some("org.bluez"), device.as_str(), Some("org.freedesktop.DBus.Properties"), "Get", &("org.bluez.Device1", "Name"))
                .await
                .ok()?;
            let v: OwnedValue = reply.body().deserialize().ok()?;
            String::try_from(v).ok()
        }
        .await;
        match name {
            // Not resolved yet during pairing: the short window has to do.
            None => Ok(()),
            Some(n) if n == self.name => Ok(()),
            Some(n) => {
                warn!(expected = %self.name, got = %n, "Bluetooth pairing from another device refused");
                Err(zbus::fdo::Error::AccessDenied("not the phone that asked".into()))
            }
        }
    }
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    async fn release(&self) {}
    async fn request_pin_code(&self, _device: OwnedObjectPath) -> zbus::fdo::Result<String> {
        Err(zbus::fdo::Error::AccessDenied("no PIN pairing".into()))
    }
    async fn display_pin_code(&self, _device: OwnedObjectPath, _pincode: String) {}
    async fn request_passkey(&self, _device: OwnedObjectPath) -> zbus::fdo::Result<u32> {
        Err(zbus::fdo::Error::AccessDenied("no passkey pairing".into()))
    }
    async fn display_passkey(&self, _device: OwnedObjectPath, _passkey: u32, _entered: u16) {}
    async fn request_confirmation(&self, device: OwnedObjectPath, _passkey: u32) -> zbus::fdo::Result<()> {
        self.check(&device).await
    }
    async fn request_authorization(&self, device: OwnedObjectPath) -> zbus::fdo::Result<()> {
        self.check(&device).await
    }
    async fn authorize_service(&self, device: OwnedObjectPath, _uuid: String) -> zbus::fdo::Result<()> {
        self.check(&device).await
    }
    async fn cancel(&self) {}
}

/// Lets the phone named `name` pair over Bluetooth for a minute: our own
/// agent (default while open), the adapter visible and pairable. Calls
/// `ready` once the phone can start bonding. Returns the paired device's
/// address, and always puts the adapter and agents back as they were.
pub async fn pair_window(name: String, ready: impl FnOnce()) -> Result<String> {
    if PAIRING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        bail!("Bluetooth pairing is already open on this PC");
    }
    struct Done;
    impl Drop for Done {
        fn drop(&mut self) {
            PAIRING.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let _done = Done;

    let conn = Connection::system().await?;
    let adapter = adapter_path(&conn).await.context("Bluetooth is off or missing on this PC")?;
    let get = |prop: &'static str| {
        let (conn, adapter) = (conn.clone(), adapter.clone());
        async move {
            let r = conn.call_method(Some("org.bluez"), adapter.as_str(), Some("org.freedesktop.DBus.Properties"), "Get", &("org.bluez.Adapter1", prop)).await?;
            let v: OwnedValue = r.body().deserialize()?;
            anyhow::Ok(bool::try_from(v).unwrap_or(false))
        }
    };
    let set = |prop: &'static str, on: bool| {
        let (conn, adapter) = (conn.clone(), adapter.clone());
        async move {
            conn.call_method(Some("org.bluez"), adapter.as_str(), Some("org.freedesktop.DBus.Properties"), "Set", &("org.bluez.Adapter1", prop, Value::from(on)))
                .await
                .map(|_| ())
        }
    };
    let was = (get("Discoverable").await.unwrap_or(false), get("Pairable").await.unwrap_or(false));

    conn.object_server().at(AGENT_PATH, Agent { conn: conn.clone(), name: name.clone() }).await?;
    let agents = |method: &'static str| {
        let conn = conn.clone();
        async move {
            let path = ObjectPath::try_from(AGENT_PATH)?;
            match method {
                "RegisterAgent" => conn.call_method(Some("org.bluez"), "/org/bluez", Some("org.bluez.AgentManager1"), method, &(path, "DisplayYesNo")).await?,
                _ => conn.call_method(Some("org.bluez"), "/org/bluez", Some("org.bluez.AgentManager1"), method, &(path,)).await?,
            };
            anyhow::Ok(())
        }
    };

    let result = async {
        agents("RegisterAgent").await.context("registering the pairing agent")?;
        agents("RequestDefaultAgent").await.context("becoming the pairing agent")?;
        set("Pairable", true).await.context("making Bluetooth pairable")?;
        set("Discoverable", true).await.context("making Bluetooth visible")?;
        info!(phone = %name, "Bluetooth pairing open for a minute");

        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender("org.bluez")
            .interface("org.freedesktop.DBus.Properties")
            .member("PropertiesChanged")
            .path_namespace(adapter.as_str())
            .build()?;
        let mut signals = zbus::MessageStream::for_match_rule(rule, &conn, None).await?;
        ready();
        tokio::time::timeout(PAIR_WINDOW, async {
            while let Some(Ok(msg)) = signals.next().await {
                let Some(path) = msg.header().path().map(|p| p.to_string()) else { continue };
                let Ok((iface, changed, _)) = msg.body().deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>() else { continue };
                if iface != "org.bluez.Device1" || !changed.get("Paired").and_then(|v| bool::try_from(v).ok()).unwrap_or(false) {
                    continue;
                }
                // Ours: trust it so its connections never ask.
                let _ = conn
                    .call_method(Some("org.bluez"), path.as_str(), Some("org.freedesktop.DBus.Properties"), "Set", &("org.bluez.Device1", "Trusted", Value::from(true)))
                    .await;
                return Ok(mac_of(&OwnedObjectPath::try_from(path)?));
            }
            bail!("Bluetooth stopped")
        })
        .await
        .map_err(|_| anyhow::anyhow!("no phone paired within a minute"))?
    }
    .await;

    if !was.0 {
        let _ = set("Discoverable", false).await;
    }
    if !was.1 {
        let _ = set("Pairable", false).await;
    }
    let _ = agents("UnregisterAgent").await;
    let _ = conn.object_server().remove::<Agent, _>(AGENT_PATH).await;
    match &result {
        Ok(mac) => info!(%mac, "paired over Bluetooth"),
        Err(e) => warn!("Bluetooth pairing: {e:#}"),
    }
    result
}

/// The first powered adapter's object path.
async fn adapter_path(conn: &Connection) -> Option<OwnedObjectPath> {
    let reply = conn
        .call_method(Some("org.bluez"), "/", Some("org.freedesktop.DBus.ObjectManager"), "GetManagedObjects", &())
        .await
        .ok()?;
    let objects: HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>> = reply.body().deserialize().ok()?;
    let mut adapters: Vec<_> = objects
        .into_iter()
        .filter(|(_, ifaces)| ifaces.get("org.bluez.Adapter1").and_then(|a| a.get("Powered")).and_then(|v| bool::try_from(v).ok()).unwrap_or(false))
        .map(|(p, _)| p)
        .collect();
    adapters.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    adapters.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_from_device_path() {
        let p = OwnedObjectPath::try_from("/org/bluez/hci0/dev_AA_BB_CC_00_11_22").unwrap();
        assert_eq!(mac_of(&p), "AA:BB:CC:00:11:22");
    }
}
