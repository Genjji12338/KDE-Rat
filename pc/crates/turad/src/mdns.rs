//! Announces turad on the local network (mDNS / DNS-SD through Avahi), so a
//! paired phone finds the PC when its address changes (DHCP, new router).
//! The TXT record carries the certificate fingerprint to tell PCs apart;
//! who the PC is gets checked by the pinned TLS certificate, as always.

use std::time::Duration;

use anyhow::{Result, bail};
use futures_util::StreamExt;
use tracing::{info, warn};
use zbus::{Connection, MatchRule, MessageStream, OwnedObjectPath, fdo::DBusProxy};

const AVAHI: &str = "org.freedesktop.Avahi";
const GROUP: &str = "org.freedesktop.Avahi.EntryGroup";
const SERVICE: &str = "_tunnelrat._tcp";

/// Whether the announcement is up (for the status page).
pub static ANNOUNCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Runs forever: keeps the announcement up (re-registers if Avahi restarts).
pub async fn run(port: u16, fingerprint: String) {
    loop {
        let res = announce(port, &fingerprint).await;
        ANNOUNCED.store(false, std::sync::atomic::Ordering::Relaxed);
        if let Err(e) = res {
            warn!("mDNS announcement: {e:#} (retrying)");
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

async fn announce(port: u16, fingerprint: &str) -> Result<()> {
    let conn = Connection::system().await?;
    let dbus = DBusProxy::new(&conn).await?;
    let mut owners = dbus.receive_name_owner_changed().await?;
    let group: OwnedObjectPath = conn.call_method(Some(AVAHI), "/", Some("org.freedesktop.Avahi.Server"), "EntryGroupNew", &()).await?.body().deserialize()?;
    let rule = MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(GROUP).member("StateChanged").path(group.as_str()).build()?;
    let mut states = MessageStream::for_match_rule(rule, &conn, None).await?;

    let host = crate::sysinfo::hostname();
    let txt: Vec<Vec<u8>> = vec![format!("fp={fingerprint}").into_bytes(), format!("v={}", tura_proto::PROTOCOL_VERSION).into_bytes()];
    let add = |name: String| {
        let (conn, group, txt) = (conn.clone(), group.clone(), txt.clone());
        async move {
            // interface, protocol: any (-1); flags 0; domain/host: defaults.
            conn.call_method(Some(AVAHI), group.as_str(), Some(GROUP), "AddService", &(-1i32, -1i32, 0u32, name.as_str(), SERVICE, "", "", port, txt)).await?;
            conn.call_method(Some(AVAHI), group.as_str(), Some(GROUP), "Commit", &()).await?;
            anyhow::Ok(())
        }
    };
    add(host.clone()).await?;

    loop {
        tokio::select! {
            msg = states.next() => {
                let Some(Ok(msg)) = msg else { bail!("Avahi connection closed") };
                let Ok((state, _err)) = msg.body().deserialize::<(i32, String)>() else { continue };
                match state {
                    2 => {
                        ANNOUNCED.store(true, std::sync::atomic::Ordering::Relaxed);
                        info!(%host, port, "announced on the local network");
                    }
                    // Name taken (another PC with the same hostname): disambiguate.
                    3 => {
                        conn.call_method(Some(AVAHI), group.as_str(), Some(GROUP), "Reset", &()).await?;
                        add(format!("{host} {}", &fingerprint[..6.min(fingerprint.len())])).await?;
                    }
                    4 => bail!("Avahi couldn't register the service"),
                    _ => {}
                }
            }
            Some(ev) = owners.next() => {
                if ev.args().is_ok_and(|a| a.name().as_str() == AVAHI) {
                    bail!("Avahi restarted");
                }
            }
        }
    }
}

/// Looks up another Tunnelrat PC by the name turad announces it under
/// (its hostname, or "hostname fp6" after a name clash) and returns its IPv4
/// address if the announcement carries `fingerprint`.
pub async fn find(name: &str, fingerprint: &str) -> Option<String> {
    let conn = Connection::system().await.ok()?;
    let fp = fingerprint.to_lowercase();
    for candidate in [name.to_owned(), format!("{name} {}", &fp[..6.min(fp.len())])] {
        // interface any, IPv4 record and address, no flags.
        let reply = conn
            .call_method(Some(AVAHI), "/", Some("org.freedesktop.Avahi.Server"), "ResolveService", &(-1i32, 0i32, candidate.as_str(), SERVICE, "local", 0i32, 0u32))
            .await;
        let Ok(reply) = reply else { continue };
        type Resolved = (i32, i32, String, String, String, String, i32, String, u16, Vec<Vec<u8>>, u32);
        let Ok((.., address, _port, txt, _flags)) = reply.body().deserialize::<Resolved>() else { continue };
        let matches = txt.iter().any(|t| t.strip_prefix(b"fp=").is_some_and(|v| v.eq_ignore_ascii_case(fp.as_bytes())));
        if matches {
            return Some(address);
        }
    }
    None
}
