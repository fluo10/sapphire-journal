//! Status label classification for entry types and freshness.
//!
//! This module computes machine-readable labels for an entry based on its
//! frontmatter (task status, event presence, timestamps). Display rendering
//! (emoji, nerd-font glyphs, initials) is the responsibility of each frontend.

use chrono::{Duration, Local, NaiveDateTime};

use crate::entry::{EventMeta, TaskMeta};

/// A single status label.
#[derive(Debug, Clone)]
pub struct Symbol {
    pub label: &'static str,
}

/// Returns the canonical label for a task status string.
///
/// Conventional statuses: `open`, `in_progress`, `done`, `cancelled`, `archived`.
/// Any unrecognised status is treated as `open`.
pub fn task_status_label(status: &str) -> &'static str {
    match status {
        "done" | "completed" => "done",
        "cancelled" | "canceled" => "cancelled",
        "in_progress" | "wip" => "in_progress",
        "archived" => "archived",
        _ => "open",
    }
}

/// Returns the status labels for an entry.
///
/// Slot 1 (urgency/freshness): `overdue`, `new` (created <24 h), `updated` (<24 h), absent otherwise.
/// Slot 2 (entry type): `event`, task status label, or `note`.
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
        symbols.push(Symbol { label: "overdue" });
    } else {
        let threshold = now - Duration::hours(24);
        if created_at >= threshold {
            symbols.push(Symbol { label: "new" });
        } else if updated_at >= threshold {
            symbols.push(Symbol { label: "updated" });
        }
    }

    // Slot 2: entry type
    if event.is_some() {
        symbols.push(Symbol { label: "event" });
    } else if let Some(task) = task {
        symbols.push(Symbol { label: task_status_label(&task.status) });
    } else {
        symbols.push(Symbol { label: "note" });
    }

    symbols
}
