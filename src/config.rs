//! Where the application keeps its files, and what it remembers between runs.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The name every directory below is made under.
const APP: &str = "canora";

/// Errors from reading or writing the settings file.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The platform gives no home directory.
    #[error("cannot find a home directory")]
    NoHome,
    /// The file cannot be read or written.
    #[error("settings file: {0}")]
    #[allow(dead_code, reason = "read and write both produce it; only one is reached so far")]
    Io(#[from] std::io::Error),
    /// The file holds something other than settings.
    #[error("settings file is malformed: {0}")]
    Decode(#[from] serde_json::Error),
}

/// The directory for caches: credentials, audio, and album art.
///
/// Create the directory if it is absent.
pub fn cache_dir() -> Result<PathBuf, ConfigError> {
    let dir = dirs::cache_dir().ok_or(ConfigError::NoHome)?.join(APP);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// The directory for settings.
///
/// Create the directory if it is absent.
pub fn config_dir() -> Result<PathBuf, ConfigError> {
    let dir = dirs::config_dir().ok_or(ConfigError::NoHome)?.join(APP);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// What the application remembers between runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Which colour scheme the interface presents in.
    pub scheme: String,
    /// Which theme the interface uses.
    pub variant: String,
}

impl Settings {
    /// The settings from disk. Return the defaults if the file is absent.
    pub fn load() -> Result<Self, ConfigError> {
        let path = config_dir()?.join("settings.json");
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(serde_json::from_str(&text)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    /// Write the settings to disk.
    pub fn save(&self) -> Result<(), ConfigError> {
        let path = config_dir()?.join("settings.json");
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}
