//! Power actions.
//!
//! Shutdown/reboot/suspend/lock go through systemd-logind on the system bus.
//! A user service is not part of the active seat session, so polkit would ask
//! for a password; `pc/packaging/50-tunnelrat-power.rules` allows these for
//! members of the `tunnelrat` group.
//!
//! Screen off goes through Mutter (GNOME) on the session bus.
//!
//! Suspend is disabled unless `--allow-suspend` is passed: on some machines
//! (e.g. Navi 22 / RX 6700 XT) the dGPU fails to resume from S3 and the PC
//! needs a hard power-off. Remote-suspending such a PC would strand it.

use anyhow::{Context, Result, bail};
use tura_proto::PowerAction;
use zbus::Connection;

const LOGIND: &str = "org.freedesktop.login1";
const LOGIND_PATH: &str = "/org/freedesktop/login1";
const LOGIND_IFACE: &str = "org.freedesktop.login1.Manager";

const MUTTER: &str = "org.gnome.Mutter.DisplayConfig";
const MUTTER_PATH: &str = "/org/gnome/Mutter/DisplayConfig";
/// DPMS "off" for Mutter's PowerSaveMode property.
const POWER_SAVE_OFF: i32 = 3;

#[derive(Debug, Clone, Copy)]
pub struct Policy {
    pub allow_suspend: bool,
}

impl Policy {
    pub fn allowed(&self) -> Vec<PowerAction> {
        let mut v = vec![PowerAction::Lock, PowerAction::ScreenOff, PowerAction::Reboot, PowerAction::Shutdown];
        if self.allow_suspend {
            v.push(PowerAction::Suspend);
        }
        v
    }
}

pub async fn run(action: PowerAction, policy: Policy) -> Result<()> {
    if !policy.allowed().contains(&action) {
        bail!("{action:?} is disabled on this PC (start turad with --allow-suspend to enable)");
    }

    if action == PowerAction::ScreenOff {
        return screen_off().await;
    }

    // Prefer GNOME's session manager: it runs inside the seat session, so
    // logind lets it power off without the polkit rule, and it ends the
    // session cleanly (apps get to save). Falls back to logind elsewhere.
    if matches!(action, PowerAction::Shutdown | PowerAction::Reboot) {
        let method = if action == PowerAction::Shutdown { "RequestShutdown" } else { "RequestReboot" };
        match gnome_session_request(method).await {
            Ok(()) => return Ok(()),
            Err(e) => tracing::info!("GNOME session manager unavailable ({e:#}), trying logind"),
        }
    }

    let conn = Connection::system().await.context("connecting to the system D-Bus")?;
    let method = match action {
        PowerAction::Shutdown => "PowerOff",
        PowerAction::Reboot => "Reboot",
        PowerAction::Suspend => "Suspend",
        PowerAction::Lock => return lock_own_session(&conn).await,
        PowerAction::ScreenOff => unreachable!(),
    };
    // `false` = non-interactive: fail instead of popping a polkit dialog.
    conn.call_method(Some(LOGIND), LOGIND_PATH, Some(LOGIND_IFACE), method, &(false,))
        .await
        .with_context(|| format!("logind {method}"))?;
    Ok(())
}

/// `RequestShutdown` / `RequestReboot`: no confirmation dialog unless an app
/// inhibits logout, in which case GNOME shows its usual dialog.
async fn gnome_session_request(method: &str) -> Result<()> {
    let conn = Connection::session().await.context("connecting to the session D-Bus")?;
    conn.call_method(Some("org.gnome.SessionManager"), "/org/gnome/SessionManager", Some("org.gnome.SessionManager"), method, &())
        .await
        .with_context(|| format!("org.gnome.SessionManager.{method}"))?;
    Ok(())
}

