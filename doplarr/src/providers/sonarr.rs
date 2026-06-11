use super::*;
use crate::{config::BackendConfig, discord::MAX_DROPDOWN_OPTIONS};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use sonarr_api::{
    apis::{
        Error as SonarrApiError,
        command_api::api_v3_command_post_custom,
        configuration::{ApiKey, Configuration},
        quality_profile_api::api_v3_qualityprofile_get,
        root_folder_api::api_v3_rootfolder_get,
        series_api::{api_v3_series_id_get, api_v3_series_id_put, api_v3_series_post},
        series_lookup_api::api_v3_series_lookup_get,
    },
    commands::SeasonSearchCommand,
    models::{
        AddSeriesOptions, MonitorTypes, QualityProfileResource, RootFolderResource, SeriesResource,
        SeriesTypes,
    },
};
use tracing::{debug, error, info, trace, warn};

/// Helper function to log detailed error information from Sonarr API responses
fn log_api_error<T: std::fmt::Debug>(err: &SonarrApiError<T>, context: &str) {
    match err {
        SonarrApiError::ResponseError(response) => {
            super::api_logging::log_api_error_details(response.status, &response.content, context);
            if let Some(ref entity) = response.entity {
                debug!("Parsed error entity: {:#?}", entity);
            }
        }
        SonarrApiError::Reqwest(e) => {
            error!("{} - Reqwest error: {}", context, e);
        }
        SonarrApiError::Serde(e) => {
            error!("{} - Serialization error: {}", context, e);
        }
        SonarrApiError::Io(e) => {
            error!("{} - IO error: {}", context, e);
        }
    }
}

