use std::sync::{Arc, Mutex};

use eframe::egui;

use crate::app::{
    App, AppState, CloneState, ConfirmDeleteState, DialogKind, DialogState, NewJournalState,
};
use crate::dialogs;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::top("list_header").show_inside(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.heading("Sapphire Journal");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Clone").clicked() && app.dialog.is_none() {
                    app.dialog = Some(DialogState::Clone(CloneState {
                        name: String::new(),
                        url: String::new(),
                        progress: Arc::new(Mutex::new(None)),
                    }));
                }
                if ui.button("New Journal").clicked() && app.dialog.is_none() {
                    app.dialog = Some(DialogState::NewJournal(NewJournalState {
                        name: String::new(),
                        in_progress: false,
                    }));
                }
            });
        });
        ui.add_space(4.0);
    });

    if let Some(msg) = app.error_msg.clone() {
        egui::Panel::top("error_banner").show_inside(ui, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.colored_label(egui::Color32::LIGHT_RED, msg);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("×").clicked() {
                        app.error_msg = None;
                    }
                });
            });
            ui.add_space(2.0);
        });
    }

    egui::CentralPanel::default().show_inside(ui, |ui| {
        let journals = app.registry.journals.clone();

        if journals.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label("No journals yet.");
                ui.label("Create a new journal or clone an existing one to get started.");
            });
        } else {
            egui::ScrollArea::vertical().show(ui, |ui| {
                for entry in journals {
                    let reachable = entry.storage_path.join(".sapphire-journal").is_dir();
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.strong(&entry.name);
                                    if !reachable {
                                        ui.colored_label(
                                            egui::Color32::YELLOW,
                                            "unreachable",
                                        );
                                    }
                                });
                                ui.small(entry.storage_path.display().to_string());
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("Delete").clicked() && app.dialog.is_none() {
                                        app.dialog = Some(DialogState::ConfirmDelete(
                                            ConfirmDeleteState {
                                                entry: entry.clone(),
                                                typed: String::new(),
                                                in_progress: false,
                                            },
                                        ));
                                    }
                                    let open = ui
                                        .add_enabled(reachable, egui::Button::new("Open"))
                                        .clicked();
                                    if open {
                                        app.screen = AppState::Home {
                                            journal_id: entry.id,
                                        };
                                    }
                                },
                            );
                        });
                    });
                    ui.add_space(4.0);
                }
            });
        }
    });

    // Render the active dialog (if any) on top of the screen.
    let dialog_kind = app.dialog.as_ref().map(DialogState::kind);
    let ctx = ui.ctx().clone();
    match dialog_kind {
        Some(DialogKind::NewJournal) => dialogs::new_journal::show(app, &ctx),
        Some(DialogKind::Clone) => dialogs::clone::show(app, &ctx),
        Some(DialogKind::ConfirmDelete) => dialogs::confirm_delete::show(app, &ctx),
        None => {}
    }
}
