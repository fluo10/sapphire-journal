use eframe::egui;
use uuid::Uuid;

use crate::app::{App, AppState};

pub fn show(app: &mut App, ui: &mut egui::Ui, journal_id: Uuid) {
    egui::Panel::top("home_header").show_inside(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("← Back").clicked() {
                app.screen = AppState::List;
            }
        });
        ui.add_space(4.0);
    });

    egui::CentralPanel::default().show_inside(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.heading("Journal");
            ui.label(format!("journal_id: {journal_id}"));
            ui.label("Entry list coming soon.");
        });
    });
}
