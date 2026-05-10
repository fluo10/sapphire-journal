use eframe::egui;
use uuid::Uuid;

use crate::app::{App, AppEvent, DialogState};
use crate::registry::{init_journal, JournalRegistry, RegistryEntry};

pub fn show(app: &mut App, ctx: &egui::Context) {
    let mut close = false;
    let mut submit_name: Option<String> = None;

    let DialogState::NewJournal(state) = app.dialog.as_mut().unwrap() else {
        return;
    };

    let mut open = true;
    egui::Window::new("New Journal")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_min_width(360.0);

            ui.label("Journal Name");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut state.name)
                    .hint_text("e.g. My Journal")
                    .desired_width(f32::INFINITY),
            );
            if !state.in_progress {
                resp.request_focus();
            }
            let enter_pressed =
                resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            ui.add_space(8.0);

            let trimmed = state.name.trim().to_string();
            let can_create = !trimmed.is_empty() && !state.in_progress;

            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let create_label = if state.in_progress {
                        "Creating…"
                    } else {
                        "Create"
                    };
                    if ui
                        .add_enabled(can_create, egui::Button::new(create_label))
                        .clicked()
                        || (enter_pressed && can_create)
                    {
                        submit_name = Some(trimmed.clone());
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

    if let Some(name) = submit_name {
        state.in_progress = true;
        let storage_uuid = Uuid::new_v4();
        let storage_path = JournalRegistry::journals_dir().join(storage_uuid.to_string());
        let storage_path_clone = storage_path.clone();
        let tx = app.event_tx.clone();
        app.runtime.spawn(async move {
            let result =
                tokio::task::spawn_blocking(move || init_journal(&storage_path)).await;
            match result {
                Ok(Ok(journal_id)) => {
                    let _ = tx.send(AppEvent::JournalAdded(RegistryEntry {
                        id: journal_id,
                        name,
                        storage_path: storage_path_clone,
                    }));
                }
                Ok(Err(e)) => {
                    let _ = tx.send(AppEvent::CleanupAndError {
                        storage_path: storage_path_clone,
                        error: e.to_string(),
                    });
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::CleanupAndError {
                        storage_path: storage_path_clone,
                        error: e.to_string(),
                    });
                }
            }
        });
    }

    if close {
        app.dialog = None;
    }
}
