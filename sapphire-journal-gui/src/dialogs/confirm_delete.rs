use eframe::egui;

use crate::app::{App, AppEvent, DialogState};

pub fn show(app: &mut App, ctx: &egui::Context) {
    let mut close = false;
    let mut submit = false;

    let DialogState::ConfirmDelete(state) = app.dialog.as_mut().unwrap() else {
        return;
    };

    let expected = state.entry.name.clone();
    let entry_id = state.entry.id;
    let storage_path = state.entry.storage_path.clone();

    let mut open = true;
    egui::Window::new("Delete Journal")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_min_width(360.0);

            ui.colored_label(
                egui::Color32::LIGHT_RED,
                "This will permanently delete the journal and all its entries.",
            );
            ui.label("This action cannot be undone.");
            ui.add_space(8.0);

            ui.horizontal_wrapped(|ui| {
                ui.label("Type");
                ui.strong(&expected);
                ui.label("to confirm:");
            });
            let resp = ui.add(
                egui::TextEdit::singleline(&mut state.typed)
                    .hint_text(&expected)
                    .desired_width(f32::INFINITY),
            );
            if !state.in_progress {
                resp.request_focus();
            }

            let name_matches = state.typed.trim() == expected.as_str();
            let can_delete = name_matches && !state.in_progress;

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if state.in_progress {
                        "Deleting…"
                    } else {
                        "Delete Journal"
                    };
                    if ui
                        .add_enabled(can_delete, egui::Button::new(label))
                        .clicked()
                    {
                        submit = true;
                    }
                    if ui
                        .add_enabled(!state.in_progress, egui::Button::new("Cancel"))
                        .clicked()
                    {
                        close = true;
                    }
                });
            });
        });

    if !open && !state.in_progress {
        close = true;
    }

    if submit {
        state.in_progress = true;
        let tx = app.event_tx.clone();
        app.runtime.spawn(async move {
            let storage = storage_path.clone();
            let result =
                tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&storage)).await;
            match result {
                Ok(Ok(())) => {
                    let _ = tx.send(AppEvent::JournalRemoved(entry_id));
                }
                Ok(Err(e)) => {
                    let _ = tx.send(AppEvent::Error(e.to_string()));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(e.to_string()));
                }
            }
        });
    }

    if close {
        app.dialog = None;
    }
}
