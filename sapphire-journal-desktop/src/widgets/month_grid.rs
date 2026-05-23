//! Reusable month-grid calendar widget.
//!
//! Used by the home screen's period filter (with the ISO week column and
//! per-day entry-count dots) and by the date pickers in the entry editor
//! (week column hidden, no badges).

use chrono::{Datelike, Duration, NaiveDate};
use eframe::egui;

/// Highlighted range on the grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Selection {
    #[default]
    None,
    /// Inclusive [start, end] range — used for week / month / explicit ranges.
    Range(NaiveDate, NaiveDate),
}

impl Selection {
    fn contains(&self, d: NaiveDate) -> bool {
        match self {
            Selection::None => false,
            Selection::Range(s, e) => d >= *s && d <= *e,
        }
    }
}

/// One day's worth of UI extras: a click-count badge.
pub type DayBadge<'a> = &'a dyn Fn(NaiveDate) -> usize;

pub struct MonthGrid<'a> {
    id_salt: &'a str,
    /// Displayed month (driven by the prev/next arrows). The caller stores
    /// this between frames; the widget mutates it when the user navigates.
    cursor: &'a mut NaiveDate,
    selection: Selection,
    show_week_column: bool,
    /// Optional closure returning the entry count for a given date. `0` →
    /// no badge.
    day_badge: Option<DayBadge<'a>>,
    cell_size: f32,
}

impl<'a> MonthGrid<'a> {
    pub fn new(id_salt: &'a str, cursor: &'a mut NaiveDate) -> Self {
        Self {
            id_salt,
            cursor,
            selection: Selection::None,
            show_week_column: false,
            day_badge: None,
            cell_size: 28.0,
        }
    }

    pub fn selection(mut self, sel: Selection) -> Self {
        self.selection = sel;
        self
    }

    pub fn show_week_column(mut self, show: bool) -> Self {
        self.show_week_column = show;
        self
    }

    pub fn day_badge(mut self, badge: DayBadge<'a>) -> Self {
        self.day_badge = Some(badge);
        self
    }

    pub fn cell_size(mut self, size: f32) -> Self {
        self.cell_size = size;
        self
    }
}

// `MonthGridResponse` is currently used by callers via `MonthGrid::show`'s
// return value rather than by name; re-exported through `mod.rs`.

#[derive(Default, Debug)]
pub struct MonthGridResponse {
    pub day_clicked: Option<NaiveDate>,
    /// Monday of the clicked ISO week.
    pub week_clicked: Option<NaiveDate>,
    /// (year, month) of the clicked month header.
    pub month_clicked: Option<(i32, u32)>,
    /// True when the user clicked the "Today" shortcut.
    pub today_clicked: bool,
    /// True when the user clicked the "Clear" shortcut.
    pub clear_clicked: bool,
}