/// Treat a 2xx response whose body fails to parse as success - by the time we're
/// reading the body, Sonarr has already applied the change
fn tolerate_response_parse_error<T, E>(
    result: std::result::Result<T, SonarrApiError<E>>,
    context: &str,
) -> Result<Option<T>>
where
    E: std::fmt::Debug + Send + Sync + 'static,
{
    match result {
        Ok(x) => Ok(Some(x)),
        Err(SonarrApiError::Serde(e)) => {
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
pub struct Sonarr {
    config: Configuration,
    details: Details,
    /// Whether Season 0 (specials) can be requested for existing series
    allow_specials: bool,
}

#[derive(Debug, Clone)]
// All the details we want to collect
pub struct Details {
    rootfolders: Vec<RootFolderResource>,
    quality_profiles: Vec<QualityProfileResource>,
    monitor: Vec<MonitorTypes>,
    /// Config-pinned series type; when unset, it's auto-detected per series
    series_type: Option<SeriesTypes>,
    season_folder: Option<bool>,
}

#[derive(Debug)]
// The final details needed to complete the request
pub struct SelectedDetails {
    pub rootfolder_path: Option<String>, // Only for new series - existing series inherit
    pub quality_profile_id: Option<i32>, // Only for new series
    pub monitor: Option<MonitorTypes>,   // Only for new series
    pub series_type: Option<SeriesTypes>, // Only for new series
    pub season_folder: Option<bool>,     // Only for new series - existing series inherit
    pub season_number: Option<i32>,      // Only for existing series - which season to monitor
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
        allowed_monitor_types: Option<Vec<MonitorTypes>>,
        allow_specials: bool,
        client: reqwest::Client,
    ) -> Result<Self> {
        // Log connection before moving base_path
        info!("Connecting to Sonarr at {}", base_path);

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
            log_api_error(e, "Failed to get root folders from Sonarr");
        })?;
        trace!("Retrieved {} root folders", rootfolders.len());

        let mut quality_profiles = api_v3_qualityprofile_get(&config).await.inspect_err(|e| {
            log_api_error(e, "Failed to get quality profiles from Sonarr");
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

        let monitor = if let Some(x) = monitor_type {
            vec![x]
        } else if let Some(allowed) = allowed_monitor_types {
            // Use admin-configured allowed monitor types
            allowed
        } else {
            // Default user-facing options
            vec![
                MonitorTypes::All,
                MonitorTypes::FirstSeason,
                MonitorTypes::LastSeason,
                MonitorTypes::LatestSeason,
                MonitorTypes::Pilot,
                MonitorTypes::Recent,
                MonitorTypes::MonitorSpecials,
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

        Ok(Self {
            config,
            details,
            allow_specials,
        })
    }

    #[allow(irrefutable_let_patterns)]
    pub async fn connect(backend: BackendConfig, client: reqwest::Client) -> Result<Self> {
        if let BackendConfig::Sonarr {
            url,
            api_key,
            monitor_type,
            quality_profile,
            rootfolder,
            series_type,
            season_folders,
            allowed_monitor_types,
            allow_specials,
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
                allowed_monitor_types,
                allow_specials.unwrap_or(false),
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
    pub const SEASON: &str = "sonarr:season";
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
                    MonitorTypes::MonitorSpecials => "Specials",
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
            always_show: false,
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
            always_show: false,
        };

        vec![
            rootfolder_details,
            monitor_details,
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
        let mut season_number = None;

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
                Some(field_keys::SERIES_TYPE) => {
                    series_type = match selection.id {
                        Some(SelectableId::String(s)) => Some(deserialize_from_string(&s)?),
                        other => bail!("Series type must have a string ID, got {other:?}"),
                    };
                }
                Some(field_keys::SEASON_FOLDER) => {
                    season_folder = match selection.id {
                        Some(SelectableId::Boolean(b)) => Some(b),
                        other => bail!("Season folder must have a boolean ID, got {other:?}"),
                    };
                }
                Some(field_keys::SEASON) => {
                    season_number = match selection.id {
                        Some(SelectableId::Integer(i)) => Some(i),
                        other => bail!("Season must have an integer ID, got {other:?}"),
                    };
                }
                other => bail!("Unknown metadata key: {other:?}"),
            }
        }

        Ok(Self {
            rootfolder_path: root_folder_path, // Optional - only for new series
            quality_profile_id,                // Optional - only for new series
            monitor,                           // Optional - only for new series
            series_type,                       // Optional - only for new series
            season_folder,                     // Optional - only for new series
            season_number,                     // Optional - only for existing series
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
        info!("Searching Sonarr for series: {}", term);
        let results = api_v3_series_lookup_get(&self.config, Some(term))
            .await
            .inspect_err(|e| {
                log_api_error(e, "Failed to search Sonarr");
            })?;
        debug!("Found {} series results", results.len());
        Ok(results
            .into_iter()
            .map(|s| Box::new(s) as Box<dyn MediaItem>)
            .collect())
    }

    fn early_stop(&self, media: &dyn MediaItem) -> bool {
        let Some(media) = media.as_any().downcast_ref::<SeriesResource>() else {
            error!("early_stop called with wrong media type for Sonarr backend");
            return false;
        };

        // Check if series exists and all requestable seasons are already monitored
        // (when specials are disabled, an unmonitored Season 0 doesn't count)
        if let Some(id) = media.id
            && let Some(Some(ref seasons)) = media.seasons
        {
            let all_monitored = seasons
                .iter()
                .filter(|s| self.allow_specials || s.season_number.unwrap_or(0) != 0)
                .all(|s| s.monitored.unwrap_or(false));

            if all_monitored && !seasons.is_empty() {
                info!(series_id = id, "Series already fully monitored");
                return true;
            }
        }

        // Otherwise, allow the request to proceed
        // Users can select individual unmonitored seasons to add
        false
    }

    fn display_info(&self, media: &dyn MediaItem) -> MediaDisplayInfo {
        let Some(media) = media.as_any().downcast_ref::<SeriesResource>() else {
            error!("display_info called with wrong media type for Sonarr backend");
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

    async fn additional_details(&self, media: &dyn MediaItem) -> Result<Vec<RequestDetails>> {
        let media = media
            .as_any()
            .downcast_ref::<SeriesResource>()
            .context("Invalid media type for Sonarr")?;

        let mut details: Vec<RequestDetails> = self.details.clone().into();

        // If series already exists in Sonarr
        if media.id.is_some() {
            debug!("Series already exists, showing only season selector");

            // Filter out fields that should be inherited from existing series
            details.retain(|d| {
                !matches!(
                    d.metadata.as_deref(),
                    Some(field_keys::QUALITY_PROFILE)
                        | Some(field_keys::SERIES_TYPE)
                        | Some(field_keys::MONITOR)
                        | Some(field_keys::ROOT_FOLDER)
                        | Some(field_keys::SEASON_FOLDER)
                )
            });

            // Add season selector showing only unmonitored seasons
            // (Season 0 is only requestable when specials are enabled)
            if let Some(Some(ref seasons)) = media.seasons {
                let mut unmonitored_seasons: Vec<_> = seasons
                    .iter()
                    .filter(|s| !s.monitored.unwrap_or(false))
                    .filter(|s| self.allow_specials || s.season_number.unwrap_or(0) != 0)
                    .collect();

                if !unmonitored_seasons.is_empty() {
                    // Sort by season number descending (most recent first)
                    unmonitored_seasons.sort_by(|a, b| {
                        let a_num = a.season_number.unwrap_or(0);
                        let b_num = b.season_number.unwrap_or(0);
                        b_num.cmp(&a_num)
                    });

                    let total_unmonitored = unmonitored_seasons.len();
                    if total_unmonitored > MAX_DROPDOWN_OPTIONS {
                        debug!(
                            total_unmonitored = total_unmonitored,
                            showing = MAX_DROPDOWN_OPTIONS,
                            "Series has more unmonitored seasons than Discord dropdown limit, showing most recent"
                        );
                    }

                    let season_options: Vec<DropdownOption> = unmonitored_seasons
                        .into_iter()
                        .take(MAX_DROPDOWN_OPTIONS)
                        .map(|s| {
                            let season_num = s.season_number.unwrap_or(0);
                            let title = if season_num == 0 {
                                "Season 0 (Specials)".to_string()
                            } else {
                                format!("Season {}", season_num)
                            };
                            DropdownOption {
                                title,
                                description: None,
                                id: Some(SelectableId::Integer(season_num)),
                            }
                        })
                        .collect();

                    let season_details = RequestDetails {
                        title: "Season to Monitor".to_string(),
                        options: season_options,
                        metadata: Some(field_keys::SEASON.to_string()),
                        field_type: FieldType::Dropdown,
                        // Even a lone season should be reviewable before requesting
                        always_show: true,
                    };

                    // Insert season details at the end
                    details.push(season_details);
                }
            }

            // Without a season to offer, the request flow has nothing to do
            // (e.g. the lookup payload had no season data at all)
            if details.is_empty() {
                bail!("Series is already in Sonarr but has no seasons available to request");
            }
        } else {
            // New series: series type is Sonarr arcana most requesters won't
            // understand, so don't ask - use the config pin if present,
            // otherwise auto-detect anime from the lookup's genres
            let series_type = self.details.series_type.unwrap_or_else(|| {
                let is_anime = matches!(&media.genres, Some(Some(genres))
                    if genres.iter().any(|g| g.eq_ignore_ascii_case("anime")));
                if is_anime {
                    SeriesTypes::Anime
                } else {
                    SeriesTypes::Standard
                }
            });
            debug!(series_type = %series_type, "Resolved series type");

            let title = match series_type {
                SeriesTypes::Standard => "Standard",
                SeriesTypes::Daily => "Daily",
                SeriesTypes::Anime => "Anime",
            };
            details.push(RequestDetails {
                title: "Series Type".to_string(),
                options: vec![DropdownOption {
                    title: title.to_string(),
                    description: None,
                    id: Some(SelectableId::String(series_type.to_string())),
                }],
                metadata: Some(field_keys::SERIES_TYPE.to_string()),
                field_type: FieldType::Dropdown,
                always_show: false,
            });
        }

        Ok(details)
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
            .downcast::<SeriesResource>()
            .map_err(|_| anyhow::anyhow!("Invalid media type for Sonarr"))?;

        info!(
            "Requesting series: {} (tvdb_id: {:?})",
            media.title.clone().flatten().unwrap_or_default(),
            media.tvdb_id
        );

        // Check if series already exists in Sonarr (has an ID)
        if let Some(id) = media.id {
            info!(
                series_id = id,
                "Series already exists in Sonarr, adding season to monitoring"
            );

            let season_number = selected
                .season_number
                .context("No season was selected for an existing series")?;

            debug!(season_number = season_number, "Adding season to monitoring");

            // Get the current series data
            let mut existing_series = api_v3_series_id_get(&self.config, id, None)
                .await
                .inspect_err(|e| {
                    log_api_error(e, "Failed to get existing series from Sonarr");
                })?;

            debug!(
                existing_quality_profile = ?existing_series.quality_profile_id,
                existing_series_type = ?existing_series.series_type,
                "Preserving existing series settings"
            );

            // Find and monitor the selected season (additive only)
            if let Some(Some(ref mut seasons)) = existing_series.seasons {
                let season = seasons
                    .iter_mut()
                    .find(|s| s.season_number == Some(season_number));

                match season {
                    Some(season) => {
                        season.monitored = Some(true);
                        info!(
                            season_number = season_number,
                            "Season marked for monitoring"
                        );
                    }
                    None => bail!("Season {} not found in series", season_number),
                }
            } else {
                bail!(
                    "Season {} not found in series (no seasons array)",
                    season_number
                );
            }

            // Update series monitored flag
            existing_series.monitored = Some(true);

            trace!("Updated series object: {:#?}", existing_series);

            // PUT the updated series back
            tolerate_response_parse_error(
                api_v3_series_id_put(&self.config, &id.to_string(), None, Some(existing_series))
                    .await,
                "Failed to update series in Sonarr",
            )?;

            // Trigger a search scoped to the newly monitored season
            info!("Triggering search for newly monitored season");
            let search_command = SeasonSearchCommand::new(id, season_number);
            trace!("Search command: {:?}", search_command);

            let result = tolerate_response_parse_error(
                api_v3_command_post_custom(&self.config, &search_command).await,
                "Failed to trigger season search",
            )?;

            info!(
                "Search command queued successfully, command_id: {:?}",
                result.and_then(|r| r.id)
            );
        } else {
            info!("Series is new, adding to Sonarr");

            let monitor = selected
                .monitor
                .context("No monitor type was selected for a new series")?;
            let rootfolder_path = selected
                .rootfolder_path
                .context("No root folder was selected for a new series")?;
            let season_folder = selected
                .season_folder
                .context("No season folder choice was selected for a new series")?;
            let quality_profile_id = selected
                .quality_profile_id
                .context("No quality profile was selected for a new series")?;
            let series_type = selected
                .series_type
                .context("No series type was selected for a new series")?;

            debug!(
                "Request details - rootfolder: {}, quality_profile_id: {:?}, monitor: {:?}, series_type: {:?}, season_folder: {}",
                rootfolder_path, quality_profile_id, monitor, series_type, season_folder
            );

            // Update the media object with the selected options
            media.add_options = Some(Box::new(AddSeriesOptions {
                ignore_episodes_with_files: Some(false),
                ignore_episodes_without_files: Some(false),
                monitor: Some(monitor),
                search_for_cutoff_unmet_episodes: Some(false),
                search_for_missing_episodes: Some(true),
            }));
            media.root_folder_path = Some(Some(rootfolder_path));
            media.season_folder = Some(season_folder);

            if monitor != MonitorTypes::None {
                media.monitored = Some(true);
            }

            // Set quality profile and series type
            media.quality_profile_id = Some(quality_profile_id);
            media.series_type = Some(series_type);

            trace!("Full media object: {:#?}", media);

            tolerate_response_parse_error(
                api_v3_series_post(&self.config, Some(media)).await,
                "Failed to add series to Sonarr",
            )?;
        }

        Ok(())
    }

    fn success_message(&self, details: &[RequestDetails], media: &dyn MediaItem) -> SuccessMessage {
        let Some(media) = media.as_any().downcast_ref::<SeriesResource>() else {
            error!("success_message called with wrong media type for Sonarr backend");
            return SuccessMessage {
                summary: "Request submitted".into(),
                description: "Will be downloaded when available.".into(),
                thumbnail_url: None,
            };
        };

        let title = media.title.clone().flatten().unwrap_or_default();
        let year = media.year.unwrap_or_default();

        // Check if this was adding a season or creating a new series
        let detail_text = if media.id.is_some() {
            // Existing series - find which season was added
            details
                .iter()
                .find(|d| d.metadata.as_deref() == Some(field_keys::SEASON))
                .and_then(|d| d.options.first())
                .map(|opt| format!(" ({})", opt.title))
                .unwrap_or_else(|| " (new season)".to_string())
        } else {
            // New series - find monitor type
            details
                .iter()
                .find(|d| d.metadata.as_deref() == Some(field_keys::MONITOR))
                .and_then(|d| d.options.first())
                .map(|opt| format!(" ({})", opt.title))
                .unwrap_or_default()
        };

        SuccessMessage {
            summary: format!("{title} ({year}){detail_text}"),
            description: "Will be downloaded when available.".to_string(),
            thumbnail_url: media.remote_poster.clone().flatten(),
        }
    }
}
