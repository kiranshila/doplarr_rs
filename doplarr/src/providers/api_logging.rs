/// Shared utilities for logging API errors from generated OpenAPI clients
///
/// Both Sonarr and Radarr use OpenAPI-generated clients with similar error structures.
/// This module provides generic logging for these errors.
use tracing::error;

/// Generic implementation for logging HTTP error responses
/// This works for both sonarr_api::apis::Error and radarr_api::apis::Error
pub fn log_api_error_details(status: reqwest::StatusCode, content: &str, context: &str) {
    error!("{} - HTTP {}: {}", context, status, content);
}
