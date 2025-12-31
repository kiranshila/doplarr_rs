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
    pub movie_backend: Option<MovieBackend>,
    pub series_backend: Option<SeriesBackend>,
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq, Clone)]
pub enum MovieBackend {
    Radarr {
        url: String,
        api_key: String,
        monitor_type: Option<RadarrMonitor>,
        quality_profile: Option<String>,
        rootfolder: Option<String>,
        minimum_availability: Option<MovieStatusType>,
    },
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq, Clone)]
pub enum SeriesBackend {
    Sonarr {
        url: String,
        api_key: String,
        monitor_type: Option<SonarrMonitor>,
        quality_profile: Option<String>,
        rootfolder: Option<String>,
        series_type: Option<SeriesTypes>,
        season_folders: Option<bool>,
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

           [movie_backend.Radarr]
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
            movie_backend: Some(MovieBackend::Radarr {
                url: "http://1.2.3.4:7878".to_string(),
                api_key: "abc123".to_string(),
                monitor_type: Some(RadarrMonitor::MovieOnly),
                rootfolder: Some("/storage/movies".to_string()),
                minimum_availability: Some(MovieStatusType::Announced),
                quality_profile: None,
            }),
            series_backend: None,
            log_level: None,
            public_followup: None,
        };

        assert_eq!(config, expected);
    }
}
