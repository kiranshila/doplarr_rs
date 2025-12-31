use anyhow::bail;
use clap::Parser;
use config::{MovieBackend, SeriesBackend};
use discord::InteractionContinue;
use providers::{MediaBackend, radarr::Radarr, sonarr::Sonarr};
use std::{collections::HashMap, sync::Arc, time::Instant};
use tokio::sync::{Mutex, mpsc};
use tokio::time::{Duration, interval};
use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::EnvFilter;
use twilight_cache_inmemory::{DefaultInMemoryCache, ResourceType};
use twilight_gateway::{Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt as _};
use twilight_http::Client as HttpClient;
use twilight_model::application::interaction::{
    InteractionData, application_command::CommandOptionValue,
};

pub mod args;
pub mod config;
pub mod discord;
pub mod providers;

/// Sanitize error messages for Discord users while keeping full details in logs
fn user_facing_error(err: &anyhow::Error) -> &'static str {
    let err_msg = err.to_string().to_lowercase();

    // Provide specific guidance for known error types
    if err_msg.contains("timeout") || err_msg.contains("timed out") {
        "Request timed out. The backend server may be slow or unavailable."
    } else if err_msg.contains("connection") || err_msg.contains("connect") {
        "Could not connect to the backend server. Please try again later."
    } else if err_msg.contains("401")
        || err_msg.contains("403")
        || err_msg.contains("unauthorized")
        || err_msg.contains("forbidden")
    {
        "Backend authentication error. Please contact your administrator."
    } else if err_msg.contains("500") || err_msg.contains("502") || err_msg.contains("503") {
        "The backend server encountered an error. Please try again later."
    } else {
        // Generic message that doesn't leak any internal details
        "An error occurred while processing your request. Please try again or contact your administrator."
    }
}

