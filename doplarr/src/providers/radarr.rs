use super::*;
use crate::config::BackendConfig;
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use radarr_api::{
    apis::{
        Error as RadarrApiError,
        configuration::{ApiKey, Configuration},
        movie_api::api_v3_movie_post,
        movie_lookup_api::api_v3_movie_lookup_get,
        quality_profile_api::api_v3_qualityprofile_get,
        root_folder_api::api_v3_rootfolder_get,
    },
    models::{
        AddMovieOptions, MonitorTypes, MovieResource, MovieStatusType, QualityProfileResource,
        RootFolderResource,
    },
};
use tracing::{debug, error, info, trace, warn};

/// Helper function to log detailed error information from Radarr API responses
fn log_api_error<T: std::fmt::Debug>(err: &RadarrApiError<T>, context: &str) {
    match err {
        RadarrApiError::ResponseError(response) => {
            super::api_logging::log_api_error_details(response.status, &response.content, context);
            if let Some(ref entity) = response.entity {
                debug!("Parsed error entity: {:#?}", entity);
            }
        }
        RadarrApiError::Reqwest(e) => {
            error!("{} - Reqwest error: {}", context, e);
        }
        RadarrApiError::Serde(e) => {
            error!("{} - Serialization error: {}", context, e);
        }
        RadarrApiError::Io(e) => {
            error!("{} - IO error: {}", context, e);
        }
    }
}

/// Treat a 2xx response whose body fails to parse as success - by the time we're
/// reading the body, Radarr has already applied the change
fn tolerate_response_parse_error<T, E>(
    result: std::result::Result<T, RadarrApiError<E>>,
    context: &str,
) -> Result<Option<T>>
where
    E: std::fmt::Debug + Send + Sync + 'static,
{
    match result {
        Ok(x) => Ok(Some(x)),
        Err(RadarrApiError::Serde(e)) => {
            warn!(
                "{} - succeeded, but the response body failed to parse: {}",
                context, e
            );
            Ok(None)
        }
        Err(e) => {
            log_api_error(&e, context);
            Err(e.into())
        }
    }
}

#[derive(Debug, Clone)]
pub struct Radarr {
    config: Configuration,
    details: Details,
}

#[derive(Debug, Clone)]
// All the details we want to collect
pub struct Details {
    rootfolders: Vec<RootFolderResource>,
    quality_profiles: Vec<QualityProfileResource>,
    monitor: Vec<MonitorTypes>,
    minimum_availability: Vec<MovieStatusType>,
}

#[derive(Debug)]
// The final details needed to complete the request
pub struct SelectedDetails {
    pub rootfolder_path: String,
    pub quality_profile_id: i32,
    pub monitor: MonitorTypes,
    pub minimum_availability: MovieStatusType,
}

impl Radarr {
    /// Builds the Radarr connection and attempts to use it
    pub async fn new(
        base_path: String,
        key: String,
        monitor_type: Option<MonitorTypes>,
        quality_profile: Option<String>,
        rootfolder: Option<String>,
        minimum_availability: Option<MovieStatusType>,
        client: reqwest::Client,
    ) -> Result<Self> {
        // Log connection before moving base_path
        info!("Connecting to Radarr at {}", base_path);

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
        let mut rootfolders = api_v3_rootfolder_get(&config).await.inspect_err(|e| {
            log_api_error(e, "Failed to get root folders from Radarr");
        })?;
        trace!("Retrieved {} root folders", rootfolders.len());

        let mut quality_profiles = api_v3_qualityprofile_get(&config).await.inspect_err(|e| {
            log_api_error(e, "Failed to get quality profiles from Radarr");
        })?;
        trace!("Retrieved {} quality profiles", quality_profiles.len());

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

        let minimum_availability = if let Some(x) = minimum_availability {
            vec![x]
        } else {
            vec![
                MovieStatusType::Tba,
                MovieStatusType::Announced,
                MovieStatusType::InCinemas,
                MovieStatusType::Released,
                MovieStatusType::Deleted,
            ]
        };

        let monitor = if let Some(x) = monitor_type {
            vec![x]
        } else {
            vec![
                MonitorTypes::MovieAndCollection,
                MonitorTypes::MovieOnly,
                MonitorTypes::None,
            ]
        };

        // Build the details
        let details = Details {
            rootfolders,
            quality_profiles,
            monitor,
            minimum_availability,
        };

        Ok(Self { config, details })
    }

