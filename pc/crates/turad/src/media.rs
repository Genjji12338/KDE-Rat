//! Media: the active MPRIS player plus system volume (PipeWire via `pactl`).
//!
//! A poller builds a [`NowPlaying`] snapshot about once a second while a phone
//! is online and publishes it on a `watch` channel; sessions forward changes.
//! Polling (rather than PropertiesChanged signals) keeps this robust against
//! players appearing/disappearing and buggy signal emitters.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine;
use tokio::process::Command;
use tokio::sync::{Notify, watch};
use tura_proto::{MediaAction, NowPlaying};
use zbus::{Connection, OwnedVariant};

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
/// Local cover files larger than this are not sent to the phone.
const MAX_ART_BYTES: u64 = 512 * 1024;

pub struct Media {
    tx: watch::Sender<NowPlaying>,
    poke: Notify,
}

impl Media {
    pub fn new() -> Self {
        Self { tx: watch::channel(NowPlaying::default()).0, poke: Notify::new() }
    }

    pub fn subscribe(&self) -> watch::Receiver<NowPlaying> {
        self.tx.subscribe()
    }

    /// Runs forever. `phones_online` gates the work so an idle PC isn't polled.
    pub async fn poll_loop(&self, phones_online: impl Fn() -> bool) {
        let mut art_cache: (String, Option<String>) = (String::new(), None);
        let mut conn: Option<Connection> = None;
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(1000)) => {}
                _ = self.poke.notified() => tokio::time::sleep(Duration::from_millis(150)).await,
            }
            if !phones_online() {
                continue;
            }
            if conn.is_none() {
                conn = Connection::session().await.ok();
            }
            let Some(c) = &conn else { continue };
            match snapshot(c, &mut art_cache).await {
                Ok(np) => {
                    self.tx.send_if_modified(|cur| {
                        let changed = *cur != np;
                        if changed {
                            *cur = np;
                        }
                        changed
                    });
                }
                Err(e) => {
                    tracing::debug!("media snapshot failed: {e:#}");
                    conn = None; // reconnect next round
                }
            }
        }
    }

    pub async fn command(&self, action: MediaAction) -> Result<()> {
        let conn = Connection::session().await?;
        let Some(player) = pick_player(&conn).await? else { bail!("nothing is playing on the PC") };
        let method = match action {
            MediaAction::PlayPause => "PlayPause",
            MediaAction::Next => "Next",
            MediaAction::Previous => "Previous",
        };
        conn.call_method(Some(player.as_str()), MPRIS_PATH, Some(PLAYER_IFACE), method, &())
            .await
            .with_context(|| format!("{method} on {player}"))?;
        self.poke.notify_one();
        Ok(())
    }

    /// Changes volume by `step` percent of the loudest channel while scaling
    /// every channel by the same factor, so the left/right balance set in
    /// GNOME is preserved (wpctl set-volume would flatten it).
    pub async fn volume_step(&self, step: i32) -> Result<()> {
        let raw = channel_volumes().await?;
        let max = raw.iter().copied().max().unwrap_or(0);
        // Never push the loudest channel past the UI limit, but never pull a
        // level the user set above it down either.
        let limit = (ui_max_volume().await).max(max);
        let target = (max as i64 + i64::from(step.clamp(-50, 50)) * NORM as i64 / 100).clamp(0, limit as i64) as u32;
        let new: Vec<u32> = if max == 0 {
            raw.iter().map(|_| target).collect()
        } else {
            raw.iter().map(|&v| ((u64::from(v) * u64::from(target) + u64::from(max) / 2) / u64::from(max)) as u32).collect()
        };
        let args: Vec<String> = ["set-sink-volume", "@DEFAULT_SINK@"].into_iter().map(String::from).chain(new.iter().map(u32::to_string)).collect();
        pactl(&args.iter().map(String::as_str).collect::<Vec<_>>()).await?;
        self.poke.notify_one();
        Ok(())
    }

    pub async fn mute_toggle(&self) -> Result<()> {
        pactl(&["set-sink-mute", "@DEFAULT_SINK@", "toggle"]).await?;
        self.poke.notify_one();
        Ok(())
    }
}

/// PulseAudio "100%" in raw volume units.
const NORM: u32 = 65536;

