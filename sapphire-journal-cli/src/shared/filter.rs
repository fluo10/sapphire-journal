use anyhow::Result;
use sapphire_journal_core::{
    ops::{EntryFilter, FieldSelector, SortField, SortOrder},
    period::parse_period,
};

/// Frontend-neutral inputs for [`build_filter`].
///
/// Both the CLI's clap-derived `EntryFilterArgs` and the MCP's serde-derived
/// `EntryListParams` convert into this struct via `From`, so the actual filter
/// construction lives in one place.
pub struct FilterInputs<'a> {
    pub period: Option<&'a str>,
    pub active: bool,
    pub task_overdue: bool,
    pub task_in_progress: bool,
    pub task_unstarted: bool,
    pub event_span: bool,
    pub created_at: bool,
    pub updated_at: bool,
    pub task_status: &'a [String],
    pub tags: &'a [String],
    pub sort_by: Option<&'a str>,
    pub sort_order: Option<&'a str>,
}

pub fn build_filter(inputs: FilterInputs<'_>) -> Result<EntryFilter> {
    let parse_p = |s: &str| parse_period(s).map_err(anyhow::Error::msg);
    let base = if inputs.active { FieldSelector::active() } else { FieldSelector::default() };
    Ok(EntryFilter {
        period: inputs.period.map(parse_p).transpose()?,
        fields: FieldSelector {
            task_overdue:     base.task_overdue     || inputs.task_overdue,
            task_in_progress: base.task_in_progress || inputs.task_in_progress,
            task_unstarted:   base.task_unstarted   || inputs.task_unstarted,
            event_span:       base.event_span       || inputs.event_span,
            created_at:       base.created_at       || inputs.created_at,
            updated_at:       base.updated_at       || inputs.updated_at,
        },
        task_status: inputs.task_status.to_vec(),
        tags: inputs.tags.to_vec(),
        sort_by: inputs.sort_by
            .map(|s| s.parse::<SortField>().map_err(anyhow::Error::msg))
            .transpose()?
            .unwrap_or_default(),
        sort_order: inputs.sort_order
            .map(|s| s.parse::<SortOrder>().map_err(anyhow::Error::msg))
            .transpose()?
            .unwrap_or_default(),
    })
}