/// Locks only this user's graphical session. Unlike `LockSessions` (all
/// sessions, needs polkit), the session owner may lock their own session.
async fn lock_own_session(conn: &Connection) -> Result<()> {
    let user = zbus::Proxy::new(conn, LOGIND, "/org/freedesktop/login1/user/self", "org.freedesktop.login1.User").await?;
    let (_id, session): (String, zbus::OwnedObjectPath) =
        user.get_property("Display").await.context("finding your graphical session")?;
    if session.as_str() == "/" {
        bail!("no graphical session to lock");
    }
    conn.call_method(Some(LOGIND), session.as_str(), Some("org.freedesktop.login1.Session"), "Lock", &())
        .await
        .context("locking your session")?;
    Ok(())
}

/// Short, human message for the phone; the full error chain goes to the log.
pub fn friendly(e: &anyhow::Error) -> String {
    for cause in e.chain() {
        if let Some(zbus::Error::MethodError(name, _, _)) = cause.downcast_ref::<zbus::Error>() {
            return match name.as_str() {
                "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired" | "org.freedesktop.DBus.Error.AccessDenied" => {
                    format!("{}: the PC needs the Tunnelrat polkit rule (see README)", e)
                }
                "org.freedesktop.DBus.Error.ServiceUnknown" => format!("{}: not available in this desktop session", e),
                _ => e.to_string(),
            };
        }
    }
    e.to_string()
}

const IDLE: &str = "org.gnome.Mutter.IdleMonitor";
const IDLE_PATH: &str = "/org/gnome/Mutter/IdleMonitor/Core";
const POWER_SAVE_ON: i32 = 0;

/// Blanks the displays and turns them back on at the next user activity.
///
/// Mutter does not undo an externally set PowerSaveMode on input (gsd-power
/// only undoes blanking it started itself), so without the watch the screens
/// stay black forever. The watch is armed *before* blanking; if arming fails we
/// never blank.
async fn screen_off() -> Result<()> {
    let conn = Connection::session().await.context("connecting to the session D-Bus")?;

    let idle = zbus::Proxy::new(&conn, IDLE, IDLE_PATH, IDLE).await.context("Mutter IdleMonitor")?;
    let mut fired = idle.receive_signal("WatchFired").await.context("subscribing to WatchFired")?;
    let (watch_id,): (u32,) = idle.call("AddUserActiveWatch", &()).await.context("AddUserActiveWatch")?;

    set_power_save(&conn, POWER_SAVE_OFF).await?;

    tokio::spawn(async move {
        use futures_util::StreamExt;
        while let Some(msg) = fired.next().await {
            if msg.body().deserialize::<(u32,)>().is_ok_and(|(id,)| id == watch_id) {
                break;
            }
        }
        // User-active watches are one-shot in Mutter, so no RemoveWatch needed.
        if let Err(e) = set_power_save(&conn, POWER_SAVE_ON).await {
            tracing::warn!("could not turn screens back on: {e:#}");
        } else {
            tracing::info!("screens back on after user activity");
        }
    });
    Ok(())
}

async fn set_power_save(conn: &Connection, mode: i32) -> Result<()> {
    conn.call_method(
        Some(MUTTER),
        MUTTER_PATH,
        Some("org.freedesktop.DBus.Properties"),
        "Set",
        &(MUTTER, "PowerSaveMode", zbus::Variant::from(mode)),
    )
    .await
    .context("Mutter PowerSaveMode (is this a GNOME session?)")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suspend_off_by_default() {
        let p = Policy { allow_suspend: false };
        assert!(!p.allowed().contains(&PowerAction::Suspend));
        assert!(p.allowed().contains(&PowerAction::ScreenOff));
    }

    #[tokio::test]
    async fn suspend_refused_before_touching_dbus() {
        let err = run(PowerAction::Suspend, Policy { allow_suspend: false }).await.unwrap_err();
        assert!(err.to_string().contains("disabled"), "{err}");
    }

    #[test]
    fn opt_in_enables_suspend() {
        assert!(Policy { allow_suspend: true }.allowed().contains(&PowerAction::Suspend));
    }
}
