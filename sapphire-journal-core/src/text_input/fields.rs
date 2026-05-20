use chrono::NaiveDateTime;

use crate::{
    error::{Error, Result},
    period::{parse_datetime, parse_datetime_end},
};

/// Split a comma-separated tag string into a `Vec<String>`, trimming and dropping empties.
/// Returns `None` when the input is `None`, preserving the "leave tags untouched" semantics.
/// Used by MCP where tags arrive as a single string; CLI uses clap's value_delimiter and skips this.
pub fn parse_tags_csv(tags: Option<&str>) -> Option<Vec<String>> {
    tags.map(|s| {
        s.split(',')
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .collect()
    })
}

pub fn parse_optional_datetime(s: Option<&str>) -> Result<Option<NaiveDateTime>> {
    s.map(|s| parse_datetime(s).map_err(Error::InvalidInput)).transpose()
}

pub fn parse_optional_datetime_end(s: Option<&str>) -> Result<Option<NaiveDateTime>> {
    s.map(|s| parse_datetime_end(s).map_err(Error::InvalidInput)).transpose()
}
