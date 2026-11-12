//! turad's settings (`~/.config/tunnelrat/settings.json`), changed from the app.

use anyhow::Result;
use tura_proto::TuradSettings;

fn path() -> std::path::PathBuf {
    crate::identity::config_dir().join("settings.json")
}

pub fn load() -> TuradSettings {
    std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save(s: &TuradSettings) -> Result<()> {
    std::fs::write(path(), serde_json::to_vec_pretty(s)?)?;
    Ok(())
}

/// The first executable called `name` on $PATH.
pub fn which(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH")?.to_str()?.split(':').map(|d| std::path::Path::new(d).join(name)).find(|p| p.is_file())
}
