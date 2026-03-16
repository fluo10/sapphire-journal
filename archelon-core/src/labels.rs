//! Status flag classification for entry types and freshness.
//!
//! This module computes machine-readable flags for an entry based on its
//! frontmatter (task status, event presence, timestamps). Display rendering
//! (emoji, nerd-font glyphs, initials) is handled via [`EntryFlag`] methods.

use chrono::{Duration, Local, NaiveDateTime};

use crate::entry::{EventMetaView, TaskMetaView};

/// A computed flag describing an entry's type or freshness state.
///
/// Serializes to its string representation via [`EntryFlag::as_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryFlag {
    // Freshness / urgency (slot 1)
    Overdue,
    New,
    Updated,
    // Entry type (slot 2)
    Event,
    /// Past event whose `end` timestamp is before the current time.
    EventClosed,
    Done,
    Cancelled,
    InProgress,
    Archived,
    Open,
    Note,
}

impl EntryFlag {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Overdue     => "overdue",
            Self::New         => "new",
            Self::Updated     => "updated",
            Self::Event       => "event",
            Self::EventClosed => "event_closed",
            Self::Done        => "done",
            Self::Cancelled   => "cancelled",
            Self::InProgress  => "in_progress",
            Self::Archived    => "archived",
            Self::Open        => "open",
            Self::Note        => "note",
        }
    }

    pub fn to_emoji(self) -> &'static str {
        match self {
            Self::Overdue     => "⏰",
            Self::New         => "🆕",
            Self::Updated     => "✏️",
            Self::Event       => "📅",
            Self::EventClosed => "🗓️",
            Self::Done        => "✅",
            Self::Cancelled   => "❌",
            Self::InProgress  => "🔄",
            Self::Archived    => "📦",
            Self::Open        => "⬜",
            Self::Note        => "📝",
        }
    }

    pub fn to_nerd(self) -> &'static str {
        match self {
            Self::Overdue     => "󱦟",
            Self::New         => "󰐕",
            Self::Updated     => "󰏫",
            Self::Event       => "󰃭",
            Self::EventClosed => "󰄻",
            Self::Done        => "󰄲",
            Self::Cancelled   => "󰜺",
            Self::InProgress  => "󰔛",
            Self::Archived    => "󰀼",
            Self::Open        => "󰄱",
            Self::Note        => "󰈙",
        }
    }

    pub fn to_initial(self) -> char {
        match self {
            Self::Overdue     => '!',
            Self::New         => '+',
            Self::Updated     => '~',
            Self::Event       => 'E',
            Self::EventClosed => 'e',
            Self::Done        => 'D',
            Self::Cancelled   => 'C',
            Self::InProgress  => 'I',
            Self::Archived    => 'A',
            Self::Open        => 'O',
            Self::Note        => 'N',
        }
    }
}

impl serde::Serialize for EntryFlag {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// Returns the canonical flag string for a task status string.
///
/// Conventional statuses: `open`, `in_progress`, `done`, `cancelled`, `archived`.
/// Any unrecognised status is treated as `open`.
pub fn task_status_label(status: &str) -> &'static str {
    match status {
        "done" | "completed"     => "done",
        "cancelled" | "canceled" => "cancelled",
        "in_progress" | "wip"   => "in_progress",
        "archived"               => "archived",
        _                        => "open",
    }
}

/// Returns the computed [`EntryFlag`]s for an entry.
///
/// Slot 1 (urgency/freshness): `Overdue`, `New` (created <24 h), `Updated` (<24 h), absent otherwise.
/// Slot 2 (entry type): `Event` / `EventClosed` (past event), task status flag, or `Note`.
pub fn entry_flags(
    task: Option<&TaskMetaView>,
    event: Option<&EventMetaView>,
    created_at: NaiveDateTime,
    updated_at: NaiveDateTime,
) -> Vec<EntryFlag> {
    let mut flags = Vec::new();

    let now = Local::now().naive_local();

    // Slot 1: overdue (highest priority) > created <24h > updated <24h
    let is_overdue = task.map_or(false, |t| {
        t.due.map_or(false, |due| due < now) && t.closed_at.is_none()
    });
    if is_overdue {
        flags.push(EntryFlag::Overdue);
    } else {
        let threshold = now - Duration::hours(24);
        if created_at >= threshold {
            flags.push(EntryFlag::New);
        } else if updated_at >= threshold {
            flags.push(EntryFlag::Updated);
        }
    }

    // Slot 2: entry type
    if let Some(ev) = event {
        if ev.end < now {
            flags.push(EntryFlag::EventClosed);
        } else {
            flags.push(EntryFlag::Event);
        }
    } else if let Some(task) = task {
        let flag = match task_status_label(&task.status) {
            "done"        => EntryFlag::Done,
            "cancelled"   => EntryFlag::Cancelled,
            "in_progress" => EntryFlag::InProgress,
            "archived"    => EntryFlag::Archived,
            _             => EntryFlag::Open,
        };
        flags.push(flag);
    } else {
        flags.push(EntryFlag::Note);
    }

    flags
}
