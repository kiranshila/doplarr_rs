use std::error;
use std::fmt;

#[derive(Debug, Clone)]
pub struct ResponseContent<T> {
    pub status: reqwest::StatusCode,
    pub content: String,
    pub entity: Option<T>,
}

#[derive(Debug)]
pub enum Error<T> {
    Reqwest(reqwest::Error),
    Serde(serde_json::Error),
    Io(std::io::Error),
    ResponseError(ResponseContent<T>),
}

impl<T> fmt::Display for Error<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (module, e) = match self {
            Error::Reqwest(e) => ("reqwest", e.to_string()),
            Error::Serde(e) => ("serde", e.to_string()),
            Error::Io(e) => ("IO", e.to_string()),
            Error::ResponseError(e) => ("response", format!("status code {}", e.status)),
        };
        write!(f, "error in {}: {}", module, e)
    }
}

impl<T: fmt::Debug> error::Error for Error<T> {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        Some(match self {
            Error::Reqwest(e) => e,
            Error::Serde(e) => e,
            Error::Io(e) => e,
            Error::ResponseError(_) => return None,
        })
    }
}

impl<T> From<reqwest::Error> for Error<T> {
    fn from(e: reqwest::Error) -> Self {
        Error::Reqwest(e)
    }
}

impl<T> From<serde_json::Error> for Error<T> {
    fn from(e: serde_json::Error) -> Self {
        Error::Serde(e)
    }
}

impl<T> From<std::io::Error> for Error<T> {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub fn urlencode<T: AsRef<str>>(s: T) -> String {
    ::url::form_urlencoded::byte_serialize(s.as_ref().as_bytes()).collect()
}

pub fn parse_deep_object(prefix: &str, value: &serde_json::Value) -> Vec<(String, String)> {
    if let serde_json::Value::Object(object) = value {
        let mut params = vec![];

        for (key, value) in object {
            match value {
                serde_json::Value::Object(_) => params.append(&mut parse_deep_object(
                    &format!("{}[{}]", prefix, key),
                    value,
                )),
                serde_json::Value::Array(array) => {
                    for (i, value) in array.iter().enumerate() {
                        params.append(&mut parse_deep_object(
                            &format!("{}[{}][{}]", prefix, key, i),
                            value,
                        ));
                    }
                }
                serde_json::Value::String(s) => {
                    params.push((format!("{}[{}]", prefix, key), s.clone()))
                }
                _ => params.push((format!("{}[{}]", prefix, key), value.to_string())),
            }
        }

        return params;
    }

    unimplemented!("Only objects are supported with style=deepObject")
}

/// Internal use only
/// A content type supported by this client.
#[allow(dead_code)]
enum ContentType {
    Json,
    Text,
    Unsupported(String),
}

impl From<&str> for ContentType {
    fn from(content_type: &str) -> Self {
        if content_type.starts_with("application") && content_type.contains("json") {
            return Self::Json;
        } else if content_type.starts_with("text/plain") {
            return Self::Text;
        } else {
            return Self::Unsupported(content_type.to_string());
        }
    }
}

pub mod alternative_title_api;
pub mod api_info_api;
pub mod authentication_api;
pub mod auto_tagging_api;
pub mod backup_api;
pub mod blocklist_api;
pub mod calendar_api;
pub mod calendar_feed_api;
pub mod collection_api;
pub mod command_api;
pub mod credit_api;
pub mod custom_filter_api;
pub mod custom_format_api;
pub mod cutoff_api;
pub mod delay_profile_api;
pub mod disk_space_api;
pub mod download_client_api;
pub mod download_client_config_api;
pub mod extra_file_api;
pub mod file_system_api;
pub mod health_api;
pub mod history_api;
pub mod host_config_api;
pub mod import_list_api;
pub mod import_list_config_api;
pub mod import_list_exclusion_api;
pub mod import_list_movies_api;
pub mod indexer_api;
pub mod indexer_config_api;
pub mod indexer_flag_api;
pub mod language_api;
pub mod localization_api;
pub mod log_api;
pub mod log_file_api;
pub mod manual_import_api;
pub mod media_cover_api;
pub mod media_management_config_api;
pub mod metadata_api;
pub mod metadata_config_api;
pub mod missing_api;
pub mod movie_api;
pub mod movie_editor_api;
pub mod movie_file_api;
pub mod movie_folder_api;
pub mod movie_import_api;
pub mod movie_lookup_api;
pub mod naming_config_api;
pub mod notification_api;
pub mod parse_api;
pub mod ping_api;
pub mod quality_definition_api;
pub mod quality_profile_api;
pub mod quality_profile_schema_api;
pub mod queue_action_api;
pub mod queue_api;
pub mod queue_details_api;
pub mod queue_status_api;
pub mod release_api;
pub mod release_profile_api;
pub mod release_push_api;
pub mod remote_path_mapping_api;
pub mod rename_movie_api;
pub mod root_folder_api;
pub mod static_resource_api;
pub mod system_api;
pub mod tag_api;
pub mod tag_details_api;
pub mod task_api;
pub mod ui_config_api;
pub mod update_api;
pub mod update_log_file_api;

pub mod configuration;
