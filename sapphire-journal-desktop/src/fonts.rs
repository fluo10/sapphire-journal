//! Font setup for the GUI.
//!
//! Currently this module loads a single bundled Japanese font (Noto Sans JP)
//! and registers it as a fallback for the proportional and monospace
//! families.  Latin text continues to be rendered by egui's built-in fonts;
//! Noto Sans JP fills in CJK glyphs that would otherwise render as tofu.
//!
//! ## Future work
//!
//! The intent is to let the user pick a font (including OS-installed fonts,
//! e.g. via `font-kit`) for accessibility — dyslexia-friendly fonts and
//! similar.  The public API of this module is shaped to make that change
//! additive:
//!
//! - [`install`] is the single entry point invoked from `main.rs`.
//! - Internal helpers [`base_definitions`] and [`add_bundled_jp`] produce a
//!   `FontDefinitions` step-by-step, so additional sources (OS, user-chosen
//!   file path, settings) can be layered on top without touching callers.

use std::sync::Arc;

use eframe::egui;

/// Bundled Japanese fallback font: Noto Sans JP Regular (SIL OFL 1.1).
///
/// See `assets/fonts/LICENSE-NotoSansJP` for the full license.
const NOTO_SANS_JP: &[u8] =
    include_bytes!("../assets/fonts/NotoSansJP-Regular.otf");

const BUNDLED_JP_KEY: &str = "noto_sans_jp";

/// Configure the global egui font set on `ctx`.
///
/// Call this once at startup from the eframe creation callback.
pub fn install(ctx: &egui::Context) {
    let mut fonts = base_definitions();
    add_bundled_jp(&mut fonts);
    ctx.set_fonts(fonts);
}

/// Start from egui's defaults so Latin text keeps its built-in fonts.
fn base_definitions() -> egui::FontDefinitions {
    egui::FontDefinitions::default()
}

/// Register the bundled Noto Sans JP face as a *fallback* for both font
/// families.  Latin glyphs still resolve through egui's defaults first;
/// CJK glyphs that the defaults lack fall through to Noto Sans JP.
fn add_bundled_jp(fonts: &mut egui::FontDefinitions) {
    fonts.font_data.insert(
        BUNDLED_JP_KEY.to_owned(),
        Arc::new(egui::FontData::from_static(NOTO_SANS_JP)),
    );

    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .push(BUNDLED_JP_KEY.to_owned());

    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push(BUNDLED_JP_KEY.to_owned());
}
