use super::*;
use crate::config::SeriesBackend;
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use sonarr_api::{
    apis::{
        configuration::{ApiKey, Configuration},
        quality_profile_api::api_v3_qualityprofile_get,
        root_folder_api::api_v3_rootfolder_get,
        series_api::api_v3_series_post,
        series_lookup_api::api_v3_series_lookup_get,
    },
    models::{
        AddSeriesOptions, MonitorTypes, QualityProfileResource, RootFolderResource, SeriesResource,
        SeriesTypes,
    },
};

#[derive(Debug, Clone)]
pub struct Sonarr {
    config: Configuration,
    details: Details,
}

#[derive(Debug, Clone)]
// All the details we want to collect
pub struct Details {
    rootfolders: Vec<RootFolderResource>,
    quality_profiles: Vec<QualityProfileResource>,
    monitor: Vec<MonitorTypes>,
    series_type: Vec<SeriesTypes>,
    season_folder: Option<bool>,
}

#[derive(Debug)]
// The final details needed to complete the request
pub struct SelectedDetails {
    pub rootfolder_path: String,
    pub quality_profile_id: i32,
    pub monitor: MonitorTypes,
    pub series_type: SeriesTypes,
    pub season_folder: bool,
}

impl Sonarr {
    #[allow(clippy::too_many_arguments)]
    /// Builds the Sonarr connection and attempts to use it
    pub async fn new(
        base_path: String,
        key: String,
        monitor_type: Option<MonitorTypes>,
        quality_profile: Option<String>,
        rootfolder: Option<String>,
        series_type: Option<SeriesTypes>,
        season_folder: Option<bool>,
        client: reqwest::Client,
    ) -> Result<Self> {
        // Build the API config
        let config = Configuration {
            base_path,
            user_agent: None,
            client,
            basic_auth: None,
            oauth_access_token: None,
            bearer_access_token: None,
            api_key: Some(ApiKey { prefix: None, key }),
        };

        // Grab the additional details and use the config data to filter

        // First query the things we have to check (this will fail if we can't connect to the server anyway)
        let mut rootfolders = api_v3_rootfolder_get(&config).await?;
        let mut quality_profiles = api_v3_qualityprofile_get(&config).await?;

        // Select rootfolder if given
        if let Some(rf) = rootfolder {
            // Get the index of the selection
            let rf_idx = rootfolders
                .iter()
                .position(|x| matches!(&x.path, Some(Some(path)) if path == &rf))
                .with_context(|| {
                    let available = rootfolders
                        .iter()
                        .filter_map(|x| x.path.as_ref().and_then(|inner| inner.as_deref()))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(
                        "Root folder '{}' not found. Available options: [{}]",
                        rf, available
                    )
                })?;
            let selected = rootfolders.swap_remove(rf_idx);
            rootfolders = vec![selected];
        }

        // Select quality profile if given
        if let Some(qp) = quality_profile {
            // Get the index of the selection
            let qp_idx = quality_profiles
                .iter()
                .position(|x| matches!(&x.name, Some(Some(name)) if name == &qp))
                .with_context(|| {
                    let available = quality_profiles
                        .iter()
                        .filter_map(|x| x.name.as_ref().and_then(|inner| inner.as_deref()))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(
                        "Quality profile '{}' not found. Available options: [{}]",
                        qp, available
                    )
                })?;
            let selected = quality_profiles.swap_remove(qp_idx);
            quality_profiles = vec![selected];
        }

        let series_type = if let Some(x) = series_type {
            vec![x]
        } else {
            vec![
                SeriesTypes::Standard,
                SeriesTypes::Daily,
                SeriesTypes::Anime,
            ]
        };

        let monitor = if let Some(x) = monitor_type {
            vec![x]
        } else {
            vec![
                MonitorTypes::All,
                MonitorTypes::Future,
                MonitorTypes::Missing,
                MonitorTypes::Existing,
                MonitorTypes::FirstSeason,
                MonitorTypes::LastSeason,
                MonitorTypes::LatestSeason,
                MonitorTypes::Pilot,
                MonitorTypes::Recent,
                MonitorTypes::MonitorSpecials,
                MonitorTypes::UnmonitorSpecials,
                MonitorTypes::None,
            ]
        };

        // Build the details
        let details = Details {
            rootfolders,
            quality_profiles,
            monitor,
            series_type,
            season_folder,
        };

        Ok(Self { config, details })
    }