    pub async fn connect(backend: BackendConfig, client: reqwest::Client) -> Result<Self> {
        if let BackendConfig::Radarr {
            url,
            api_key,
            monitor_type,
            quality_profile,
            rootfolder,
            minimum_availability,
        } = backend
        {
            Self::new(
                url,
                api_key,
                monitor_type,
                quality_profile,
                rootfolder,
                minimum_availability,
                client,
            )
            .await
        } else {
            bail!("Configured backend not for Radarr");
        }
    }
}

/// Helper function to get to and from stringified references
fn deserialize_from_string<T: serde::de::DeserializeOwned>(s: &str) -> Result<T> {
    serde_json::from_str(&format!("\"{}\"", s))
        .with_context(|| format!("Failed to deserialize enum variant: {}", s))
}

mod field_keys {
    pub const ROOT_FOLDER: &str = "radarr:root_folder";
    pub const MONITOR: &str = "radarr:monitor";
    pub const AVAILABILITY: &str = "radarr:availability";
    pub const QUALITY_PROFILE: &str = "radarr:quality_profile";
}

impl From<Details> for Vec<RequestDetails> {
    fn from(details: Details) -> Vec<RequestDetails> {
        let quality_profile_options = details
            .quality_profiles
            .iter()
            .filter_map(|x| {
                let name = x.name.clone().flatten();
                if name.is_none() {
                    warn!("Skipping quality profile with no name (id: {:?})", x.id);
                }
                name.map(|n| DropdownOption {
                    title: n,
                    description: None,
                    id: x.id.map(SelectableId::Integer),
                })
            })
            .collect();

        let quality_profile_details = RequestDetails {
            title: "Quality Profile".to_string(),
            options: quality_profile_options,
            metadata: Some(field_keys::QUALITY_PROFILE.to_string()),
            field_type: FieldType::Dropdown,
            always_show: false,
        };

        let rootfolder_options = details
            .rootfolders
            .iter()
            .filter_map(|x| {
                let path = x.path.clone().flatten();
                if path.is_none() {
                    warn!("Skipping root folder with no path (id: {:?})", x.id);
                }
                path.map(|p| DropdownOption {
                    title: p,
                    description: None,
                    id: x.id.map(SelectableId::Integer),
                })
            })
            .collect();

        let rootfolder_details = RequestDetails {
            title: "Root Folder".to_string(),
            options: rootfolder_options,
            metadata: Some(field_keys::ROOT_FOLDER.to_string()),
            field_type: FieldType::Dropdown,
            always_show: false,
        };

        let monitor_options = details
            .monitor
            .iter()
            .map(|x| {
                let title = match x {
                    MonitorTypes::MovieOnly => "Movie Only",
                    MonitorTypes::MovieAndCollection => "Movie and Collection",
                    MonitorTypes::None => "None",
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
            always_show: false,
        };

        let availability_options = details
            .minimum_availability
            .iter()
            .map(|x| {
                let title = match x {
                    MovieStatusType::Announced => "Announced",
                    MovieStatusType::InCinemas => "In Cinemas",
                    MovieStatusType::Released => "Released",
                    MovieStatusType::Tba => "To Be Announced",
                    MovieStatusType::Deleted => "Deleted",
                };
                DropdownOption {
                    title: title.to_string(),
                    description: None,
                    id: Some(SelectableId::String(x.to_string())),
                }
            })
            .collect();

        let availability_details = RequestDetails {
            title: "Minimum Availability".to_string(),
            options: availability_options,
            metadata: Some(field_keys::AVAILABILITY.to_string()),
            field_type: FieldType::Dropdown,
            always_show: false,
        };

        vec![
            rootfolder_details,
            monitor_details,
            availability_details,
            quality_profile_details,
        ]
    }
}

impl TryFrom<Vec<RequestDetails>> for SelectedDetails {
    type Error = anyhow::Error;

    fn try_from(details: Vec<RequestDetails>) -> Result<Self> {
        let mut root_folder_path = None;
        let mut quality_profile_id = None;
        let mut monitor = None;
        let mut minimum_availability = None;

        for detail in details {
            let Some(selection) = detail.options.into_iter().next() else {
                bail!("No option was selected for '{}'", detail.title);
            };

            match detail.metadata.as_deref() {
                Some(field_keys::ROOT_FOLDER) => {
                    root_folder_path = Some(selection.title);
                }
                Some(field_keys::QUALITY_PROFILE) => {
                    quality_profile_id = match selection.id {
                        Some(SelectableId::Integer(i)) => Some(i),
                        other => bail!("Quality profile must have an integer ID, got {other:?}"),
                    };
                }
                Some(field_keys::MONITOR) => {
                    monitor = match selection.id {
                        Some(SelectableId::String(s)) => Some(deserialize_from_string(&s)?),
                        other => bail!("Monitor must have a string ID, got {other:?}"),
                    };
                }
                Some(field_keys::AVAILABILITY) => {
                    minimum_availability = match selection.id {
                        Some(SelectableId::String(s)) => Some(deserialize_from_string(&s)?),
                        other => bail!("Availability must have a string ID, got {other:?}"),
                    };
                }
                other => bail!("Unknown metadata key: {other:?}"),
            }
        }

        Ok(Self {
            rootfolder_path: root_folder_path.context("No root folder was selected")?,
            quality_profile_id: quality_profile_id.context("No quality profile was selected")?,
            monitor: monitor.context("No monitor type was selected")?,
            minimum_availability: minimum_availability
                .context("No minimum availability was selected")?,
        })
    }
}

impl MediaItem for MovieResource {
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
impl MediaBackend for Radarr {
    async fn search(&self, term: &str) -> Result<Vec<Box<dyn MediaItem>>> {
        info!("Searching Radarr for movie: {}", term);
        let results = api_v3_movie_lookup_get(&self.config, Some(term))
            .await
            .inspect_err(|e| {
                log_api_error(e, "Failed to search Radarr");
            })?;
        debug!("Found {} movie results", results.len());
        Ok(results
            .into_iter()
            .map(|m| Box::new(m) as Box<dyn MediaItem>)
            .collect())
    }

    fn early_stop(&self, media: &dyn MediaItem) -> bool {
        media
            .as_any()
            .downcast_ref::<MovieResource>()
            .map(|m| m.id.is_some())
            .unwrap_or(false)
    }

    fn display_info(&self, media: &dyn MediaItem) -> MediaDisplayInfo {
        let Some(media) = media.as_any().downcast_ref::<MovieResource>() else {
            error!("display_info called with wrong media type for Radarr backend");
            return MediaDisplayInfo {
                title: String::new(),
                subtitle: None,
                description: None,
                thumbnail_url: None,
            };
        };

        MediaDisplayInfo {
            title: media.title.clone().flatten().unwrap_or_default(),
            subtitle: media.year.map(|y| y.to_string()),
            description: media.overview.clone().flatten(),
            thumbnail_url: media.remote_poster.clone().flatten(),
        }
    }

    async fn additional_details(&self, _media: &dyn MediaItem) -> Result<Vec<RequestDetails>> {
        Ok(self.details.clone().into())
    }

    async fn request(
        &self,
        details: Vec<RequestDetails>,
        media: Box<dyn MediaItem>,
        _requester_discord_id: u64,
    ) -> Result<()> {
        let selected = SelectedDetails::try_from(details)?;

        // Downcast to concrete type
        let mut media = *media
            .into_any()
            .downcast::<MovieResource>()
            .map_err(|_| anyhow::anyhow!("Invalid media type for Radarr"))?;

        // Update the media object with the selected options
        media.add_options = Some(Box::new(AddMovieOptions {
            monitor: Some(selected.monitor),
            search_for_movie: Some(true),
            ..Default::default()
        }));
        media.quality_profile_id = Some(selected.quality_profile_id);
        media.minimum_availability = Some(selected.minimum_availability);
        media.root_folder_path = Some(Some(selected.rootfolder_path.clone()));

        if selected.monitor != MonitorTypes::None {
            media.monitored = Some(true);
        }

        info!(
            "Requesting movie: {} (tmdb_id: {:?})",
            media.title.clone().flatten().unwrap_or_default(),
            media.tmdb_id
        );
        debug!(
            "Request details - rootfolder: {}, quality_profile_id: {}, monitor: {:?}, minimum_availability: {:?}",
            selected.rootfolder_path,
            selected.quality_profile_id,
            selected.monitor,
            selected.minimum_availability
        );
        trace!("Full media object: {:#?}", media);

        // Make the API call
        tolerate_response_parse_error(
            api_v3_movie_post(&self.config, Some(media)).await,
            "Failed to add movie to Radarr",
        )?;

        Ok(())
    }

    fn success_message(
        &self,
        _details: &[RequestDetails],
        media: &dyn MediaItem,
    ) -> SuccessMessage {
        let Some(media) = media.as_any().downcast_ref::<MovieResource>() else {
            error!("success_message called with wrong media type for Radarr backend");
            return SuccessMessage {
                summary: "Request submitted".into(),
                description: "Will be downloaded when available.".into(),
                thumbnail_url: None,
            };
        };

        let title = media.title.clone().flatten().unwrap_or_default();
        let year = media.year.unwrap_or_default();
        SuccessMessage {
            summary: format!("{title} ({year})"),
            description: "Will be downloaded when available.".to_string(),
            thumbnail_url: media.remote_poster.clone().flatten(),
        }
    }
}
