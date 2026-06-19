use crate::providers::{
    DropdownOption, MediaBackend, MediaDisplayInfo, MediaItem, RequestDetails, SuccessMessage,
};
use anyhow::Context;
use std::{sync::Arc, time::Duration};
use tokio::{sync::mpsc::Receiver, time::timeout};
use tracing::{debug, error, info, trace};
use twilight_http::Client as HttpClient;
use twilight_model::{
    application::{
        command::{Command, CommandType},
        interaction::message_component::MessageComponentInteractionData,
    },
    channel::message::{
        Component, MessageFlags,
        component::{ActionRow, ButtonStyle, SelectMenuType, UnfurledMediaItem},
    },
    http::interaction::{InteractionResponse, InteractionResponseType},
    id::{
        Id,
        marker::{ApplicationMarker, ChannelMarker, InteractionMarker, UserMarker},
    },
};
use twilight_util::builder::{
    InteractionResponseDataBuilder,
    command::{CommandBuilder, StringBuilder, SubCommandBuilder},
    message::{
        ActionRowBuilder, ButtonBuilder, ContainerBuilder, SectionBuilder, SelectMenuBuilder,
        SelectMenuOptionBuilder, SeparatorBuilder, TextDisplayBuilder, ThumbnailBuilder,
    },
};
use uuid::Uuid;

pub const TOP_LEVEL_COMMAND_NAME: &str = "request";
pub const QUERY_COMMAND_NAME: &str = "query";
pub const TIMEOUT_MESSAGE: &str = "Interaction timed out, please try again";
pub const EARLY_STOP_MESSAGE: &str = "Already requested - nothing more to add";

/// Discord's maximum number of options in a dropdown menu
pub const MAX_DROPDOWN_OPTIONS: usize = 25;

/// Discord's maximum character length for text content in components
const MAX_TEXT_CONTENT_LENGTH: usize = 4000;

const ACCENT_COLOR: u32 = 0xCE4A28;

fn escape_markdown(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('~', "\\~")
        .replace('|', "\\|")
}

const INTERACTION_TIMEOUT_DURATION: Duration = Duration::from_secs(300);

