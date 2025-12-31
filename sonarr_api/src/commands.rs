/// Command payloads for Sonarr API
/// Reference: https://github.com/Sonarr/Sonarr/tree/develop/src/NzbDrone.Core/IndexerSearch
use serde::Serialize;

/// Minimal SeriesSearch command payload
/// Reference: https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/IndexerSearch/SeriesSearchCommand.cs
#[derive(Debug, Clone, Serialize)]
pub struct SeriesSearchCommand {
    name: String,
    #[serde(rename = "seriesId")]
    pub series_id: i32,
}

impl SeriesSearchCommand {
    pub fn new(series_id: i32) -> Self {
        Self {
            name: "SeriesSearch".to_string(),
            series_id,
        }
    }
}