impl<'a> MonthGrid<'a> {
    pub fn show(self, ui: &mut egui::Ui) -> MonthGridResponse {
        let MonthGrid {
            id_salt,
            cursor,
            selection,
            show_week_column,
            day_badge,
            cell_size,
        } = self;

        let mut resp = MonthGridResponse::default();
        let today = chrono::Local::now().date_naive();

        // ── Header bar: month nav + Today + Clear ───────────────────────
        ui.horizontal(|ui| {
            if ui.small_button("◀").on_hover_text("Previous month").clicked() {
                *cursor = add_months(*cursor, -1);
            }
            let label = format!("{} {}", month_name(cursor.month()), cursor.year());
            let month_btn = ui
                .add(egui::Button::new(egui::RichText::new(label).strong()).frame(false))
                .on_hover_text("Filter by this month");
            if month_btn.clicked() {
                resp.month_clicked = Some((cursor.year(), cursor.month()));
            }
            if ui.small_button("▶").on_hover_text("Next month").clicked() {
                *cursor = add_months(*cursor, 1);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Clear").on_hover_text("Show all entries").clicked() {
                    resp.clear_clicked = true;
                }
                if ui.small_button("Today").on_hover_text("Filter by today").clicked() {
                    *cursor = today;
                    resp.today_clicked = true;
                }
            });
        });

        // ── Day-of-week header ──────────────────────────────────────────
        ui.add_space(2.0);
        egui::Grid::new(format!("{id_salt}_dow"))
            .num_columns(if show_week_column { 8 } else { 7 })
            .min_col_width(cell_size)
            .max_col_width(cell_size)
            .spacing([2.0, 2.0])
            .show(ui, |ui| {
                if show_week_column {
                    ui.weak("W");
                }
                for d in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"] {
                    ui.weak(d);
                }
                ui.end_row();
            });

        // ── 6 × 7 day grid ──────────────────────────────────────────────
        let first_of_month = NaiveDate::from_ymd_opt(cursor.year(), cursor.month(), 1).unwrap();
        // Walk back to the Monday at-or-before the 1st.
        let lead = first_of_month.weekday().num_days_from_monday() as i64;
        let grid_start = first_of_month - Duration::days(lead);

        egui::Grid::new(format!("{id_salt}_grid"))
            .num_columns(if show_week_column { 8 } else { 7 })
            .min_col_width(cell_size)
            .max_col_width(cell_size)
            .spacing([2.0, 2.0])
            .show(ui, |ui| {
                for week in 0..6 {
                    let week_monday = grid_start + Duration::days(week * 7);

                    if show_week_column {
                        let iso = week_monday.iso_week();
                        let in_sel = (0..7).any(|i| {
                            selection.contains(week_monday + Duration::days(i))
                        });
                        let txt = egui::RichText::new(format!("{:02}", iso.week())).weak();
                        let btn = egui::Button::new(txt)
                            .frame(false)
                            .min_size(egui::vec2(cell_size, cell_size))
                            .selected(in_sel);
                        if ui.add(btn).on_hover_text("Filter by this week").clicked() {
                            resp.week_clicked = Some(week_monday);
                        }
                    }

                    for dow in 0..7 {
                        let date = week_monday + Duration::days(dow);
                        let in_current_month = date.month() == cursor.month();
                        let is_today = date == today;
                        let selected = selection.contains(date);

                        let mut rt = egui::RichText::new(format!("{}", date.day()));
                        if !in_current_month {
                            rt = rt.weak();
                        }
                        if is_today {
                            rt = rt.strong().underline();
                        }

                        let btn = egui::Button::new(rt)
                            .min_size(egui::vec2(cell_size, cell_size))
                            .selected(selected)
                            .frame(in_current_month);
                        let r = ui.add(btn);
                        if r.clicked() {
                            resp.day_clicked = Some(date);
                        }

                        // Entry-count dot (drawn over the button rect).
                        if let Some(badge) = day_badge {
                            let count = badge(date);
                            if count > 0 {
                                let painter = ui.painter_at(r.rect);
                                let center = egui::pos2(
                                    r.rect.right() - 4.0,
                                    r.rect.bottom() - 4.0,
                                );
                                let color = if selected {
                                    ui.visuals().selection.stroke.color
                                } else {
                                    ui.visuals().widgets.active.fg_stroke.color
                                };
                                painter.circle_filled(center, 2.5, color);
                            }
                        }
                    }
                    ui.end_row();
                }
            });

        resp
    }
}

fn month_name(m: u32) -> &'static str {
    match m {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "?",
    }
}

/// Move `d` by `n` calendar months. Clamps the day to the last valid day
/// of the target month (e.g. Jan 31 + 1mo → Feb 28/29).
fn add_months(d: NaiveDate, n: i32) -> NaiveDate {
    let total = d.year() * 12 + (d.month() as i32 - 1) + n;
    let year = total.div_euclid(12);
    let month = (total.rem_euclid(12) + 1) as u32;
    let last = last_day_of_month(year, month);
    NaiveDate::from_ymd_opt(year, month, d.day().min(last)).unwrap()
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 { (year + 1, 1u32) } else { (year, month + 1) };
    let first_next = NaiveDate::from_ymd_opt(ny, nm, 1).unwrap();
    (first_next - Duration::days(1)).day()
}

/// First-and-last-day pair for the given month — convenient for callers
/// constructing a [`Selection::Range`] from a `(year, month)` click.
pub fn month_bounds(year: i32, month: u32) -> (NaiveDate, NaiveDate) {
    let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    let last_day = last_day_of_month(year, month);
    let last = NaiveDate::from_ymd_opt(year, month, last_day).unwrap();
    (first, last)
}

/// ISO Monday → ISO Sunday range — convenient for callers responding to
/// [`MonthGridResponse::week_clicked`].
pub fn week_bounds(monday: NaiveDate) -> (NaiveDate, NaiveDate) {
    (monday, monday + Duration::days(6))
}

/// `YYYY-MM-DD` → `YYYY-MM-DD` formatter used for period strings.
pub fn format_period_range(start: NaiveDate, end: NaiveDate) -> String {
    if start == end {
        start.format("%Y-%m-%d").to_string()
    } else {
        format!("{}/{}", start.format("%Y-%m-%d"), end.format("%Y-%m-%d"))
    }
}

/// Reverse of [`format_period_range`] — derive the highlight range from
/// a `Period` parsed from the home state. Returns `None` when the period
/// is `Period::None` or empty.
pub fn selection_from_period(
    period: &sapphire_journal_core::period::Period,
) -> Selection {
    use sapphire_journal_core::period::Period;
    match period {
        Period::None => Selection::None,
        Period::Range(s, e) => Selection::Range(s.date(), e.date()),
    }
}
