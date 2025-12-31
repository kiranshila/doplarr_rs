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

const INTERACTION_TIMEOUT_DURATION: Duration = Duration::from_secs(300);

/// Build the comand object, used to register with Discord what slash commands are available
pub fn commands<T: AsRef<str>>(media_kinds: &[T]) -> Command {
    let query = StringBuilder::new(QUERY_COMMAND_NAME, "search query").required(true);
    let mut request_command = CommandBuilder::new(
        TOP_LEVEL_COMMAND_NAME,
        "Request media",
        CommandType::ChatInput,
    );
    for kind in media_kinds {
        request_command = request_command.option(
            SubCommandBuilder::new(kind.as_ref(), format!("Request a {}", kind.as_ref()))
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
    update_interaction_component(
        client,
        application_id,
        interaction_token,
        TextDisplayBuilder::new(content).build().into(),
    )
    .await?;
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
        .component(TextDisplayBuilder::new(format!("# {}", display_info.title)).build());

        // Only add subtitle if it exists
        if let Some(subtitle) = &display_info.subtitle {
            section =
                section.component(TextDisplayBuilder::new(format!("-# {}", subtitle)).build());
        }

        // Only add description if it exists, and truncate if needed
        if let Some(description) = &display_info.description {
            let truncated = if description.len() > MAX_TEXT_CONTENT_LENGTH {
                format!("{}...", &description[..MAX_TEXT_CONTENT_LENGTH - 3])
            } else {
                description.clone()
            };
            section = section.component(TextDisplayBuilder::new(&truncated).build());
        }

        container = container.component(section.build());
    } else {
        container = container
            .component(TextDisplayBuilder::new(format!("# {}", display_info.title)).build());
        if let Some(subtitle) = &display_info.subtitle {
            container =
                container.component(TextDisplayBuilder::new(format!("-# {}", subtitle)).build());
        }
        if let Some(description) = &display_info.description {
            let truncated = if description.len() > MAX_TEXT_CONTENT_LENGTH {
                format!("{}...", &description[..MAX_TEXT_CONTENT_LENGTH - 3])
            } else {
                description.clone()
            };
            container = container.component(TextDisplayBuilder::new(&truncated).build());
        }
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
                false,
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

    // Build the request button (disabled if selections still needed)
    container = container.component(SeparatorBuilder::new().build());
    let request_button = ButtonBuilder::new(ButtonStyle::Primary)
        .label("Request")
        .custom_id(format!("request:{uuid}"))
        .disabled(selections_remaining)
        .build();

    container = container.component(ActionRowBuilder::new().component(request_button).build());

    container.build().into()
}

fn build_completion_component(message: &SuccessMessage) -> Component {
    ContainerBuilder::new()
        .accent_color(Some(ACCENT_COLOR))
        .component(TextDisplayBuilder::new("# Request Submitted").build())
        .component(TextDisplayBuilder::new(&message.description).build())
        .build()
        .into()
}

fn build_success_component(user_id: Id<UserMarker>, message: &SuccessMessage) -> Component {
    let description_with_mention =
        format!("{}\n\n-# Requested by <@{}>", message.description, user_id);

    let mut container = ContainerBuilder::new().accent_color(Some(ACCENT_COLOR));

    if let Some(url) = &message.thumbnail_url {
        // Section with thumbnail and text
        container = container.component(
            SectionBuilder::new(
                ThumbnailBuilder::new(UnfurledMediaItem {
                    url: url.to_string(),
                    proxy_url: None,
                    height: None,
                    width: None,
                    content_type: None,
                })
                .build(),
            )
            .component(TextDisplayBuilder::new("# New Request").build())
            .component(TextDisplayBuilder::new(&description_with_mention).build())
            .build(),
        );
    } else {
        container = container
            .component(TextDisplayBuilder::new("# New Request").build())
            .component(SeparatorBuilder::new().build())
            .component(TextDisplayBuilder::new(&description_with_mention).build());
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
    const MAX_RESULTS: usize = 25;
    if results.len() > MAX_RESULTS {
        info!(
            "Truncating {} results to {} for Discord dropdown",
            results.len(),
            MAX_RESULTS
        );
        results.truncate(MAX_RESULTS);
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
    debug!("Waiting for user to select a search result");
    let mut next = match timeout(Duration::from_secs(300), rx.recv()).await {
        Ok(Some(val)) => val,
        Ok(None) => anyhow::bail!("Channel closed unexpectedly"),
        Err(_) => {
            info!("User selection timed out");
            update_timeout(&discord_http, application_id, &token).await?;
            anyhow::bail!("Timed out waiting for user selection");
        }
    };
    trace!(data = ?next, "Got the next interaction");

    // Use the value from this next payload to get the index into the search results to process
    let selection_idx: usize = next
        .data
        .values
        .first()
        .expect("Will always have one")
        .parse()
        .expect("Will always be a valid index");

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
    let mut additional_details = backend.additional_details(&*selection);
    trace!(details = ?additional_details, "Request details");

    // Track which fields are user-selectable (have multiple options)
    // These are the only ones we'll show in the UI
    let user_selectable_fields: std::collections::HashSet<_> = additional_details
        .iter()
        .filter(|detail| detail.options.len() > 1)
        .filter_map(|detail| detail.metadata.as_ref())
        .cloned()
        .collect();

    let display_info = backend.display_info(&*selection);
    let mut request_container = build_request_component(
        uuid,
        &display_info,
        &additional_details,
        &user_selectable_fields,
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
            Ok(None) => anyhow::bail!("Channel closed unexpectedly"),
            Err(_) => {
                info!("User selection timed out");
                update_timeout(&discord_http, application_id, &next.token).await?;
                anyhow::bail!("Timed out waiting for user selection");
            }
        };
        trace!(data = ?next, "Got interaction from additional details");

        // Check if this was the final "Request" button click
        if next.data.custom_id.starts_with("request:") {
            info!("User clicked Request button, all details collected");

            // Acknowledge the button click immediately (before 3-second timeout)
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
                ),
            )
            .await?;

            break;
        }

        // Use this response to process the selection
        let option_idx: usize = next
            .data
            .values
            .first()
            .expect("Will always have one")
            .parse()
            .expect("Will always be a valid index");

        let (title, _) = next
            .data
            .custom_id
            .split_once(":")
            .expect("There will always be two parts to the custom id");

        let detail_idx = additional_details
            .iter()
            .position(|x| x.title == title)
            .expect("There will always be one");

        let selected_option = additional_details[detail_idx].options.remove(option_idx);
        debug!(detail = %title, selected = %selected_option.title, "User selected detail option");

        // Replace the vector of options with the selected option
        additional_details[detail_idx].options = vec![selected_option];

        // Update the component to show the selection
        request_container = build_request_component(
            uuid,
            &display_info,
            &additional_details,
            &user_selectable_fields,
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
    backend.request(additional_details, selection).await?;
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
    if public_followup {
        let success_component = build_success_component(user_id, &success_msg);
        let result = discord_http
            .create_message(channel_id)
            .flags(MessageFlags::IS_COMPONENTS_V2)
            .components(&[success_component])
            .await;

        if let Err(ref e) = result {
            error!("Discord channel message error: {:?}", e);
        }
        result.context("Failed to send channel message")?;
    }

    info!(uuid = %uuid, "Interaction flow completed successfully");
    Ok(())
}