    #[allow(irrefutable_let_patterns)]
    pub async fn connect(backend: SeriesBackend, client: reqwest::Client) -> Result<Self> {
        if let SeriesBackend::Sonarr {
            url,
            api_key,
            monitor_type,
            quality_profile,
            rootfolder,
            series_type,
            season_folders,
        } = backend
        {
            Self::new(
                url,
                api_key,
                monitor_type,
                quality_profile,
                rootfolder,
                series_type,
                season_folders,
                client,
            )
            .await
        } else {
            bail!("Configured backend not for Sonarr");
        }
    }
}

/// Helper function to get to and from stringified references
fn deserialize_from_string<T: serde::de::DeserializeOwned>(s: &str) -> Result<T> {
    serde_json::from_str(&format!("\"{}\"", s))
        .with_context(|| format!("Failed to deserialize enum variant: {}", s))
}

mod field_keys {
    pub const ROOT_FOLDER: &str = "sonarr:root_folder";
    pub const MONITOR: &str = "sonarr:monitor";
    pub const SERIES_TYPE: &str = "sonarr:series_type";
    pub const QUALITY_PROFILE: &str = "sonarr:quality_profile";
    pub const SEASON_FOLDER: &str = "sonarr:season_folder";
}

impl From<Details> for Vec<RequestDetails> {
    fn from(details: Details) -> Vec<RequestDetails> {
        let quality_profile_options = details
            .quality_profiles
            .iter()
            .map(|x| DropdownOption {
                title: x
                    .name
                    .clone()
                    .flatten()
                    .expect("Every quality profile should have a name"),
                description: None,
                id: x.id.map(SelectableId::Integer),
            })
            .collect();

        let quality_profile_details = RequestDetails {
            title: "Quality Profile".to_string(),
            options: quality_profile_options,
            metadata: Some(field_keys::QUALITY_PROFILE.to_string()),
            field_type: FieldType::Dropdown,
        };

        let rootfolder_options = details
            .rootfolders
            .iter()
            .map(|x| DropdownOption {
                title: x
                    .path
                    .clone()
                    .flatten()
                    .expect("Every root folder needs a path"),
                description: None,
                id: x.id.map(SelectableId::Integer),
            })
            .collect();

        let rootfolder_details = RequestDetails {
            title: "Root Folder".to_string(),
            options: rootfolder_options,
            metadata: Some(field_keys::ROOT_FOLDER.to_string()),
            field_type: FieldType::Dropdown,
        };

        let monitor_options = details
            .monitor
            .iter()
            .map(|x| {
                let title = match x {
                    MonitorTypes::Unknown => "Unknown",
                    MonitorTypes::All => "All",
                    MonitorTypes::Future => "Future",
                    MonitorTypes::Missing => "Missing",
                    MonitorTypes::Existing => "Existing",
                    MonitorTypes::FirstSeason => "First Season",
                    MonitorTypes::LastSeason => "Last Season",
                    MonitorTypes::LatestSeason => "Latest Season",
                    MonitorTypes::Pilot => "Pilot",
                    MonitorTypes::Recent => "Recent",
                    MonitorTypes::MonitorSpecials => "Monitor Specials",
                    MonitorTypes::UnmonitorSpecials => "Unmonitor Specials",
                    MonitorTypes::None => "None",
                    MonitorTypes::Skip => "Skip",
                };

                DropdownOption {
                    title: title.to_string(),
                    description: None,
                    id: Some(SelectableId::String(x.to_string())),
                }
            })
            .collect();

        let monitor_details = RequestDetails {
            title: "Monitor".to_string(),
            options: monitor_options,
            metadata: Some(field_keys::MONITOR.to_string()),
            field_type: FieldType::Dropdown,
        };

        let series_type_options = details
            .series_type
            .iter()
            .map(|x| {
                let title = match x {
                    SeriesTypes::Standard => "Standard",
                    SeriesTypes::Daily => "Daily",
                    SeriesTypes::Anime => "Anime",
                };
                DropdownOption {
                    title: title.to_string(),
                    description: None,
                    id: Some(SelectableId::String(x.to_string())),
                }
            })
            .collect();

        let series_type_details = RequestDetails {
            title: "Series Type".to_string(),
            options: series_type_options,
            metadata: Some(field_keys::SERIES_TYPE.to_string()),
            field_type: FieldType::Dropdown,
        };

        // Season folder boolean option - show both if None, or just the config value if Some
        let season_folder_options = match details.season_folder {
            Some(value) => {
                // Config default - show only that value
                vec![DropdownOption {
                    title: if value { "Yes" } else { "No" }.to_string(),
                    description: None,
                    id: Some(SelectableId::Boolean(value)),
                }]
            }
            None => {
                // No config default - show both for user selection
                vec![
                    DropdownOption {
                        title: "Yes".to_string(),
                        description: None,
                        id: Some(SelectableId::Boolean(true)),
                    },
                    DropdownOption {
                        title: "No".to_string(),
                        description: None,
                        id: Some(SelectableId::Boolean(false)),
                    },
                ]
            }
        };

        let season_folder_details = RequestDetails {
            title: "Use Season Folders".to_string(),
            options: season_folder_options,
            metadata: Some(field_keys::SEASON_FOLDER.to_string()),
            field_type: FieldType::Boolean,
        };

        vec![
            rootfolder_details,
            monitor_details,
            series_type_details,
            quality_profile_details,
            season_folder_details,
        ]
    }
}

