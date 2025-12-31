use super::*;
use crate::config::MovieBackend;
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
use tracing::{debug, error, info, trace};

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

    #[allow(irrefutable_let_patterns)]
    pub async fn connect(backend: MovieBackend, client: reqwest::Client) -> Result<Self> {
        if let MovieBackend::Radarr {
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
                Some(field_keys::AVAILABILITY) => {
                    minimum_availability = match selection.id {
                        Some(SelectableId::String(s)) => Some(deserialize_from_string(&s)?),
                        _ => panic!("Availability must have string ID"),
                    };
                }
                _ => panic!("Unknown metadata key: {:?}", detail.metadata),
            }
        }

        Ok(Self {
            rootfolder_path: root_folder_path.expect("Root folder must be selected"),
            quality_profile_id: quality_profile_id.expect("Quality profile must be selected"),
            monitor: monitor.expect("Monitor must be selected"),
            minimum_availability: minimum_availability
                .expect("Minimum availability must be selected"),
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
        let media = media
            .as_any()
            .downcast_ref::<MovieResource>()
            .context("Invalid media type for Radarr")
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
        api_v3_movie_post(&self.config, Some(media))
            .await
            .inspect_err(|e| {
                log_api_error(e, "Failed to add movie to Radarr");
            })?;

        Ok(())
    }

    fn success_message(
        &self,
        _details: &[RequestDetails],
        media: &dyn MediaItem,
    ) -> SuccessMessage {
        let media = media
            .as_any()
            .downcast_ref::<MovieResource>()
            .context("Invalid media type for Radarr")
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
