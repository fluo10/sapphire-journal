//! Emoji representations for entry types and task statuses.
//!
//! These are the canonical emoji mappings shared across CLI, MCP, and editor integrations.

use chrono::{Duration, Local, NaiveDateTime};

use crate::entry::{EventMeta, TaskMeta};

/// A single visual symbol with a machine-readable label.
#[derive(Debug, Clone)]
pub struct Symbol {
    pub emoji: &'static str,
    pub label: &'static str,
}

/// Returns the (emoji, label) pair for a task status string.
///
/// Conventional statuses: `open`, `in_progress`, `done`, `cancelled`, `archived`.
/// Any unrecognised status is treated as open (⬜).
pub fn task_status_symbol(status: &str) -> (&'static str, &'static str) {
    match status {
        "done" | "completed" => ("✅", "done"),
        "cancelled" | "canceled" => ("❌", "cancelled"),
        "in_progress" | "wip" => ("🔄", "in_progress"),
        "archived" => ("📦", "archived"),
        _ => ("⬜", "open"),
    }
}

/// Returns the emoji that represents a task status string.
///
/// Conventional statuses: `open`, `in_progress`, `done`, `cancelled`, `archived`.
/// Any unrecognised status is treated as open (⬜).
pub fn task_status_emoji(status: &str) -> &'static str {
    task_status_symbol(status).0
}

/// Returns a list of symbols for an entry, used for both text rendering and JSON output.
///
/// Slot 1 (urgency/freshness): `⏰` overdue task, `🆕` created <24 h, `✏️` updated <24 h, absent otherwise.
/// Slot 2 (entry type): `📅` event, task status emoji, or `📝` note.
pub fn entry_symbols(
    task: Option<&TaskMeta>,
    event: Option<&EventMeta>,
    created_at: NaiveDateTime,
    updated_at: NaiveDateTime,
) -> Vec<Symbol> {
    let mut symbols = Vec::new();

    let now = Local::now().naive_local();

    // Slot 1: overdue (highest priority) > created <24h > updated <24h
    let is_overdue = task.map_or(false, |t| {
        t.due.map_or(false, |due| due < now) && t.closed_at.is_none()
    });
    if is_overdue {
        symbols.push(Symbol { emoji: "⏰", label: "overdue" });
    } else {
        let threshold = now - Duration::hours(24);
        if created_at >= threshold {
            symbols.push(Symbol { emoji: "🆕", label: "new" });
        } else if updated_at >= threshold {
            symbols.push(Symbol { emoji: "✏️", label: "updated" });
        }
    }

    // Slot 2: entry type
    if event.is_some() {
        symbols.push(Symbol { emoji: "📅", label: "event" });
    } else if let Some(task) = task {
        let (emoji, label) = task_status_symbol(&task.status);
        symbols.push(Symbol { emoji, label });
    } else {
        symbols.push(Symbol { emoji: "📝", label: "note" });
    }

    symbols
}

/// Returns the display emoji for an entry based on its type (legacy helper, no freshness).
///
/// Priority: event > task > note.
pub fn entry_emoji(task: Option<&TaskMeta>, event: Option<&EventMeta>) -> &'static str {
    if event.is_some() {
        return "📅";
    }
    if let Some(task) = task {
        return task_status_emoji(&task.status);
    }
    "📝"
}

/// Render symbols into a fixed 2-slot string for terminal output.
///
/// Each missing slot is replaced with a fullwidth space (U+3000 `　`).
/// The result is always exactly 2 visual columns wide.
pub fn symbols_text(symbols: &[Symbol]) -> String {
    match symbols.len() {
        0 => "　　".to_owned(),
        1 => format!("　{}", symbols[0].emoji),
        _ => format!("{}{}", symbols[0].emoji, symbols[1].emoji),
    }
}
