//! Local settings persistence. Everything stays on disk in the user's config
//! dir — no network, no accounts (this app is local-only by design).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ui::Targets;

/// Everything we remember between runs.
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    pub gain: f32,
    pub threshold: f32,
    /// Preferred input device name; `None` = system default.
    pub device: Option<String>,
    pub targets: Targets,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            gain: 1.0,
            threshold: 0.01,
            device: None,
            targets: Targets::default(),
        }
    }
}

impl Settings {
    /// `%APPDATA%/audirs/config.json` (falls back to the current dir).
    fn path() -> PathBuf {
        let base = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        base.join("audirs").join("config.json")
    }

    /// Load settings, falling back to defaults on any error (missing/corrupt).
    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Best-effort save; logs but does not fail the app on error.
    pub fn save(&self) {
        let path = Self::path();
        let res = (|| -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let json = serde_json::to_string_pretty(self)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
            std::fs::write(&path, json)
        })();
        if let Err(e) = res {
            eprintln!("failed to save settings to {}: {e}", path.display());
        }
    }
}
