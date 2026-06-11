use super::*;
use crate::{config::BackendConfig, discord::MAX_DROPDOWN_OPTIONS};
use anyhow::{Result, bail};
use async_trait::async_trait;
use seerr_api::{
    apis::{
        Error as SeerrApiError,
        auth_api::auth_me_get,
        configuration::{ApiKey, Configuration},
        request_api::request_post,
        search_api::search_get,
        tv_api::tv_tv_id_get,
        users_api::{user_get, user_user_id_settings_notifications_get},
    },
    models::{_request_post_request::MediaType, RequestPostRequest, RequestPostRequestSeasons},
};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

fn log_api_error<T: std::fmt::Debug>(err: &SeerrApiError<T>, context: &str) {
    match err {
        SeerrApiError::ResponseError(response) => {
            super::api_logging::log_api_error_details(response.status, &response.content, context);
            if let Some(ref entity) = response.entity {
                debug!("Parsed error entity: {:#?}", entity);
            }
        }
        SeerrApiError::Reqwest(e) => error!("{} - Reqwest error: {}", context, e),
        SeerrApiError::Serde(e) => error!("{} - Serialization error: {}", context, e),
        SeerrApiError::Io(e) => error!("{} - IO error: {}", context, e),
    }
}

/// Log the API error details, then convert to anyhow for propagation.
fn require<T, E>(result: std::result::Result<T, SeerrApiError<E>>, context: &str) -> Result<T>
where
    E: std::fmt::Debug + Send + Sync + 'static,
{
    result.map_err(|e| {
        log_api_error(&e, context);
        anyhow::Error::from(e)
    })
}