/// Truncate text to Discord's component text limit, respecting char boundaries
fn truncate_text(text: &str) -> String {
    if text.len() <= MAX_TEXT_CONTENT_LENGTH {
        return text.to_string();
    }
    let mut end = MAX_TEXT_CONTENT_LENGTH - 3;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

/// Build the comand object, used to register with Discord what slash commands are available
pub fn commands<T: AsRef<str>>(media_kinds: impl IntoIterator<Item = T>) -> Command {
    let query = StringBuilder::new(QUERY_COMMAND_NAME, "search query").required(true);
    let mut request_command = CommandBuilder::new(
        TOP_LEVEL_COMMAND_NAME,
        "Request media",
        CommandType::ChatInput,
    );
    for kind in media_kinds {
        request_command = request_command.option(
            SubCommandBuilder::new(kind.as_ref(), format!("Request {}", kind.as_ref()))
                .option(query.clone()),
        )
    }
    request_command.build()
}

/// Updates an existing interaction with a new component (ephemeral and supporting V2 components)
async fn update_interaction_component(
    client: &Arc<HttpClient>,
    application_id: Id<ApplicationMarker>,
    interaction_token: &str,
    component: Component,
) -> anyhow::Result<()> {
    client
        .interaction(application_id)
        .update_response(interaction_token)
        .components(Some(&[component]))
        .flags(MessageFlags::IS_COMPONENTS_V2 | MessageFlags::EPHEMERAL)
        .await?;
    Ok(())
}

/// Responds to an interaction with an updated message, using a comonent as the body (ephemeral / supporting V2 components)
async fn respond_interaction_component(
    client: &Arc<HttpClient>,
    application_id: Id<ApplicationMarker>,
    interaction_id: Id<InteractionMarker>,
    interaction_token: &str,
    component: Component,
) -> anyhow::Result<()> {
    client
        .interaction(application_id)
        .create_response(
            interaction_id,
            interaction_token,
            &InteractionResponse {
                kind: InteractionResponseType::UpdateMessage,
                data: Some(
                    InteractionResponseDataBuilder::new()
                        .flags(MessageFlags::IS_COMPONENTS_V2 | MessageFlags::EPHEMERAL)
                        .components(vec![component])
                        .build(),
                ),
            },
        )
        .await?;
    Ok(())
}

/// Acknowledge a component interaction without changing the message, so Discord
/// doesn't show "interaction failed" for events we intentionally ignore
async fn ack_component(
    client: &Arc<HttpClient>,
    application_id: Id<ApplicationMarker>,
    interaction_id: Id<InteractionMarker>,
    interaction_token: &str,
) -> anyhow::Result<()> {
    client
        .interaction(application_id)
        .create_response(
            interaction_id,
            interaction_token,
            &InteractionResponse {
                kind: InteractionResponseType::DeferredUpdateMessage,
                data: None,
            },
        )
        .await?;
    Ok(())
}

/// Responds to an interaction request with an ack that lets us modify it later
pub async fn send_thinking(
    client: &Arc<HttpClient>,
    application_id: Id<ApplicationMarker>,
    interaction_id: Id<InteractionMarker>,
    interaction_token: &str,
) -> anyhow::Result<()> {
    client
        .interaction(application_id)
        .create_response(
            interaction_id,
            interaction_token,
            &InteractionResponse {
                kind: InteractionResponseType::DeferredChannelMessageWithSource,
                data: Some(
                    InteractionResponseDataBuilder::new()
                        .flags(MessageFlags::IS_COMPONENTS_V2 | MessageFlags::EPHEMERAL)
                        .build(),
                ),
            },
        )
        .await?;
    Ok(())
}

/// Convert a vector of [DropdownOption] into a discord Select Menu, keyed by the vec index
fn dropdown_options_to_select_menu<T: AsRef<str>>(
    options: Vec<DropdownOption>,
    id: T,
    uuid: Uuid,
    placeholder: Option<String>,
    disabled: bool,
) -> ActionRow {
    let mut menu = SelectMenuBuilder::new(format!("{}:{uuid}", id.as_ref()), SelectMenuType::Text)
        .disabled(disabled);

    if let Some(placeholder) = placeholder {
        menu = menu.placeholder(placeholder);
    }

    for (i, option) in options.into_iter().enumerate() {
        let mut menu_option = SelectMenuOptionBuilder::new(option.title, i.to_string());
        if let Some(x) = option.description {
            menu_option = menu_option.description(x);
        }
        menu = menu.option(menu_option);
    }

    ActionRowBuilder::new().component(menu.build()).build()
}

/// Using the result payload from a search, create a dropdown that will select a search result
pub async fn update_search_results_component(
    uuid: Uuid,
    results: &[Box<dyn MediaItem>],
    client: &Arc<HttpClient>,
    application_id: Id<ApplicationMarker>,
    interaction_token: &str,
) -> anyhow::Result<()> {
    // Create the select menu option from the payload
    let options = results.iter().map(|x| x.to_dropdown()).collect();
    let dropdown = dropdown_options_to_select_menu(options, "result", uuid, None, false);

    let component = ContainerBuilder::new()
        .accent_color(Some(ACCENT_COLOR))
        .component(TextDisplayBuilder::new("# Search Results").build())
        .component(SeparatorBuilder::new().build())
        .component(dropdown)
        .build()
        .into();

    // And update the interaction with discord
    update_interaction_component(client, application_id, interaction_token, component).await?;
    Ok(())
}

pub async fn update_string_message(
    content: &str,
    client: &Arc<HttpClient>,
    application_id: Id<ApplicationMarker>,
    interaction_token: &str,
) -> anyhow::Result<()> {
    let component = ContainerBuilder::new()
        .accent_color(Some(ACCENT_COLOR))
        .component(TextDisplayBuilder::new(content).build())
        .build()
        .into();
    update_interaction_component(client, application_id, interaction_token, component).await?;
    Ok(())
}

pub async fn update_timeout(
    client: &Arc<HttpClient>,
    application_id: Id<ApplicationMarker>,
    interaction_token: &str,
) -> anyhow::Result<()> {
    update_string_message(TIMEOUT_MESSAGE, client, application_id, interaction_token).await
}

fn build_request_component(
    uuid: Uuid,
    display_info: &MediaDisplayInfo,
    request_details: &[RequestDetails],
    user_selectable_fields: &std::collections::HashSet<String>,
    submitting: bool,
) -> Component {
    // Build the container that holds everything
    let mut container = ContainerBuilder::new().accent_color(Some(ACCENT_COLOR));

    // Build the media overview
    if let Some(thumbnail_url) = &display_info.thumbnail_url {
        let mut section = SectionBuilder::new(
            ThumbnailBuilder::new(UnfurledMediaItem {
                url: thumbnail_url.clone(),
                proxy_url: None,
                height: None,
                width: None,
                content_type: None,
            })
            .build(),
        )
        .component(
            TextDisplayBuilder::new(format!("# {}", escape_markdown(&display_info.title))).build(),
        );

        // Only add subtitle if it exists
        if let Some(subtitle) = &display_info.subtitle {
            section = section.component(
                TextDisplayBuilder::new(format!("-# {}", escape_markdown(subtitle))).build(),
            );
        }

        let overview = display_info
            .description
            .as_deref()
            .filter(|s| !s.is_empty())
            .map_or("*Overview unavailable.*", |s| s);
        section = section.component(TextDisplayBuilder::new(truncate_text(overview)).build());

        container = container.component(section.build());
    } else {
        container = container.component(
            TextDisplayBuilder::new(format!("# {}", escape_markdown(&display_info.title))).build(),
        );
        if let Some(subtitle) = &display_info.subtitle {
            container = container.component(
                TextDisplayBuilder::new(format!("-# {}", escape_markdown(subtitle))).build(),
            );
        }
        let overview = display_info
            .description
            .as_deref()
            .filter(|s| !s.is_empty())
            .map_or("*Overview unavailable.*", |s| s);
        container = container.component(TextDisplayBuilder::new(truncate_text(overview)).build());
    }

    // Build the additional options
    // Show dropdowns that still need selection, and text for completed selections
    let mut selections_remaining = false;

    for detail in request_details {
        // Only show fields that were user-selectable (had multiple options initially)
        let is_user_selectable = detail
            .metadata
            .as_ref()
            .map(|m| user_selectable_fields.contains(m))
            .unwrap_or(false);

        if !is_user_selectable {
            // Skip config defaults (fields that always had 1 option)
            continue;
        }

        if detail.options.len() > 1 {
            // User needs to choose - show dropdown
            selections_remaining = true;
            let row = dropdown_options_to_select_menu(
                detail.options.clone(),
                detail.title.clone(),
                uuid,
                None,
                submitting,
            );

            container = container
                .component(SeparatorBuilder::new().build())
                .component(TextDisplayBuilder::new(format!("### {}", detail.title)).build())
                .component(row);
        } else if detail.options.len() == 1 {
            // User has selected - show as text for review
            let selection = detail.options.first().unwrap().title.clone();
            container = container
                .component(SeparatorBuilder::new().build())
                .component(
                    TextDisplayBuilder::new(format!("### {}\n{}", detail.title, selection)).build(),
                );
        }
    }

    // Build the request button (disabled if selections still needed or already submitting)
    container = container.component(SeparatorBuilder::new().build());
    let request_button = ButtonBuilder::new(ButtonStyle::Primary)
        .label(if submitting {
            "Requesting..."
        } else {
            "Request"
        })
        .custom_id(format!("request:{uuid}"))
        .disabled(selections_remaining || submitting)
        .build();

    container = container.component(ActionRowBuilder::new().component(request_button).build());

    container.build().into()
}

fn build_completion_component(message: &SuccessMessage) -> Component {
    let mut container = ContainerBuilder::new().accent_color(Some(ACCENT_COLOR));

    let heading =
        TextDisplayBuilder::new(format!("# {}", escape_markdown(&message.summary))).build();
    let body = TextDisplayBuilder::new(&message.description).build();

    if let Some(thumbnail_url) = &message.thumbnail_url {
        let section = SectionBuilder::new(
            ThumbnailBuilder::new(UnfurledMediaItem {
                url: thumbnail_url.clone(),
                proxy_url: None,
                height: None,
                width: None,
                content_type: None,
            })
            .build(),
        )
        .component(heading)
        .component(body)
        .build();
        container = container.component(section);
    } else {
        container = container.component(heading).component(body);
    }

    container.build().into()
}

#[derive(Debug)]
/// Data needed to start an interaction flow
pub struct InteractionStart {
    pub uuid: Uuid,
    pub rx: Receiver<InteractionContinue>,
    pub query: String,
    pub interaction_id: Id<InteractionMarker>,
    pub application_id: Id<ApplicationMarker>,
    pub token: String,
    pub user_id: Id<UserMarker>,
    pub channel_id: Id<ChannelMarker>,
}

#[derive(Debug)]
/// Data needed to continue an interaction flow
pub struct InteractionContinue {
    pub data: Box<MessageComponentInteractionData>,
    pub interaction_id: Id<InteractionMarker>,
    pub token: String,
}

/// The coroutine that runs the request interaction to completion
pub async fn run_interaction(
    start: InteractionStart,
    discord_http: Arc<HttpClient>,
    backend: Arc<dyn MediaBackend>,
    public_followup: bool,
) -> anyhow::Result<()> {
    // Destructure some some of the starting data
    let InteractionStart {
        uuid,
        mut rx,
        query,
        interaction_id,
        application_id,
        token,
        user_id,
        channel_id,
    } = start;

    info!(uuid = %uuid, query = %query, "Starting interaction flow");

    // Send the "thinking" ack so we can take some time to actually perform the request
    // This is done over the HTTP client connection
    send_thinking(&discord_http, application_id, interaction_id, &token).await?;

    debug!(query = %query, "Performing search");
    let mut results = backend.search(&query).await?;
    info!(count = results.len(), "Search completed");

    // Check if there were no results
    if results.is_empty() {
        info!("No search results found");
        update_string_message("No results", &discord_http, application_id, &token).await?;
        return Ok(());
    }

    // Discord allows a maximum of 25 options in a dropdown
    if results.len() > MAX_DROPDOWN_OPTIONS {
        info!(
            "Truncating {} results to {} for Discord dropdown",
            results.len(),
            MAX_DROPDOWN_OPTIONS
        );
        results.truncate(MAX_DROPDOWN_OPTIONS);
    }

    // Now update the interaction with all of the options that result from the search
    trace!("Showing search results to user");
    update_search_results_component(
        uuid,
        results.as_slice(),
        &discord_http,
        application_id,
        &token,
    )
    .await?;

    // Now wait for the user to select an option, which will come in on the channel
    // An abandoned interaction is a normal outcome, not an error
    debug!("Waiting for user to select a search result");
    let mut next = match timeout(INTERACTION_TIMEOUT_DURATION, rx.recv()).await {
        Ok(Some(val)) => val,
        Ok(None) | Err(_) => {
            info!("User abandoned the interaction at search result selection");
            update_timeout(&discord_http, application_id, &token).await?;
            return Ok(());
        }
    };
    trace!(data = ?next, "Got the next interaction");

    // Use the value from this next payload to get the index into the search results to process
    let selection_idx: usize = next
        .data
        .values
        .first()
        .and_then(|v| v.parse().ok())
        .filter(|idx| *idx < results.len())
        .context("Search result selection didn't map to a valid result")?;

    let selection = results.remove(selection_idx);
    info!(index = selection_idx, "User made selection");
    trace!(selection = ?selection, "Selection details");

    // Now check the early stop critera
    if backend.early_stop(&*selection) {
        info!("Stopping early - media already requested");
        update_string_message(EARLY_STOP_MESSAGE, &discord_http, application_id, &token).await?;
        return Ok(());
    }
    debug!("Selection has not been requested, continuing interaction");

    // Now, we need to collect the additional information needed to perform the request
    debug!("Fetching additional details required");
    let mut additional_details = backend.additional_details(&*selection).await?;
    trace!(details = ?additional_details, "Request details");

    // Track which fields to show in the UI: ones the user must choose from
    // (multiple options), plus ones the backend wants reviewed regardless
    let user_selectable_fields: std::collections::HashSet<_> = additional_details
        .iter()
        .filter(|detail| detail.options.len() > 1 || detail.always_show)
        .filter_map(|detail| detail.metadata.as_ref())
        .cloned()
        .collect();

    let display_info = backend.display_info(&*selection);
    let mut request_container = build_request_component(
        uuid,
        &display_info,
        &additional_details,
        &user_selectable_fields,
        false,
    );

    respond_interaction_component(
        &discord_http,
        application_id,
        next.interaction_id,
        &next.token,
        request_container,
    )
    .await?;

    // Collect all the selections
    loop {
        debug!("Waiting for user to select a detail option");
        next = match timeout(INTERACTION_TIMEOUT_DURATION, rx.recv()).await {
            Ok(Some(val)) => val,
            Ok(None) | Err(_) => {
                info!("User abandoned the interaction at detail selection");
                update_timeout(&discord_http, application_id, &token).await?;
                return Ok(());
            }
        };
        trace!(data = ?next, "Got interaction from additional details");

        // Check if this was the final "Request" button click
        if next.data.custom_id.starts_with("request:") {
            info!("User clicked Request button, all details collected");

            // Acknowledge the button click immediately (before 3-second timeout),
            // disabling everything so it can't be clicked again while we submit
            respond_interaction_component(
                &discord_http,
                application_id,
                next.interaction_id,
                &next.token,
                build_request_component(
                    uuid,
                    &display_info,
                    &additional_details,
                    &user_selectable_fields,
                    true,
                ),
            )
            .await?;

            break;
        }

        // Map the response back to one of our details, ignoring stale or malformed
        // events (e.g. a second click on a dropdown we already collapsed)
        let stale = 'event: {
            let Some((title, _)) = next.data.custom_id.split_once(':') else {
                break 'event Some("custom id has no uuid suffix");
            };
            let Some(detail_idx) = additional_details.iter().position(|x| x.title == title) else {
                break 'event Some("no detail matching custom id");
            };
            let Some(option_idx) = next.data.values.first().and_then(|v| v.parse().ok()) else {
                break 'event Some("selection value is not a valid index");
            };
            if additional_details[detail_idx].options.len() <= 1 {
                break 'event Some("detail was already selected");
            }
            if option_idx >= additional_details[detail_idx].options.len() {
                break 'event Some("selection index out of bounds");
            }

            let selected_option = additional_details[detail_idx].options.remove(option_idx);
            debug!(detail = %title, selected = %selected_option.title, "User selected detail option");

            // Replace the vector of options with the selected option
            additional_details[detail_idx].options = vec![selected_option];
            None
        };

        if let Some(reason) = stale {
            debug!(data = ?next.data, reason = reason, "Ignoring component event");
            ack_component(
                &discord_http,
                application_id,
                next.interaction_id,
                &next.token,
            )
            .await?;
            continue;
        }

        // Update the component to show the selection
        request_container = build_request_component(
            uuid,
            &display_info,
            &additional_details,
            &user_selectable_fields,
            false,
        );

        respond_interaction_component(
            &discord_http,
            application_id,
            next.interaction_id,
            &next.token,
            request_container,
        )
        .await?;
        trace!("Updated component with selection");

        // Check if all the detail options have only one item in them
        if additional_details.iter().all(|x| x.options.len() == 1) {
            debug!("All details have been selected, waiting for final Request button click");
        }
    }

    info!("All options collected, performing request");
    trace!(options = ?additional_details, "Collected options");

    // Perform the actual request
    let success_msg = backend.success_message(&additional_details, &*selection);
    backend
        .request(additional_details, selection, user_id.get())
        .await?;
    info!("Request completed successfully");

    // Update the message with success (using original token since we already responded to button click)
    update_interaction_component(
        &discord_http,
        application_id,
        &token,
        build_completion_component(&success_msg),
    )
    .await
    .context("Failed to send success response")?;

    // Send public message to channel if configured
    // Plain content only: it's the one thing OS notification previews render
    if public_followup {
        let content = format!(
            "{} requested by <@{}>",
            escape_markdown(&success_msg.summary),
            user_id
        );
        let result = discord_http
            .create_message(channel_id)
            .content(&content)
            .await;

        if let Err(ref e) = result {
            error!("Discord channel message error: {:?}", e);
        }
        result.context("Failed to send channel message")?;
    }

    info!(uuid = %uuid, "Interaction flow completed successfully");
    Ok(())
}
