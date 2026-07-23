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

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
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
        let base =
            dirs::config_dir().ok_or_else(|| anyhow::anyhow!("could not find config directory"))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert_eq!(config.server_url, "http://localhost:8080");
        assert!(config.token.is_none());
        assert_eq!(config.output_format, OutputFormat::Table);
    }

    #[test]
    fn test_config_api_url() {
        let config = Config {
            server_url: "https://gitforge.example.com".to_string(),
            ..Default::default()
        };
        assert_eq!(config.api_url(), "https://gitforge.example.com/api");
    }

    #[test]
    fn test_output_format_default() {
        assert_eq!(OutputFormat::default(), OutputFormat::Table);
    }

    #[test]
    fn test_config_with_token() {
        let mut config = Config::default();
        config.token = Some("test-token".to_string());
        assert_eq!(config.token.as_ref().unwrap(), "test-token");
    }

    #[test]
    fn test_config_with_features() {
        let mut config = Config::default();
        config.features.insert("sync".to_string(), true);
        assert_eq!(config.features.get("sync"), Some(&true));
    }

    #[test]
    fn test_output_format_variants() {
        assert!(matches!(OutputFormat::Table, OutputFormat::Table));
        assert!(matches!(OutputFormat::Json, OutputFormat::Json));
        assert!(matches!(OutputFormat::Yaml, OutputFormat::Yaml));
    }

    #[test]
    fn test_output_format_serialize() {
        let fmt = OutputFormat::Json;
        let serialized = serde_json::to_string(&fmt).unwrap();
        assert_eq!(serialized, "\"Json\"");
    }

    #[test]
    fn test_output_format_deserialize() {
        let fmt: OutputFormat = serde_json::from_str("\"Yaml\"").unwrap();
        assert!(matches!(fmt, OutputFormat::Yaml));
    }

    #[test]
    fn test_config_serialize() {
        let config = Config::default();
        let serialized = serde_json::to_string(&config).unwrap();
        assert!(serialized.contains("http://localhost:8080"));
    }

    #[test]
    fn test_config_deserialize() {
        let json = r#"{
            "server_url": "https://custom.example.com",
            "token": null,
            "local_data_dir": "/tmp/gitforge",
            "organization": null,
            "editor": null,
            "output_format": "Json",
            "features": {}
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.server_url, "https://custom.example.com");
        assert!(matches!(config.output_format, OutputFormat::Json));
    }

    #[test]
    fn test_config_clone() {
        let config = Config::default();
        let cloned = config.clone();
        assert_eq!(config.server_url, cloned.server_url);
    }

    #[test]
    fn test_config_local_data_dir_default() {
        let config = Config::default();
        assert!(config.local_data_dir.to_string_lossy().contains("gitforge"));
    }

    #[test]
    fn test_config_organization() {
        let mut config = Config::default();
        config.organization = Some("my-org".to_string());
        assert_eq!(config.organization.unwrap(), "my-org");
    }

    #[test]
    fn test_config_editor() {
        let mut config = Config::default();
        config.editor = Some("vim".to_string());
        assert_eq!(config.editor.unwrap(), "vim");
    }
}
