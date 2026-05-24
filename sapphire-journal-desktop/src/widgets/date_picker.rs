//! Calendar-popup date picker for `YYYY-MM-DD` text fields.
//!
//! Renders a small button next to the text input that toggles a popup
//! containing [`MonthGrid`].  Selecting a day overwrites the buffer with
//! the formatted date string.

use chrono::NaiveDate;
use eframe::egui;

use crate::icons;

use super::month_grid::{MonthGrid, Selection};

const DATE_FMT: &str = "%Y-%m-%d";

/// Cursor (displayed month) for an open date-picker popup. Persisted in
/// egui memory so the user's scroll position survives across frames.
#[derive(Clone, Copy)]
struct CursorMem(NaiveDate);

/// Shows a calendar-icon button.  When clicked, opens a popup containing a
/// month grid; clicking a day writes the formatted date into `buf`.
pub fn date_picker_button(
    ui: &mut egui::Ui,
    id_salt: &str,
    buf: &mut String,
) -> egui::Response {
    let icon = egui::Image::new(icons::calendar())
        .fit_to_exact_size(egui::vec2(14.0, 14.0))
        .tint(ui.visuals().text_color());
    let button = ui.add(egui::Button::image(icon).small());
    let popup_id = button.id.with(("date_picker", id_salt));

    egui::Popup::from_toggle_button_response(&button)
        .id(popup_id)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(220.0);

            let parsed = NaiveDate::parse_from_str(buf.trim(), DATE_FMT).ok();
            let today = chrono::Local::now().date_naive();

            // Seed the cursor from the buffer, falling back to today.
            // Subsequent month navigation persists in egui memory.
            let cursor_key = popup_id.with("cursor");
            let mut cursor = ui
                .ctx()
                .data_mut(|d| d.get_temp::<CursorMem>(cursor_key))
                .map(|c| c.0)
                .unwrap_or(parsed.unwrap_or(today));

            let selection = parsed
                .map(|d| Selection::Range(d, d))
                .unwrap_or(Selection::None);

            let resp = MonthGrid::new(id_salt, &mut cursor)
                .selection(selection)
                .show_week_column(false)
                .cell_size(24.0)
                .show(ui);

            ui.ctx()
                .data_mut(|d| d.insert_temp(cursor_key, CursorMem(cursor)));

            let mut close = false;
            if let Some(d) = resp.day_clicked {
                *buf = d.format(DATE_FMT).to_string();
                close = true;
            }
            if resp.today_clicked {
                *buf = today.format(DATE_FMT).to_string();
                close = true;
            }
            if resp.clear_clicked {
                buf.clear();
                close = true;
            }
            // Week / month clicks are ignored — a date picker selects a
            // single day.  Those events are still surfaced by the widget
            // for the home-screen filter use case.

            if close {
                egui::Popup::close_id(ui.ctx(), popup_id);
            }
        });

    button
}
