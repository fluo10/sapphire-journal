//! Obsidian-style right sidebar showing metadata for the active entry.
//!
//! Hosted from `journal_home::show`, which decides whether to render this as
//! a side panel (wide windows) or as a floating overlay (narrow windows).

use eframe::egui;

use sapphire_journal_core::{entry::EntryHeader, labels::EntryFlag};

use crate::app::HomeState;
use crate::icons;
use crate::settings::RightTab;

/// Render the tab bar plus the body of the currently-selected tab.
pub fn draw(home: &mut HomeState, ui: &mut egui::Ui) {
    ui.add_space(4.0);
    draw_tab_bar(home, ui);
    ui.separator();

    match home.right_sidebar_tab {
        RightTab::Metadata => draw_metadata_tab(home, ui),
    }
}

/// Draw the floating overlay variant used when the window is too narrow to
/// fit a full side panel.
pub fn draw_overlay(ctx: &egui::Context, home: &mut HomeState, top_offset: f32) {
    let width = home.right_sidebar_width.clamp(220.0, 360.0);
    egui::Area::new(egui::Id::new("right_sidebar_overlay"))
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, top_offset))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_width(width);
                    ui.set_max_height(ctx.content_rect().height() - top_offset - 16.0);
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            draw(home, ui);
                        });
                });
        });
}

fn draw_tab_bar(home: &mut HomeState, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        for (tab, label) in [(RightTab::Metadata, "Metadata")] {
            if ui
                .selectable_label(home.right_sidebar_tab == tab, label)
                .clicked()
            {
                home.right_sidebar_tab = tab;
            }
        }
    });
}

fn draw_metadata_tab(home: &HomeState, ui: &mut egui::Ui) {
    let Some(header) = home.active_header() else {
        ui.add_space(8.0);
        ui.weak("No entry selected");
        return;
    };

    ui.add_space(4.0);
    draw_title(header, ui);
    ui.add_space(8.0);

    draw_field(ui, "ID", |ui| {
        ui.monospace(header.frontmatter.id.to_string());
    });
    draw_field(ui, "Slug", |ui| {
        ui.monospace(&header.frontmatter.slug);
    });
    draw_field(ui, "Created", |ui| {
        ui.label(format_dt(header.frontmatter.created_at));
    });
    draw_field(ui, "Updated", |ui| {
        ui.label(format_dt(header.frontmatter.updated_at));
    });

    ui.add_space(4.0);
    draw_tags(header, ui);

    ui.add_space(4.0);
    draw_flags(header, ui);
}

fn draw_title(header: &EntryHeader, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new(&header.frontmatter.title).strong().size(15.0));
}

fn draw_field<F>(ui: &mut egui::Ui, label: &str, value: F)
where
    F: FnOnce(&mut egui::Ui),
{
    ui.horizontal(|ui| {
        let label_color = ui.visuals().weak_text_color();
        ui.label(egui::RichText::new(format!("{label}:")).color(label_color));
        value(ui);
    });
}

fn draw_tags(header: &EntryHeader, ui: &mut egui::Ui) {
    let label_color = ui.visuals().weak_text_color();
    ui.label(egui::RichText::new("Tags:").color(label_color));
    if header.frontmatter.tags.is_empty() {
        ui.weak("(none)");
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for tag in &header.frontmatter.tags {
            let _ = ui.small_button(tag);
        }
    });
}

fn draw_flags(header: &EntryHeader, ui: &mut egui::Ui) {
    let label_color = ui.visuals().weak_text_color();
    ui.label(egui::RichText::new("Flags:").color(label_color));
    if header.flags.is_empty() {
        ui.weak("(none)");
        return;
    }
    let tint = ui.visuals().text_color();
    ui.horizontal_wrapped(|ui| {
        for flag in &header.flags {
            ui.add(
                egui::Image::new(icons::flag_icon(*flag))
                    .fit_to_exact_size(egui::vec2(14.0, 14.0))
                    .tint(tint),
            );
            ui.label(flag_label(*flag));
            ui.add_space(4.0);
        }
    });
}

fn flag_label(flag: EntryFlag) -> &'static str {
    match flag {
        EntryFlag::Overdue => "Overdue",
        EntryFlag::New => "New",
        EntryFlag::Updated => "Updated",
        EntryFlag::Event => "Event",
        EntryFlag::EventClosed => "Event closed",
        EntryFlag::Done => "Done",
        EntryFlag::Cancelled => "Cancelled",
        EntryFlag::InProgress => "In progress",
        EntryFlag::Archived => "Archived",
        EntryFlag::Open => "Open",
        EntryFlag::Note => "Note",
    }
}

fn format_dt(dt: chrono::NaiveDateTime) -> String {
    dt.format("%Y-%m-%d %H:%M").to_string()
}
