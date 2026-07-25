use crate::common::{DimiError, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootConfig {
    pub data_dir: PathBuf,
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("DIMI_DATA_DIR") {
        return PathBuf::from(dir);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".dimi")
}

impl Default for BootConfig {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            log_level: default_log_level(),
        }
    }
}

impl BootConfig {
    pub fn load_or_default() -> Result<Self> {
        let default_dir = default_data_dir();
        let candidate = default_dir.join("config.toml");

        let config = if candidate.exists() {
            let text = std::fs::read_to_string(&candidate)?;
            toml::from_str(&text)
                .map_err(|e| DimiError::Internal(format!("invalid config.toml: {e}")))?
        } else {
            Self::default()
        };

        std::fs::create_dir_all(&config.data_dir)?;
        std::fs::create_dir_all(config.cache_dir())?;
        Ok(config)
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("dimi.db")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.data_dir.join("cache")
    }

    pub fn plugins_dir(&self) -> PathBuf {
        self.data_dir.join("plugins")
    }

    pub fn attached_files_dir(&self) -> PathBuf {
        self.data_dir.join("attached-files")
    }
}