type InteractionMap = Arc<Mutex<HashMap<uuid::Uuid, (mpsc::Sender<InteractionContinue>, Instant)>>>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse command line args to get path to config file
    let cli = args::Cli::parse();

    // Read the config file
    let config = config::Config::from_file(cli.config_file.unwrap())?;

    // Setup logging with configured level
    let log_level = config.log_level.as_deref().unwrap_or("info");
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level));
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    // Build the HTTP request client for backend calls with a reasonable timeout
    let backend_http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()?;

    // Connect to all available backends and cast into a trait object
    // NOTE: You'd create other media type backend connections connections here
    let movie_backend = match config.movie_backend {
        Some(backend_config @ MovieBackend::Radarr { .. }) => {
            let radarr = Radarr::connect(backend_config, backend_http.clone()).await?;
            Some(Arc::new(radarr) as Arc<dyn MediaBackend>)
        }
        None => None,
    };
    let series_backend = match config.series_backend {
        Some(backend_config @ SeriesBackend::Sonarr { .. }) => {
            let sonarr = Sonarr::connect(backend_config, backend_http).await?;
            Some(Arc::new(sonarr) as Arc<dyn MediaBackend>)
        }
        None => None,
    };

    // Check that we have at least one backend client
    // NOTE: Check all backends here when new media types are added
    if movie_backend.is_none() && series_backend.is_none() {
        bail!("At least one media backend is required!");
    }

    // We only need to listen for interactions (commands are registered via HTTP on READY)
    let mut shard = Shard::new(ShardId::ONE, config.discord_token.clone(), Intents::empty());

    // Create the HTTP client we use to send data *back* to Discord
    let discord_http = Arc::new(HttpClient::new(config.discord_token));

    // Cache the application ID for repeated use later in the process.
    let application_id = {
        let response = discord_http.current_user_application().await?;
        response.model().await?.id
    };

    // Build the list of media types we'll register commands for
    // NOTE: Add other media types here when needed
    let mut command_list = vec![];
    if movie_backend.is_some() {
        command_list.push("movie");
    }
    if series_backend.is_some() {
        command_list.push("series");
    }
    info!("Available backends: {:?}", command_list);

    // Cache interactions
    let cache = DefaultInMemoryCache::builder()
        .resource_types(ResourceType::INTEGRATION)
        .build();

    // Build our map that holds each interaction -> (sender, timestamp) for the particular event flow
    let in_progress_interactions: InteractionMap = Arc::new(Mutex::new(HashMap::new()));

    // Spawn a background task to clean up abandoned interactions
    const INTERACTION_TIMEOUT: Duration = Duration::from_secs(300);
    const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);
    {
        let in_progress = Arc::clone(&in_progress_interactions);
        tokio::spawn(async move {
            let mut ticker = interval(CLEANUP_INTERVAL);
            loop {
                ticker.tick().await;
                let mut map = in_progress.lock().await;
                let now = Instant::now();
                let before_count = map.len();

                map.retain(|uuid, (_tx, timestamp)| {
                    let age = now.duration_since(*timestamp);
                    if age > INTERACTION_TIMEOUT {
                        debug!(uuid = %uuid, age_secs = age.as_secs(), "Cleaning up abandoned interaction");
                        false
                    } else {
                        true
                    }
                });

                let removed = before_count - map.len();
                if removed > 0 {
                    info!("Cleaned up {} abandoned interaction(s)", removed);
                }
            }
        });
    }

    // Finally, process the stream of events as they come in
    while let Some(item) = shard
        .next_event(EventTypeFlags::READY | EventTypeFlags::INTERACTION_CREATE)
        .await
    {
        // Make sure we have a good event
        let Ok(event) = item else {
            error!(source = ?item.unwrap_err(), "Error receiving event");
            continue;
        };

        // Update the cache with the event.
        cache.update(&event);

        // Determine if we're starting an interaction flow or updating an existing one
        match event {
            Event::Ready(_) => {
                info!("Connected to Discord's server");

                // Fetch all guilds the bot is in and register commands
                match discord_http.current_user_guilds().await {
                    Ok(guilds_response) => {
                        let guilds = guilds_response.models().await.unwrap_or_default();
                        info!("Bot is in {} guild(s)", guilds.len());

                        let command = discord::commands(&command_list);
                        for guild in guilds {
                            info!(
                                "Registering commands to guild: {} ({})",
                                guild.name, guild.id
                            );

                            if let Err(e) = discord_http
                                .interaction(application_id)
                                .set_guild_commands(guild.id, std::slice::from_ref(&command))
                                .await
                            {
                                error!(error = %e, guild_id = %guild.id, "Failed to register commands to guild");
                            } else {
                                info!(
                                    "Successfully registered {} subcommands to guild {}",
                                    command_list.len(),
                                    guild.id
                                );
                            }
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to fetch guilds");
                    }
                }
            }
            Event::InteractionCreate(interaction) => {
                trace!(data = ?interaction, "Got interaction event");
                match &interaction.data {
                    Some(InteractionData::ApplicationCommand(command_data)) => {
                        debug!(data = ?command_data, "Got application command");
                        // New interaction
                        // We now dispatch on the "name" of the interaction which selects the media kind, called with the query string
                        let (media_kind, query) = if command_data.name
                            == discord::TOP_LEVEL_COMMAND_NAME
                            && let Some(subcommand) = command_data.options.first()
                            && let CommandOptionValue::SubCommand(x) = &subcommand.value
                            && let Some(option) = x.first()
                            && option.name == discord::QUERY_COMMAND_NAME
                            && let CommandOptionValue::String(value) = &option.value
                        {
                            (subcommand.name.clone(), value.clone())
                        } else {
                            warn!(data = ?command_data, "Interaction body didn't match what we expected",);
                            return Ok(());
                        };
                        info!(kind = media_kind, query = query, "Got search request");

                        // Create the channel that we'll push data through
                        let (tx, rx) = mpsc::channel(1);

                        // Add this channel to our map of in-progress interactions
                        let uuid = uuid::Uuid::new_v4();
                        in_progress_interactions
                            .lock()
                            .await
                            .insert(uuid, (tx, Instant::now()));

                        // Build the start data
                        let start = discord::InteractionStart {
                            uuid,
                            rx,
                            query,
                            interaction_id: interaction.id,
                            application_id,
                            token: interaction.token.clone(),
                            user_id: interaction
                                .author_id()
                                .expect("Interaction must have a user"),
                            channel_id: interaction
                                .channel
                                .as_ref()
                                .expect("Interaction must have a channel")
                                .id,
                        };

                        // Spawn the coroutine
                        tokio::spawn({
                            // Clone the HTTP clients so we can spawn the async task
                            let discord_http = Arc::clone(&discord_http);
                            let in_progress = Arc::clone(&in_progress_interactions);
                            let public_followup = config.public_followup.unwrap_or(true);

                            // NOTE: Add match statements here for new media type
                            let backend = match media_kind.as_str() {
                                "movie" => movie_backend.clone(),
                                "series" => series_backend.clone(),
                                _ => unreachable!(),
                            }
                            .expect("This will exist as we've checked earlier");

                            async move {
                                // Keep token for error handling
                                let interaction_token = start.token.clone();

                                if let Err(e) = discord::run_interaction(
                                    start,
                                    discord_http.clone(),
                                    backend,
                                    public_followup,
                                )
                                .await
                                {
                                    // Log full error details for admin debugging
                                    error!(error = %e, "Failed to run coroutine to completion");

                                    // Show sanitized error to Discord user (no sensitive info)
                                    let user_msg = user_facing_error(&e);
                                    if let Err(update_err) = discord::update_string_message(
                                        user_msg,
                                        &discord_http,
                                        application_id,
                                        &interaction_token,
                                    )
                                    .await
                                    {
                                        warn!(error = %update_err, "Failed to send error message to user");
                                    }
                                }

                                // Clean up the interaction from the map
                                in_progress.lock().await.remove(&uuid);
                                debug!(uuid = %uuid, "Cleaned up completed interaction");
                            }
                        });
                    }
                    Some(InteractionData::MessageComponent(component_data)) => {
                        debug!(data=?component_data, "Got message component");
                        // This is a continuation of an interaction, send this update payload through the channel to the spawned coroutine
                        // Extract the UUID from the update message and push this new data into the associated channel to move that coroutine forward
                        if let Some((_, uuid)) = component_data.custom_id.split_once(':')
                            && let Ok(uuid) = uuid::Uuid::parse_str(uuid)
                        {
                            let tx = in_progress_interactions
                                .lock()
                                .await
                                .get(&uuid)
                                .map(|(tx, _)| tx.clone());
                            match tx {
                                Some(tx) => {
                                    // Build the continuation data
                                    let cont = InteractionContinue {
                                        data: component_data.clone(),
                                        interaction_id: interaction.id,
                                        token: interaction.token.clone(),
                                    };
                                    // Try to send, if it doesn't work that means the other side timed out
                                    match tx.try_send(cont) {
                                        Ok(_) => {
                                            trace!("Sent continuation to interaction coroutine");
                                        }
                                        Err(_) => {
                                            // Other side timed out
                                            warn!(uuid = %uuid, "Interaction coroutine timed out");
                                            discord::update_timeout(
                                                &discord_http,
                                                application_id,
                                                &interaction.token,
                                            )
                                            .await.unwrap_or_else(|e| {
                                                warn!(error = %e, "Failed to update interaction with timeout message");
                                            });
                                            // Remove the TX from the map
                                            in_progress_interactions.lock().await.remove(&uuid);
                                            debug!(uuid = %uuid, "Removed timed out interaction from map");
                                        }
                                    }
                                }
                                None => {
                                    // User wanted to continue an interaction that we don't have an ID for, impling we cleaned it up from timeout
                                    // Alternatively, a user continued an interaction from a previous run of the bot, which means we don't have any interaction to update!
                                    warn!(uuid = %uuid, "No active interaction found for continuation");
                                    discord::update_timeout(
                                        &discord_http,
                                        application_id,
                                        &interaction.token,
                                    )
                                    .await.unwrap_or_else(|e| {
                                        warn!(error = %e, "Failed to update interaction with timeout message");
                                 });
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => debug!(event = ?event, "Got non-handled event"),
        }
    }
    Ok(())
}