fn tolerate_response_parse_error<T, E>(
    result: std::result::Result<T, SeerrApiError<E>>,
    context: &str,
) -> Result<Option<T>>
where
    E: std::fmt::Debug + Send + Sync + 'static,
{
    match result {
        Ok(x) => Ok(Some(x)),
        Err(SeerrApiError::Serde(e)) => {
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

struct UserMapCache {
    map: HashMap<u64, i32>,
    fetched_at: Instant,
}

pub struct Seerr {
    config: Configuration,
    fallback_user_id: Option<i32>,
    allow_4k: bool,
    user_cache: RwLock<Option<UserMapCache>>,
}

impl std::fmt::Debug for Seerr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Seerr")
            .field("base_path", &self.config.base_path)
            .finish()
    }
}

impl Seerr {
    pub async fn connect(backend: BackendConfig, client: reqwest::Client) -> Result<Self> {
        let BackendConfig::Seerr {
            url,
            api_key,
            fallback_user_id,
            allow_4k,
        } = backend
        else {
            bail!("Expected Seerr config");
        };

        let base_path = format!("{}/api/v1", url.trim_end_matches('/'));
        let config = Configuration {
            base_path,
            client,
            api_key: Some(ApiKey {
                prefix: None,
                key: api_key,
            }),
            ..Default::default()
        };

        require(auth_me_get(&config).await, "Seerr auth validation")?;
        info!("Connected to Seerr at {}", config.base_path);

        Ok(Self {
            config,
            fallback_user_id,
            allow_4k: allow_4k.unwrap_or(false),
            user_cache: RwLock::new(None),
        })
    }

    async fn resolve_seerr_user(&self, discord_id: u64) -> Result<Option<i32>> {
        const DEBOUNCE: Duration = Duration::from_secs(30);

        {
            let cache = self.user_cache.read().await;
            if let Some(ref c) = *cache
                && c.fetched_at.elapsed() < DEBOUNCE
            {
                return Ok(c.map.get(&discord_id).copied());
            }
        }

        let mut cache = self.user_cache.write().await;
        // Re-check after acquiring write lock
        if let Some(ref c) = *cache
            && c.fetched_at.elapsed() < DEBOUNCE
        {
            return Ok(c.map.get(&discord_id).copied());
        }

        let mut map: HashMap<u64, i32> = HashMap::new();
        let mut skip = 0.0f64;
        loop {
            let page = require(
                user_get(
                    &self.config,
                    Some(100.0),
                    Some(skip),
                    None,
                    None,
                    None,
                    None,
                )
                .await,
                "Fetching Seerr users",
            )?;

            let users = page.results.unwrap_or_default();
            if users.is_empty() {
                break;
            }

            for user in &users {
                let notif = require(
                    user_user_id_settings_notifications_get(&self.config, user.id as f64).await,
                    "Fetching user notification settings",
                )?;

                if let Some(Some(ids)) = notif.discord_ids {
                    for id_str in ids {
                        if let Ok(did) = id_str.parse::<u64>() {
                            map.insert(did, user.id);
                        }
                    }
                }
            }

            let total = page.page_info.and_then(|p| p.results).unwrap_or(0.0) as usize;

            skip += users.len() as f64;
            if skip as usize >= total {
                break;
            }
        }

        info!(
            "Seerr user cache refreshed: {} linked Discord user(s)",
            map.len()
        );
        let result = map.get(&discord_id).copied();
        *cache = Some(UserMapCache {
            map,
            fetched_at: Instant::now(),
        });
        Ok(result)
    }
}

// The search result type is our MediaItem for Seerr
use seerr_api::models::SearchGet200ResponseResultsInner as SeerrResult;

impl MediaItem for SeerrResult {
    fn to_dropdown(&self) -> DropdownOption {
        let display_name = match self.media_type.as_str() {
            "tv" => self.name.as_deref().unwrap_or("Unknown"),
            _ => self.title.as_deref().unwrap_or("Unknown"),
        };
        let year = match self.media_type.as_str() {
            "tv" => self.first_air_date.as_deref().and_then(|d| d.get(..4)),
            _ => self.release_date.as_deref().and_then(|d| d.get(..4)),
        };
        let type_tag = match self.media_type.as_str() {
            "movie" => "Movie",
            "tv" => "Series",
            _ => &self.media_type,
        };
        let description = match year {
            Some(y) => format!("{type_tag} · {y}"),
            None => type_tag.to_string(),
        };
        DropdownOption {
            title: display_name.to_string(),
            description: Some(description),
            id: Some(SelectableId::Integer(self.id as i32)),
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }
}

#[async_trait]
impl MediaBackend for Seerr {
    async fn search(&self, term: &str) -> Result<Vec<Box<dyn MediaItem>>> {
        let response = require(
            search_get(&self.config, term, None, None).await,
            "Seerr search",
        )?;

        let results = response
            .results
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.media_type == "movie" || r.media_type == "tv")
            .map(|r| Box::new(r) as Box<dyn MediaItem>)
            .collect();

        Ok(results)
    }

    fn early_stop(&self, media: &dyn MediaItem) -> bool {
        let Some(result) = media.as_any().downcast_ref::<SeerrResult>() else {
            return false;
        };
        let Some(ref info) = result.media_info else {
            return false;
        };
        let Some(status) = info.status else {
            return false;
        };
        match result.media_type.as_str() {
            "movie" => (2.0..=5.0).contains(&status),
            "tv" => status == 5.0,
            _ => false,
        }
    }

    fn display_info(&self, media: &dyn MediaItem) -> MediaDisplayInfo {
        let Some(result) = media.as_any().downcast_ref::<SeerrResult>() else {
            return MediaDisplayInfo {
                title: "Unknown".into(),
                subtitle: None,
                description: None,
                thumbnail_url: None,
            };
        };
        let title = match result.media_type.as_str() {
            "tv" => result.name.clone().unwrap_or_else(|| "Unknown".into()),
            _ => result.title.clone().unwrap_or_else(|| "Unknown".into()),
        };
        let year = match result.media_type.as_str() {
            "tv" => result
                .first_air_date
                .as_deref()
                .and_then(|d| d.get(..4))
                .map(str::to_string),
            _ => result
                .release_date
                .as_deref()
                .and_then(|d| d.get(..4))
                .map(str::to_string),
        };
        let thumbnail_url = result
            .poster_path
            .as_ref()
            .map(|p| format!("https://image.tmdb.org/t/p/w500{p}"));
        MediaDisplayInfo {
            title,
            subtitle: year,
            description: result.overview.clone(),
            thumbnail_url,
        }
    }

    async fn additional_details(&self, media: &dyn MediaItem) -> Result<Vec<RequestDetails>> {
        let Some(result) = media.as_any().downcast_ref::<SeerrResult>() else {
            return Ok(vec![]);
        };

        let quality_step = self.allow_4k.then(|| RequestDetails {
            title: "Quality".into(),
            options: vec![
                DropdownOption {
                    title: "Standard".into(),
                    description: None,
                    id: Some(SelectableId::Boolean(false)),
                },
                DropdownOption {
                    title: "4K".into(),
                    description: None,
                    id: Some(SelectableId::Boolean(true)),
                },
            ],
            metadata: Some("seerr:is_4k".into()),
            field_type: FieldType::Dropdown,
            always_show: true,
        });

        if result.media_type != "tv" {
            return Ok(quality_step.into_iter().collect());
        }

        let tv_id = result.id;
        let details = require(
            tv_tv_id_get(&self.config, tv_id, None).await,
            "Fetching TV details",
        )?;

        let mut options = vec![DropdownOption {
            title: "All Seasons".into(),
            description: None,
            id: Some(SelectableId::String("all".into())),
        }];

        let season_options: Vec<_> = details
            .seasons
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.season_number.is_some_and(|n| n > 0.0))
            .map(|s| {
                let n = s.season_number.unwrap() as i32;
                DropdownOption {
                    title: format!("Season {n}"),
                    description: None,
                    id: Some(SelectableId::Integer(n)),
                }
            })
            .collect();

        if season_options.is_empty() {
            bail!(UserFacingError("No requestable seasons found.".into()));
        }

        options.extend(season_options);
        if options.len() > MAX_DROPDOWN_OPTIONS {
            warn!(
                showing = MAX_DROPDOWN_OPTIONS,
                total = options.len(),
                "Truncating season list to fit Discord dropdown limit"
            );
            options.truncate(MAX_DROPDOWN_OPTIONS);
        }

        let season_step = RequestDetails {
            title: "Season".into(),
            options,
            metadata: Some("seerr:season".into()),
            field_type: FieldType::Dropdown,
            always_show: true,
        };

        Ok(quality_step
            .into_iter()
            .chain(std::iter::once(season_step))
            .collect())
    }

    async fn request(
        &self,
        details: Vec<RequestDetails>,
        media: Box<dyn MediaItem>,
        requester_discord_id: u64,
    ) -> Result<()> {
        let result = media
            .into_any()
            .downcast::<SeerrResult>()
            .map_err(|_| anyhow::anyhow!("Unexpected media type for Seerr backend"))?;

        let seerr_user_id = match self.resolve_seerr_user(requester_discord_id).await? {
            Some(id) => id,
            None => match self.fallback_user_id {
                Some(id) => id,
                None => bail!(UserFacingError(format!(
                    "Your Discord account (ID: {requester_discord_id}) is not linked to a Seerr account. \
                     To link it, go to your Seerr profile → Settings → Notifications → Discord and enter your Discord User ID."
                ))),
            },
        };

        let media_type = match result.media_type.as_str() {
            "tv" => MediaType::Tv,
            _ => MediaType::Movie,
        };

        let is_4k = details
            .iter()
            .find(|d| d.metadata.as_deref() == Some("seerr:is_4k"))
            .and_then(|d| d.options.first())
            .and_then(|o| match &o.id {
                Some(SelectableId::Boolean(v)) => Some(*v),
                _ => None,
            })
            .unwrap_or(false);

        let seasons = if media_type == MediaType::Tv {
            details
                .iter()
                .find(|d| d.metadata.as_deref() == Some("seerr:season"))
                .and_then(|d| d.options.first())
                .and_then(|o| match &o.id {
                    Some(SelectableId::String(s)) => {
                        Some(RequestPostRequestSeasons::String(s.clone()))
                    }
                    Some(SelectableId::Integer(n)) => {
                        Some(RequestPostRequestSeasons::ArrayVecf64(vec![*n as f64]))
                    }
                    _ => None,
                })
        } else {
            None
        };

        let mut req = RequestPostRequest::new(media_type, result.id);
        req.user_id = Some(Some(seerr_user_id as f64));
        req.is4k = Some(is_4k);
        req.seasons = seasons.map(Box::new);

        tolerate_response_parse_error(request_post(&self.config, req).await, "Seerr request")?;
        Ok(())
    }

    fn success_message(&self, details: &[RequestDetails], media: &dyn MediaItem) -> SuccessMessage {
        let Some(result) = media.as_any().downcast_ref::<SeerrResult>() else {
            return SuccessMessage {
                summary: "Request submitted".into(),
                description: "Your request has been submitted.".into(),
                thumbnail_url: None,
            };
        };

        let title = match result.media_type.as_str() {
            "tv" => result.name.clone().unwrap_or_else(|| "Unknown".into()),
            _ => result.title.clone().unwrap_or_else(|| "Unknown".into()),
        };
        let year = match result.media_type.as_str() {
            "tv" => result
                .first_air_date
                .as_deref()
                .and_then(|d| d.get(..4))
                .map(str::to_string),
            _ => result
                .release_date
                .as_deref()
                .and_then(|d| d.get(..4))
                .map(str::to_string),
        };

        let season_suffix = details
            .iter()
            .find(|d| d.metadata.as_deref() == Some("seerr:season"))
            .and_then(|d| d.options.first())
            .map(|o| match &o.id {
                Some(SelectableId::String(_)) => " (All Seasons)".to_string(),
                Some(SelectableId::Integer(n)) => format!(" (Season {n})"),
                _ => String::new(),
            })
            .unwrap_or_default();

        let base = match year {
            Some(y) => format!("{title} ({y})"),
            None => title.clone(),
        };
        let summary = format!("{base}{season_suffix}");

        let thumbnail_url = result
            .poster_path
            .as_ref()
            .map(|p| format!("https://image.tmdb.org/t/p/w500{p}"));

        SuccessMessage {
            summary,
            description: "Your request has been submitted to Seerr.".into(),
            thumbnail_url,
        }
    }
}