impl TryFrom<Vec<RequestDetails>> for SelectedDetails {
    type Error = anyhow::Error;

    fn try_from(details: Vec<RequestDetails>) -> Result<Self> {
        let mut root_folder_path = None;
        let mut quality_profile_id = None;
        let mut monitor = None;
        let mut series_type = None;
        let mut season_folder = None;

        for detail in details {
            let selection = detail
                .options
                .into_iter()
                .next()
                .expect("RequestDetails must have at least one option");

            match detail.metadata.as_deref() {
                Some(field_keys::ROOT_FOLDER) => {
                    root_folder_path = Some(selection.title);
                }
                Some(field_keys::QUALITY_PROFILE) => {
                    quality_profile_id = match selection.id {
                        Some(SelectableId::Integer(i)) => Some(i),
                        _ => panic!("Quality profile must have integer ID"),
                    };
                }
                Some(field_keys::MONITOR) => {
                    monitor = match selection.id {
                        Some(SelectableId::String(s)) => Some(deserialize_from_string(&s)?),
                        _ => panic!("Monitor must have string ID"),
                    };
                }
                Some(field_keys::SERIES_TYPE) => {
                    series_type = match selection.id {
                        Some(SelectableId::String(s)) => Some(deserialize_from_string(&s)?),
                        _ => panic!("Series type must have string ID"),
                    };
                }
                Some(field_keys::SEASON_FOLDER) => {
                    season_folder = match selection.id {
                        Some(SelectableId::Boolean(b)) => Some(b),
                        _ => panic!("Season folder must have boolean ID"),
                    };
                }
                _ => panic!("Unknown metadata key: {:?}", detail.metadata),
            }
        }

        Ok(Self {
            rootfolder_path: root_folder_path.expect("Root folder must be selected"),
            quality_profile_id: quality_profile_id.expect("Quality profile must be selected"),
            monitor: monitor.expect("Monitor must be selected"),
            series_type: series_type.expect("Series type must be selected"),
            season_folder: season_folder.expect("Season folder must be selected"),
        })
    }
}

