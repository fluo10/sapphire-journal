// SPDX-License-Identifier: GPL-3.0-or-later

use eframe::egui;

mod app;
mod dialogs;
mod error;
mod fonts;
mod icons;
mod registry;
mod screens;
mod settings;

use app::App;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 600.0])
            .with_min_inner_size([600.0, 400.0])
            .with_title("Sapphire Journal"),
        ..Default::default()
    };

    eframe::run_native(
        "Sapphire Journal",
        options,
        Box::new(|cc| {
            fonts::install(&cc.egui_ctx);
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(App::new()))
        }),
    )
}
