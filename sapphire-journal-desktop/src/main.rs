// SPDX-License-Identifier: GPL-3.0-or-later

use eframe::egui;

mod app;
mod dialogs;
mod error;
mod fonts;
mod gpu;
mod icons;
mod registry;
mod screens;
mod settings;
mod widgets;

use app::App;

fn main() -> eframe::Result<()> {
    gpu::init_logging();
    gpu::install_panic_hook();
    sapphire_journal_core::init_app_context();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 600.0])
            .with_min_inner_size([600.0, 400.0])
            .with_title("Sapphire Journal"),
        wgpu_options: gpu::wgpu_options(),
        ..Default::default()
    };

    eframe::run_native(
        "Sapphire Journal",
        options,
        Box::new(|cc| {
            gpu::on_render_state(cc.wgpu_render_state.as_ref());
            fonts::install(&cc.egui_ctx);
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(App::new(cc.egui_ctx.clone())))
        }),
    )
}
