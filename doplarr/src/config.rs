use anyhow::Context;
use radarr_api::models::{MonitorTypes as RadarrMonitor, MovieStatusType};
use serde::{Deserialize, Serialize};
use sonarr_api::models::{MonitorTypes as SonarrMonitor, SeriesTypes};
use std::fs;

#[derive(Deserialize, Serialize, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub log_level: Option<String>,
    pub public_followup: Option<bool>,
    pub discord_token: String,
    pub backends: Vec<Backend>,
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq, Clone)]
pub struct Backend {
    pub media: String,
    pub config: BackendConfig,
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq, Clone)]
/// All of the backend-specific configuration, passed to the backend constructors
pub enum BackendConfig {
    Radarr {
        url: String,
        api_key: String,
        monitor_type: Option<RadarrMonitor>,
        quality_profile: Option<String>,
        rootfolder: Option<String>,
        minimum_availability: Option<MovieStatusType>,
    },
    Sonarr {
        url: String,
        api_key: String,
        monitor_type: Option<SonarrMonitor>,
        quality_profile: Option<String>,
        rootfolder: Option<String>,
        series_type: Option<SeriesTypes>,
        season_folders: Option<bool>,
        /// Restrict which monitor types users can select (e.g., to prevent "All")
        allowed_monitor_types: Option<Vec<SonarrMonitor>>,
    },
}

impl Config {
    pub fn from_file(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();

        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        let config: Config = toml::from_str(&content)
            .with_context(|| format!("Failed to parse TOML in: {}", path.display()))?;

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config() {
        let config: Config = toml::from_str(
            r#"
           discord_token = "abc123"

           [[backends]]
           media = "movie"

           [backends.config.Radarr]
           url = "http://1.2.3.4:7878"
           api_key = "abc123"
           monitor_type = "movieOnly"
           rootfolder = "/storage/movies"
           minimum_availability= "announced"
        "#,
        )
        .unwrap();

        let expected = Config {
            discord_token: "abc123".to_string(),
            backends: vec![Backend {
                media: "movie".to_string(),
                config: BackendConfig::Radarr {
                    url: "http://1.2.3.4:7878".to_string(),
                    api_key: "abc123".to_string(),
                    monitor_type: Some(RadarrMonitor::MovieOnly),
                    rootfolder: Some("/storage/movies".to_string()),
                    minimum_availability: Some(MovieStatusType::Announced),
                    quality_profile: None,
                },
            }],
            log_level: None,
            public_followup: None,
        };

        assert_eq!(config, expected);
    }
}
