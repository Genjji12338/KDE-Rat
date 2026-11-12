//! Before the PC sleeps, take the phone drives down; mount them again after.
//!
//! A program touching a FUSE mount whose phone can't answer (Wi-Fi goes down
//! for sleep, and turad itself gets frozen) can't be frozen either, so the
//! suspend hangs. turad holds a logind "delay" sleep lock: on PrepareForSleep
//! every session unmounts its drive, then the lock is released.

use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use tokio::sync::broadcast;
use tracing::{info, warn};
use zbus::{Connection, MatchRule, MessageStream};

const LOGIN1: &str = "org.freedesktop.login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";

/// Runs forever. Sends `true` on `sleep` before sleeping, `false` after waking;
/// `mounted()` says whether any drive is still mounted.
pub async fn run(sleep: broadcast::Sender<bool>, mounted: impl Fn() -> bool) {
    loop {
        if let Err(e) = watch(&sleep, &mounted).await {
            warn!("sleep watcher: {e:#} (retrying)");
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

async fn inhibit(conn: &Connection) -> Result<zbus::OwnedFd> {
    let reply = conn
        .call_method(Some(LOGIN1), "/org/freedesktop/login1", Some(MANAGER), "Inhibit", &("sleep", "Tunnelrat", "Unmount phone drives before sleeping", "delay"))
        .await
        .context("Inhibit")?;
    Ok(reply.body().deserialize()?)
}

async fn watch(sleep: &broadcast::Sender<bool>, mounted: &impl Fn() -> bool) -> Result<()> {
    let conn = Connection::system().await?;
    let rule = MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(MANAGER).member("PrepareForSleep").build()?;
    let mut signals = MessageStream::for_match_rule(rule, &conn, None).await?;
    let mut lock = Some(inhibit(&conn).await?);
    while let Some(msg) = signals.next().await {
        let Ok((going,)) = msg?.body().deserialize::<(bool,)>() else { continue };
        if going {
            info!("going to sleep: unmounting phone drives");
            let _ = sleep.send(true);
            // Wait (briefly: logind's delay limit is a few seconds) for the unmounts.
            for _ in 0..40 {
                if !mounted() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            lock = None; // releases the delay lock: sleep may go ahead
        } else {
            info!("woke up");
            let _ = sleep.send(false);
            if lock.is_none() {
                lock = Some(inhibit(&conn).await?);
            }
        }
    }
    drop(lock);
    anyhow::bail!("logind signal stream ended")
}