/// pactl via PipeWire's Pulse server; LC_ALL=C because output is localised.
async fn pactl(args: &[&str]) -> Result<String> {
    let out = Command::new("pactl").args(args).env("LC_ALL", "C").output().await.context("running pactl (is pipewire-pulse installed?)")?;
    if !out.status.success() {
        bail!("pactl {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn channel_volumes() -> Result<Vec<u32>> {
    let out = pactl(&["get-sink-volume", "@DEFAULT_SINK@"]).await?;
    let v = parse_channels(&out);
    if v.is_empty() {
        bail!("couldn't read the sink volume");
    }
    Ok(v)
}

/// 150% when GNOME's "Over-Amplification" is on, else 100%.
async fn ui_max_volume() -> u32 {
    let over = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.sound", "allow-volume-above-100-percent"])
        .output()
        .await
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "true");
    if over { NORM * 3 / 2 } else { NORM }
}

/// "Volume: front-left: 58391 /  89% / -3.01 dB,   front-right: 73684 / ..." → [58391, 73684]
fn parse_channels(s: &str) -> Vec<u32> {
    let first = s.lines().next().unwrap_or("");
    first
        .split(',')
        // "Volume: front-left: 58391 / 89% ..." → the raw value sits after the last ':' before the first '/'.
        .filter_map(|ch| ch.split('/').next()?.rsplit(':').next()?.trim().parse().ok())
        .collect()
}

/// Loudest channel in percent, as GNOME's slider shows it with balance.
fn percent(raw: &[u32]) -> u32 {
    let max = raw.iter().copied().max().unwrap_or(0);
    ((u64::from(max) * 100 + u64::from(NORM) / 2) / u64::from(NORM)) as u32
}

async fn players(conn: &Connection) -> Result<Vec<String>> {
    let dbus = zbus::fdo::DBusProxy::new(conn).await?;
    Ok(dbus.list_names().await?.into_iter().map(|n| n.to_string()).filter(|n| n.starts_with(MPRIS_PREFIX)).collect())
}

async fn prop<T: TryFrom<OwnedVariant>>(conn: &Connection, dest: &str, iface: &str, name: &str) -> Option<T> {
    let proxy = zbus::Proxy::new(conn, dest, MPRIS_PATH, iface).await.ok()?;
    let v: OwnedVariant = proxy.get_property(name).await.ok()?;
    T::try_from(v).ok()
}

/// Playing beats Paused beats anything else; ties go to the first listed.
async fn pick_player(conn: &Connection) -> Result<Option<String>> {
    let mut best: Option<(u8, String)> = None;
    for p in players(conn).await? {
        let status: Option<String> = prop(conn, &p, PLAYER_IFACE, "PlaybackStatus").await;
        let rank = match status.as_deref() {
            Some("Playing") => 2,
            Some("Paused") => 1,
            _ => 0,
        };
        if best.as_ref().is_none_or(|(r, _)| rank > *r) {
            best = Some((rank, p));
        }
    }
    Ok(best.map(|(_, p)| p))
}

async fn snapshot(conn: &Connection, art_cache: &mut (String, Option<String>)) -> Result<NowPlaying> {
    let mut np = NowPlaying::default();
    if let Ok(raw) = channel_volumes().await {
        np.volume = percent(&raw);
    }
    if let Ok(out) = pactl(&["get-sink-mute", "@DEFAULT_SINK@"]).await {
        np.muted = out.contains("yes");
    }

    let Some(player) = pick_player(conn).await? else { return Ok(np) };
    np.player = Some(
        prop::<String>(conn, &player, "org.mpris.MediaPlayer2", "Identity")
            .await
            .unwrap_or_else(|| player.trim_start_matches(MPRIS_PREFIX).to_owned()),
    );
    np.playing = prop::<String>(conn, &player, PLAYER_IFACE, "PlaybackStatus").await.as_deref() == Some("Playing");
    np.can_next = prop::<bool>(conn, &player, PLAYER_IFACE, "CanGoNext").await.unwrap_or(true);
    np.can_previous = prop::<bool>(conn, &player, PLAYER_IFACE, "CanGoPrevious").await.unwrap_or(true);

    let meta: HashMap<String, OwnedVariant> = prop(conn, &player, PLAYER_IFACE, "Metadata").await.unwrap_or_default();
    let s = |k: &str| meta.get(k).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
    let list = |k: &str| meta.get(k).and_then(|v| Vec::<String>::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
    np.title = s("xesam:title");
    np.album = s("xesam:album");
    np.artist = list("xesam:artist").join(", ");

    let art_url = s("mpris:artUrl");
    if art_url != art_cache.0 {
        *art_cache = (art_url.clone(), load_art(&art_url).await);
    }
    np.art = art_cache.1.clone();
    Ok(np)
}

/// https URLs are passed through; local files are inlined as a data URI.
async fn load_art(url: &str) -> Option<String> {
    if url.starts_with("https://") {
        return Some(url.to_owned());
    }
    let path = url.strip_prefix("file://")?;
    let path = percent_decode(path);
    let meta = tokio::fs::metadata(&path).await.ok()?;
    if meta.len() > MAX_ART_BYTES {
        return None;
    }
    let bytes = tokio::fs::read(&path).await.ok()?;
    let mime = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) { "image/png" } else { "image/jpeg" };
    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pactl_channels() {
        let out = "Volume: front-left: 58391 /  89% / -3.01 dB,   front-right: 73684 / 112% / 3.05 dB\n        balance 0.21\n";
        assert_eq!(parse_channels(out), vec![58391, 73684]);
        assert_eq!(percent(&[58391, 73684]), 112);
        assert!(parse_channels("garbage").is_empty());
    }

    #[test]
    fn decodes_file_urls() {
        assert_eq!(percent_decode("/tmp/My%20Cover.jpg"), "/tmp/My Cover.jpg");
        assert_eq!(percent_decode("/a%"), "/a%");
    }
}
