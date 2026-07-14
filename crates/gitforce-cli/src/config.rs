//! CLI configuration management

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// CLI configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Server URL
    pub server_url: String,

    /// Auth token
    pub token: Option<String>,

    /// Local data directory
    pub local_data_dir: PathBuf,

    /// Default organization
    pub organization: Option<String>,

    /// Editor for multi-line input
    pub editor: Option<String>,

    /// Output format (json, yaml, table)
    pub output_format: OutputFormat,

    /// Feature flags
    pub features: HashMap<String, bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum OutputFormat {
    #[default]
    Table,
    Json,
    Yaml,
}

impl Config {
    /// Load configuration from default locations
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;

        if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path)?;
            let config: Config = toml::from_str(&contents)?;
            Ok(config)
        } else {
            // Return default config
            Ok(Self::default())
        }
    }

    /// Save configuration
    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path()?;
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self)?;
        std::fs::write(config_path, contents)?;
        Ok(())
    }

    /// Get config file path
    fn config_path() -> Result<PathBuf> {
        let base = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("could not find config directory"))?;
        Ok(base.join("gitforge").join("config.toml"))
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_url: "http://localhost:8080".to_string(),
            token: None,
            local_data_dir: dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("gitforge"),
            organization: None,
            editor: None,
            output_format: OutputFormat::Table,
            features: HashMap::new(),
        }
    }
}

impl Config {
    /// Get the API base URL
    pub fn api_url(&self) -> String {
        format!("{}/api", self.server_url)
    }
}
