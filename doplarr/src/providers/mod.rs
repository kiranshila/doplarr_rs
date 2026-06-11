//! Traits that the various backends will implement to support search and requests
//!
//! A generic provider does four things:
//! 1. Perform searches
//! 2. Determines if a selected search result is already available or has been requested before
//! 3. Provides a set of additional information needed to complete the request (quality profile, season, etc)
//! 4. Perform the request using the payload and the set of additional information and respond with a success or failure
use anyhow::Result;
use async_trait::async_trait;
use std::{any::Any, fmt::Debug};

#[derive(Debug)]
pub struct UserFacingError(pub String);

impl std::fmt::Display for UserFacingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UserFacingError {}

// Shared utilities
mod api_logging;

// Backend instances
pub mod radarr;
pub mod seerr;
pub mod sonarr;

/// Represents the different ways we can capture a unique id for a menu selection
/// Some objects in the backends have unique integer ids, while some are just string sentinel values
#[derive(Debug, Clone)]
pub enum SelectableId {
    Integer(i32),
    String(String),
    Boolean(bool),
}

#[derive(Debug, Clone, Default)]
pub struct DropdownOption {
    /// Main dropdown description
    pub title: String,
    /// Subtitle in the dropdown
    pub description: Option<String>,
    /// Backend-specific id
    pub id: Option<SelectableId>,
}

/// Type of field for the request detail
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    /// Enum/list selection
    Dropdown,
    /// Boolean yes/no selection
    Boolean,
}

#[derive(Debug, Clone)]
/// Additional details needed to complete a request
pub struct RequestDetails {
    /// Title to present to the user for this collection of options
    pub title: String,
    /// Options to select
    pub options: Vec<DropdownOption>,
    /// Backend-specific metadata
    pub metadata: Option<String>,
    /// Type of field
    pub field_type: FieldType,
    /// Show this field even when only a single option remains - single-option
    /// fields are otherwise hidden, as they represent admin-configured defaults
    pub always_show: bool,
}

/// Represents the media selection box as presented by discord
pub struct MediaDisplayInfo {
    pub title: String,
    pub subtitle: Option<String>,
    pub description: Option<String>,
    pub thumbnail_url: Option<String>,
}

/// Represents the success block shown by discord
pub struct SuccessMessage {
    /// Short one-liner identifying what was requested, e.g. "Title (Year) (Season 2)"
    /// Used as the heading and as OS notification content
    pub summary: String,
    pub description: String,
    pub thumbnail_url: Option<String>,
}

// Trait that all media types must implement
pub trait MediaItem: Send + Sync + Debug {
    fn to_dropdown(&self) -> DropdownOption;

    fn as_any(&self) -> &dyn Any;

    fn into_any(self: Box<Self>) -> Box<dyn Any>;
}

#[async_trait]
pub trait MediaBackend: Send + Sync {
    /// Given a search term, return a vector of things that can be converted into Discord's `SelectMenuOption`
    async fn search(&self, term: &str) -> Result<Vec<Box<dyn MediaItem>>>;

    /// Given a search results payload, determine if we should stop the interaction flow early
    /// Not all providers will be able to do this with the payload alone, but this needs to not require a backend request
    fn early_stop(&self, media: &dyn MediaItem) -> bool;

    /// Return the media display info
    fn display_info(&self, media: &dyn MediaItem) -> MediaDisplayInfo;

    /// Return the additional details we want to collect in order to complete a request
    async fn additional_details(&self, media: &dyn MediaItem) -> Result<Vec<RequestDetails>>;

    /// Perform the request with the backend, using the information gathered
    /// from the media search result and the additional details
    async fn request(
        &self,
        details: Vec<RequestDetails>,
        media: Box<dyn MediaItem>,
        requester_discord_id: u64,
    ) -> Result<()>;

    /// Build the success message including details about what was requested
    fn success_message(&self, details: &[RequestDetails], media: &dyn MediaItem) -> SuccessMessage;
}