impl MediaItem for SeriesResource {
    fn to_dropdown(&self) -> DropdownOption {
        DropdownOption {
            title: self.title.clone().flatten().unwrap_or_default(),
            description: self.year.map(|y| y.to_string()),
            id: self.id.map(SelectableId::Integer),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

#[async_trait]
impl MediaBackend for Sonarr {
    async fn search(&self, term: &str) -> Result<Vec<Box<dyn MediaItem>>> {
        let results = api_v3_series_lookup_get(&self.config, Some(term)).await?;
        Ok(results
            .into_iter()
            .map(|s| Box::new(s) as Box<dyn MediaItem>)
            .collect())
    }

    fn early_stop(&self, _media: &dyn MediaItem) -> bool {
        // For series, we don't early stop even if the series exists in Sonarr because:
        // 1. The series might be partially monitored (e.g., only "First Season" or "Last Season")
        // 2. Users might want to request new seasons of an existing series
        // 3. Users might want to change monitoring settings (e.g., from "Missing" to "All")
        // 4. Determining if a series is "fully requested" is complex and depends on user intent
        // Sonarr handles duplicate requests gracefully by updating the existing series instead
        // of creating duplicates, so it's safer to always allow the request to proceed.
        false
    }

    fn display_info(&self, media: &dyn MediaItem) -> MediaDisplayInfo {
        let media = media
            .as_any()
            .downcast_ref::<SeriesResource>()
            .context("Invalid media type for Sonarr")
            .unwrap();

        MediaDisplayInfo {
            title: media.title.clone().flatten().unwrap_or_default(),
            subtitle: media.year.map(|y| y.to_string()),
            description: media.overview.clone().flatten(),
            thumbnail_url: media.remote_poster.clone().flatten(),
        }
    }

    fn additional_details(&self, _media: &dyn MediaItem) -> Vec<RequestDetails> {
        self.details.clone().into()
    }

    async fn request(&self, details: Vec<RequestDetails>, media: Box<dyn MediaItem>) -> Result<()> {
        let selected = SelectedDetails::try_from(details)?;

        // Downcast to concrete type
        let mut media = *media
            .into_any()
            .downcast::<SeriesResource>()
            .map_err(|_| anyhow::anyhow!("Invalid media type for Sonarr"))?;

        // Update the media object with the selected options
        media.add_options = Some(Box::new(AddSeriesOptions {
            ignore_episodes_with_files: Some(false),
            ignore_episodes_without_files: Some(false),
            monitor: Some(selected.monitor),
            search_for_cutoff_unmet_episodes: Some(false),
            search_for_missing_episodes: Some(true),
        }));
        media.quality_profile_id = Some(selected.quality_profile_id);
        media.series_type = Some(selected.series_type);
        media.root_folder_path = Some(Some(selected.rootfolder_path));
        media.season_folder = Some(selected.season_folder);

        if selected.monitor != MonitorTypes::None {
            media.monitored = Some(true);
        }

        // Make the API call
        api_v3_series_post(&self.config, Some(media)).await?;

        Ok(())
    }

    fn success_message(&self, media: &dyn MediaItem) -> SuccessMessage {
        let media = media
            .as_any()
            .downcast_ref::<SeriesResource>()
            .context("Invalid media type for Sonarr")
            .unwrap();

        let title = media.title.clone().flatten().unwrap_or_default();
        let year = media.year.unwrap_or_default();
        SuccessMessage {
            title: "Request Successful".to_string(),
            description: format!(
                "{title} ({year}) has been requested and will be downloaded when available.",
            ),
            thumbnail_url: media.remote_poster.clone().flatten(),
        }
    }
}
