use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui;
use sapphire_journal_core::journal::Journal;

use crate::app::{
    App, AppState, CloneState, ConfirmDeleteState, DialogKind, DialogState, NewJournalState,
};
use crate::dialogs;
use crate::registry::{JournalRegistry, RegistryEntry};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::top("list_header").show_inside(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if let Some(prev_id) = app.previous_journal_id {
                if ui.button("← Back").clicked() {
                    app.screen = AppState::Home { journal_id: prev_id };
                    app.previous_journal_id = None;
                    return;
                }
                ui.separator();
            }
            ui.heading("Sapphire Journal");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Clone").clicked() && app.dialog.is_none() {
                    app.dialog = Some(DialogState::Clone(CloneState {
                        name: String::new(),
                        url: String::new(),
                        progress: Arc::new(Mutex::new(None)),
                    }));
                }
                if ui.button("Open Existing…").clicked() && app.dialog.is_none() {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_title("Open Sapphire Journal")
                        .pick_folder()
                    {
                        if let Err(msg) = register_existing(&mut app.registry, path) {
                            app.error_msg = Some(msg);
                        }
                    }
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
                ui.label("Create a new journal, open an existing one, or clone one to get started.");
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
                                        app.previous_journal_id = None;
                                        app.remember_last_opened(Some(entry.id));
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

fn register_existing(registry: &mut JournalRegistry, path: PathBuf) -> Result<(), String> {
    let journal = Journal::from_root(path.clone()).map_err(|_| {
        format!(
            "Not a sapphire journal (missing .sapphire-journal/): {}",
            path.display()
        )
    })?;
    let id = journal.journal_id().map_err(|e| e.to_string())?;

    if let Some(existing) = registry.journals.iter().find(|e| e.id == id) {
        return Err(format!("Already registered as \"{}\".", existing.name));
    }

    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled")
        .to_string();
    registry.add(RegistryEntry {
        id,
        name,
        storage_path: path,
    });
    registry.save().map_err(|e| e.to_string())
}
