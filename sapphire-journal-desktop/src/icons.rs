#![allow(dead_code)]

//! Bundled SVG icons (Lucide, ISC license).
//!
//! All icons are embedded at compile time via [`egui::include_image!`] and
//! decoded by `egui_extras`'s SVG loader (see `main.rs` for the loader
//! installation).  Lucide SVGs use `stroke="currentColor"` so they tint via
//! `egui::Image::tint`.

use eframe::egui;

use sapphire_journal_core::labels::EntryFlag;

macro_rules! icon {
    ($name:literal) => {
        egui::include_image!(concat!("../assets/icons/", $name, ".svg"))
    };
}

// ── Toolbar / chrome ───────────────────────────────────────────────────────

pub fn arrow_left() -> egui::ImageSource<'static> { icon!("arrow-left") }
pub fn calendar() -> egui::ImageSource<'static> { icon!("calendar") }
pub fn chevron_down() -> egui::ImageSource<'static> { icon!("chevron-down") }
pub fn chevron_right() -> egui::ImageSource<'static> { icon!("chevron-right") }
pub fn funnel() -> egui::ImageSource<'static> { icon!("funnel") }
pub fn list_view() -> egui::ImageSource<'static> { icon!("list") }
pub fn tree_view() -> egui::ImageSource<'static> { icon!("folder-tree") }
pub fn panel_right() -> egui::ImageSource<'static> { icon!("panel-right") }
pub fn plus() -> egui::ImageSource<'static> { icon!("plus") }
pub fn refresh() -> egui::ImageSource<'static> { icon!("refresh-cw") }
pub fn trash() -> egui::ImageSource<'static> { icon!("trash-2") }

// ── Entry flags ────────────────────────────────────────────────────────────

/// SVG icon for an [`EntryFlag`].  Replaces the emoji-based glyph that used to
/// render as tofu when no color emoji font was available.
pub fn flag_icon(flag: EntryFlag) -> egui::ImageSource<'static> {
    match flag {
        EntryFlag::Overdue     => icon!("alarm-clock"),
        EntryFlag::New         => icon!("plus"),
        EntryFlag::Updated     => icon!("pencil-line"),
        EntryFlag::Event       => icon!("calendar"),
        EntryFlag::EventClosed => icon!("calendar-check"),
        EntryFlag::Done        => icon!("circle-check"),
        EntryFlag::Cancelled   => icon!("circle-x"),
        EntryFlag::InProgress  => icon!("clock"),
        EntryFlag::Archived    => icon!("archive"),
        EntryFlag::Open        => icon!("circle"),
        EntryFlag::Note        => icon!("file-text"),
    }
}
