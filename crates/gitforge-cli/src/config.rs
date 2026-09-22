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
    ///
    /// The file may hold the bearer token persisted by `auth --login`, so
    /// the mode is forced to owner-only. Setting permissions after the write
    /// also tightens a pre-existing file that an older CLI created 0644.
    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::config_path()?)
    }

    /// Write the configuration to an explicit path, forcing owner-only mode.
    pub fn save_to(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self)?;
        std::fs::write(path, contents)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
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
            server_url: "http://localhost:42780".to_string(),
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
        assert_eq!(config.server_url, "http://localhost:42780");
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
        let config = Config {
            token: Some("test-token".to_string()),
            ..Default::default()
        };
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
        assert!(serialized.contains("http://localhost:42780"));
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

    /// The saved file may carry the bearer token, so the mode must be
    /// owner-only — both on first write and when tightening a file an older
    /// CLI had left world-readable.
    #[cfg(unix)]
    #[test]
    fn test_save_forces_owner_only_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("gitforge-config-test-{}", std::process::id()));
        let path = dir.join("nested").join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);

        let config = Config {
            token: Some("secret".to_string()),
            ..Default::default()
        };
        config.save_to(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "fresh save must be owner-only");

        // A pre-existing wide-open file is tightened on the next save.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        config.save_to(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "re-save must tighten wide modes");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_config_local_data_dir_default() {
        let config = Config::default();
        assert!(config.local_data_dir.to_string_lossy().contains("gitforge"));
    }

    #[test]
    fn test_config_organization() {
        let config = Config {
            organization: Some("my-org".to_string()),
            ..Default::default()
        };
        assert_eq!(config.organization.unwrap(), "my-org");
    }

    #[test]
    fn test_config_editor() {
        let config = Config {
            editor: Some("vim".to_string()),
            ..Default::default()
        };
        assert_eq!(config.editor.unwrap(), "vim");
    }
}
