use std::path::PathBuf;

use eframe::egui;
use uuid::Uuid;

use sapphire_journal_core::{
    entry::EntryHeader,
    journal::Journal,
    labels::EntryFlag,
    ops::{
        EntryFilter, FieldSelector, SortField as CoreSortField, SortOrder as CoreSortOrder,
        fix_entry, prepare_new_entry, remove_entry,
    },
    parser::{read_entry, write_entry},
    period::parse_period,
};

use crate::app::{App, AppState, EditorState, HomeState};

pub fn show(app: &mut App, ui: &mut egui::Ui, journal_id: Uuid) {
    // ── Ensure HomeState matches the requested journal ────────────────────
    let needs_init = app.home.as_ref().map(|h| h.journal_id) != Some(journal_id);
    if needs_init {
        match app.registry.journals.iter().find(|e| e.id == journal_id).cloned() {
            Some(entry) => app.home = Some(HomeState::new(entry)),
            None => {
                app.home = None;
                show_journal_missing(app, ui);
                return;
            }
        }
    }

    // ── Reload entries from disk if dirty ────────────────────────────────
    if let Some(home) = app.home.as_mut() {
        if home.needs_reload {
            reload_entries(home);
        }
    }

    // ── Layout: top header + left sidebar + central editor ───────────────
    egui::Panel::top("home_header")
        .resizable(false)
        .show_inside(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("← Back").clicked() {
                    app.home = None;
                    app.screen = AppState::List;
                }
                if let Some(home) = &app.home {
                    ui.separator();
                    ui.strong(&home.journal_name);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.small(home.journal_root.display().to_string());
                    });
                }
            });
            ui.add_space(4.0);
        });

    if app.home.is_none() {
        return;
    }

    egui::Panel::left("entry_sidebar")
        .resizable(true)
        .default_size(280.0)
        .size_range(220.0..=480.0)
        .show_inside(ui, |ui| {
            draw_sidebar(app, ui);
        });

    egui::CentralPanel::default().show_inside(ui, |ui| {
        draw_editor_panel(app, ui);
    });

    // Confirm-delete-entry inline dialog
    if app
        .home
        .as_ref()
        .is_some_and(|h| h.confirm_delete_entry)
    {
        let ctx = ui.ctx().clone();
        draw_confirm_delete_entry(app, &ctx);
    }
}

// ── Helpers: missing journal ────────────────────────────────────────────────

fn show_journal_missing(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::top("home_header_missing")
        .resizable(false)
        .show_inside(ui, |ui| {
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
            ui.heading("Journal not found");
        });
    });
}

// ── Sidebar ──────────────────────────────────────────────────────────────────

fn draw_sidebar(app: &mut App, ui: &mut egui::Ui) {
    let home = match app.home.as_mut() {
        Some(h) => h,
        None => return,
    };

    // Toolbar: New Entry + Refresh
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.button("+ New Entry").clicked() {
            match Journal::from_root(home.journal_root.clone()) {
                Ok(journal) => match prepare_new_entry(&journal, None) {
                    Ok(path) => {
                        home.selected_path = Some(path.clone());
                        home.editor = match load_editor(&path) {
                            Ok(e) => Some(e),
                            Err(msg) => {
                                home.error_msg = Some(msg);
                                None
                            }
                        };
                        home.needs_reload = true;
                    }
                    Err(e) => home.error_msg = Some(e.to_string()),
                },
                Err(e) => home.error_msg = Some(e.to_string()),
            }
        }
        if ui.button("⟳").on_hover_text("Refresh entry list").clicked() {
            home.needs_reload = true;
        }
    });
    ui.add_space(4.0);

    if let Some(msg) = home.error_msg.clone() {
        ui.horizontal(|ui| {
            ui.colored_label(egui::Color32::LIGHT_RED, msg);
            if ui.small_button("×").clicked() {
                home.error_msg = None;
            }
        });
        ui.add_space(2.0);
    }

    // Search input
    ui.label("Search");
    ui.add(
        egui::TextEdit::singleline(&mut home.filter_text)
            .hint_text("Title / tag / id")
            .desired_width(f32::INFINITY),
    );

    ui.add_space(6.0);

    // Filters: period + sort
    ui.label("Period");
    egui::ComboBox::from_id_salt("home_period")
        .selected_text(period_label(&home.period))
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for (value, label) in PERIOD_OPTIONS {
                ui.selectable_value(&mut home.period, (*value).to_string(), *label);
            }
        });

    ui.add_space(4.0);
    ui.label("Sort by");
    egui::ComboBox::from_id_salt("home_sort_by")
        .selected_text(sort_label(&home.sort_by))
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for (value, label) in SORT_OPTIONS {
                ui.selectable_value(&mut home.sort_by, (*value).to_string(), *label);
            }
        });

    ui.add_space(4.0);
    ui.label("Order");
    egui::ComboBox::from_id_salt("home_sort_order")
        .selected_text(if home.sort_order == "asc" {
            "Ascending"
        } else {
            "Descending"
        })
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut home.sort_order, "desc".to_string(), "Descending");
            ui.selectable_value(&mut home.sort_order, "asc".to_string(), "Ascending");
        });

    ui.add_space(6.0);
    ui.separator();

    // Entry list
    if let Some(msg) = &home.entries_error {
        ui.colored_label(egui::Color32::LIGHT_RED, msg);
    }

    let filtered = filter_and_sort(home);
    if filtered.is_empty() {
        ui.add_space(8.0);
        ui.weak("No entries match.");
        return;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            let mut want_select: Option<PathBuf> = None;
            for header in &filtered {
                let path = PathBuf::from(header.path.clone());
                let is_active = home.selected_path.as_ref() == Some(&path);
                if draw_entry_row(ui, header, is_active).clicked() {
                    want_select = Some(path);
                }
            }
            if let Some(path) = want_select {
                if home.selected_path.as_ref() != Some(&path) {
                    home.selected_path = Some(path.clone());
                    home.editor = match load_editor(&path) {
                        Ok(e) => Some(e),
                        Err(msg) => {
                            home.error_msg = Some(msg);
                            None
                        }
                    };
                    home.info_msg = None;
                }
            }
        });
}

fn draw_entry_row(
    ui: &mut egui::Ui,
    header: &EntryHeader,
    is_active: bool,
) -> egui::Response {
    let title = if header.title().is_empty() {
        "(untitled)".to_string()
    } else {
        header.title().to_string()
    };
    let flags: String = header
        .flags
        .iter()
        .map(|f| flag_glyph(*f))
        .collect::<Vec<_>>()
        .join(" ");
    let id_str = header.id().to_string();
    let tags_str = if header.frontmatter.tags.is_empty() {
        String::new()
    } else {
        header
            .frontmatter
            .tags
            .iter()
            .map(|t| format!("#{t}"))
            .collect::<Vec<_>>()
            .join(" ")
    };

    let frame = if is_active {
        egui::Frame::new()
            .fill(ui.visuals().selection.bg_fill)
            .inner_margin(egui::Margin::symmetric(6, 4))
            .corner_radius(4.0)
    } else {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(6, 4))
    };

    let resp = frame
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    if !flags.is_empty() {
                        ui.label(&flags);
                    }
                    ui.add(egui::Label::new(egui::RichText::new(&title).strong()).truncate());
                });
                ui.horizontal(|ui| {
                    ui.weak(format!("@{id_str}"));
                    if !tags_str.is_empty() {
                        ui.weak(tags_str);
                    }
                });
            });
        })
        .response;

    // Make the row clickable.
    let resp = ui.interact(resp.rect, resp.id.with("row_btn"), egui::Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

// ── Editor panel ─────────────────────────────────────────────────────────────

fn draw_editor_panel(app: &mut App, ui: &mut egui::Ui) {
    let home = match app.home.as_mut() {
        Some(h) => h,
        None => return,
    };

    if home.editor.is_none() {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.heading("No entry selected");
            ui.label("Pick an entry from the sidebar, or create a new one.");
        });
        return;
    }

    // Header bar: id + action buttons
    let mut do_save = false;
    let mut do_request_delete = false;
    {
        let editor = home.editor.as_ref().unwrap();
        ui.horizontal(|ui| {
            ui.weak(format!("@{}", editor.id));
            if let Some(msg) = &home.info_msg {
                ui.colored_label(egui::Color32::LIGHT_GREEN, msg);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Save").clicked() {
                    do_save = true;
                }
                if ui.button("Delete").clicked() {
                    do_request_delete = true;
                }
            });
        });
    }

    if do_request_delete {
        home.confirm_delete_entry = true;
    }

    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            let editor = home.editor.as_mut().unwrap();

            ui.label("Title");
            ui.add(
                egui::TextEdit::singleline(&mut editor.title)
                    .hint_text("Untitled")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(8.0);

            ui.label("Tags (comma-separated)");
            ui.add(
                egui::TextEdit::singleline(&mut editor.tags)
                    .hint_text("work, journal")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(8.0);

            // Task fieldset
            ui.group(|ui| {
                ui.checkbox(&mut editor.has_task, "Task");
                if editor.has_task {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label("Status");
                            egui::ComboBox::from_id_salt("editor_task_status")
                                .selected_text(&editor.task_status)
                                .show_ui(ui, |ui| {
                                    for s in TASK_STATUS_OPTIONS {
                                        ui.selectable_value(
                                            &mut editor.task_status,
                                            (*s).to_string(),
                                            *s,
                                        );
                                    }
                                });
                        });
                        ui.vertical(|ui| {
                            ui.label("Due (YYYY-MM-DD)");
                            ui.add(
                                egui::TextEdit::singleline(&mut editor.task_due)
                                    .hint_text("2026-05-16")
                                    .desired_width(160.0),
                            );
                        });
                    });
                }
            });
            ui.add_space(6.0);

            // Event fieldset
            ui.group(|ui| {
                ui.checkbox(&mut editor.has_event, "Event");
                if editor.has_event {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label("Start (YYYY-MM-DD)");
                            ui.add(
                                egui::TextEdit::singleline(&mut editor.event_start)
                                    .desired_width(160.0),
                            );
                        });
                        ui.vertical(|ui| {
                            ui.label("End (YYYY-MM-DD)");
                            ui.add(
                                egui::TextEdit::singleline(&mut editor.event_end)
                                    .desired_width(160.0),
                            );
                        });
                    });
                }
            });
            ui.add_space(8.0);

            ui.label("Body (Markdown)");
            ui.add(
                egui::TextEdit::multiline(&mut editor.body)
                    .desired_rows(20)
                    .desired_width(f32::INFINITY)
                    .code_editor(),
            );
        });

    if do_save {
        save_current_entry(home);
    }
}

fn draw_confirm_delete_entry(app: &mut App, ctx: &egui::Context) {
    let mut cancel = false;
    let mut confirm = false;
    let mut open = true;

    egui::Window::new("Delete entry?")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_min_width(360.0);
            ui.label("This will permanently delete the entry file.");
            ui.label("This action cannot be undone.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Delete").clicked() {
                        confirm = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        });

    if !open {
        cancel = true;
    }

    let home = match app.home.as_mut() {
        Some(h) => h,
        None => return,
    };

    if cancel {
        home.confirm_delete_entry = false;
    }
    if confirm {
        home.confirm_delete_entry = false;
        if let Some(path) = home.selected_path.clone() {
            match remove_entry(&path) {
                Ok(()) => {
                    home.selected_path = None;
                    home.editor = None;
                    home.info_msg = None;
                    home.needs_reload = true;
                }
                Err(e) => home.error_msg = Some(e.to_string()),
            }
        }
    }
}

// ── Disk operations ──────────────────────────────────────────────────────────

fn reload_entries(home: &mut HomeState) {
    home.needs_reload = false;
    home.entries_error = None;
    let journal = match Journal::from_root(home.journal_root.clone()) {
        Ok(j) => j,
        Err(e) => {
            home.entries_error = Some(e.to_string());
            home.entries.clear();
            return;
        }
    };
    let paths = match journal.collect_entries() {
        Ok(p) => p,
        Err(e) => {
            home.entries_error = Some(e.to_string());
            home.entries.clear();
            return;
        }
    };
    home.entries = paths
        .iter()
        .filter_map(|p| read_entry(p).ok().map(EntryHeader::from))
        .collect();
}

fn filter_and_sort(home: &HomeState) -> Vec<EntryHeader> {
    let mut headers = home.entries.clone();

    // Period filter via core EntryFilter.
    if !home.period.trim().is_empty() {
        if let Ok(period) = parse_period(home.period.trim()) {
            let filter = EntryFilter {
                period: Some(period),
                fields: FieldSelector::default(),
                task_status: Vec::new(),
                tags: Vec::new(),
                sort_by: CoreSortField::Unsorted,
                sort_order: CoreSortOrder::Asc,
            };
            headers.retain(|h| filter.matches(h).0);
        }
    }

    // Substring filter on title / tags / id.
    let needle = home.filter_text.trim().to_lowercase();
    if !needle.is_empty() {
        headers.retain(|h| {
            h.title().to_lowercase().contains(&needle)
                || h.id().to_string().to_lowercase().contains(&needle)
                || h.frontmatter
                    .tags
                    .iter()
                    .any(|t| t.to_lowercase().contains(&needle))
        });
    }

    // Sort.
    match home.sort_by.as_str() {
        "title" => headers.sort_by(|a, b| a.title().cmp(b.title())),
        "id" => headers.sort_by(|a, b| a.id().cmp(&b.id())),
        "created_at" => headers.sort_by(|a, b| a.frontmatter.created_at.cmp(&b.frontmatter.created_at)),
        "updated_at" => headers.sort_by(|a, b| a.frontmatter.updated_at.cmp(&b.frontmatter.updated_at)),
        "task_due" => headers.sort_by(|a, b| {
            let av = a.frontmatter.task.as_ref().and_then(|t| t.due);
            let bv = b.frontmatter.task.as_ref().and_then(|t| t.due);
            cmp_opt(av, bv)
        }),
        "event_start" => headers.sort_by(|a, b| {
            let av = a.frontmatter.event.as_ref().map(|e| e.start);
            let bv = b.frontmatter.event.as_ref().map(|e| e.start);
            cmp_opt(av, bv)
        }),
        _ => {}
    }
    if home.sort_order == "desc" {
        headers.reverse();
    }
    headers
}

fn cmp_opt<T: Ord>(a: Option<T>, b: Option<T>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn load_editor(path: &std::path::Path) -> Result<EditorState, String> {
    let entry = read_entry(path).map_err(|e| e.to_string())?;
    let fm = &entry.frontmatter;
    Ok(EditorState {
        id: fm.id.to_string(),
        title: fm.title.clone(),
        tags: fm.tags.join(", "),
        body: entry.body.clone(),
        has_task: fm.task.is_some(),
        task_status: fm
            .task
            .as_ref()
            .map(|t| t.status.clone())
            .unwrap_or_else(|| "open".to_string()),
        task_due: fm
            .task
            .as_ref()
            .and_then(|t| t.due)
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default(),
        has_event: fm.event.is_some(),
        event_start: fm
            .event
            .as_ref()
            .map(|e| e.start.format("%Y-%m-%d").to_string())
            .unwrap_or_default(),
        event_end: fm
            .event
            .as_ref()
            .map(|e| e.end.format("%Y-%m-%d").to_string())
            .unwrap_or_default(),
    })
}

fn save_current_entry(home: &mut HomeState) {
    let Some(path) = home.selected_path.clone() else {
        return;
    };
    let Some(editor) = home.editor.as_ref() else {
        return;
    };

    home.error_msg = None;
    home.info_msg = None;

    let mut entry = match read_entry(&path) {
        Ok(e) => e,
        Err(e) => {
            home.error_msg = Some(format!("read failed: {e}"));
            return;
        }
    };

    entry.frontmatter.title = editor.title.clone();
    entry.frontmatter.tags = editor
        .tags
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    entry.body = editor.body.clone();

    if editor.has_task {
        let due = if editor.task_due.trim().is_empty() {
            None
        } else {
            match sapphire_journal_core::period::parse_datetime_end(editor.task_due.trim()) {
                Ok(d) => Some(d),
                Err(e) => {
                    home.error_msg = Some(format!("task.due: {e}"));
                    return;
                }
            }
        };
        let status = if editor.task_status.is_empty() {
            "open".to_string()
        } else {
            editor.task_status.clone()
        };
        let prev = entry.frontmatter.task.take();
        entry.frontmatter.task = Some(sapphire_journal_core::entry::TaskMeta {
            due,
            status,
            started_at: prev.as_ref().and_then(|t| t.started_at),
            closed_at: prev.as_ref().and_then(|t| t.closed_at),
            extra: prev.map(|t| t.extra).unwrap_or_default(),
        });
    } else {
        entry.frontmatter.task = None;
    }

    if editor.has_event {
        if editor.event_start.trim().is_empty() && editor.event_end.trim().is_empty() {
            home.error_msg = Some("event requires a start or end date".to_string());
            return;
        }
        let start_str = if editor.event_start.trim().is_empty() {
            editor.event_end.trim()
        } else {
            editor.event_start.trim()
        };
        let end_str = if editor.event_end.trim().is_empty() {
            editor.event_start.trim()
        } else {
            editor.event_end.trim()
        };
        let start = match sapphire_journal_core::period::parse_datetime(start_str) {
            Ok(d) => d,
            Err(e) => {
                home.error_msg = Some(format!("event.start: {e}"));
                return;
            }
        };
        let end = match sapphire_journal_core::period::parse_datetime_end(end_str) {
            Ok(d) => d,
            Err(e) => {
                home.error_msg = Some(format!("event.end: {e}"));
                return;
            }
        };
        let prev = entry.frontmatter.event.take();
        entry.frontmatter.event = Some(sapphire_journal_core::entry::EventMeta {
            start,
            end,
            extra: prev.map(|e| e.extra).unwrap_or_default(),
        });
    } else {
        entry.frontmatter.event = None;
    }

    if let Err(e) = write_entry(&mut entry) {
        home.error_msg = Some(format!("write failed: {e}"));
        return;
    }

    match fix_entry(&entry.path) {
        Ok(maybe_new) => {
            let final_path = maybe_new.unwrap_or(entry.path.clone());
            home.selected_path = Some(final_path.clone());
            home.editor = match load_editor(&final_path) {
                Ok(e) => Some(e),
                Err(msg) => {
                    home.error_msg = Some(msg);
                    None
                }
            };
            home.info_msg = Some("Saved.".to_string());
            home.needs_reload = true;
        }
        Err(e) => home.error_msg = Some(format!("fix failed: {e}")),
    }
}

// ── Display helpers ─────────────────────────────────────────────────────────

fn flag_glyph(flag: EntryFlag) -> &'static str {
    flag.to_emoji()
}

const PERIOD_OPTIONS: &[(&str, &str)] = &[
    ("", "All"),
    ("today", "Today"),
    ("yesterday", "Yesterday"),
    ("tomorrow", "Tomorrow"),
    ("this_week", "This week"),
    ("last_week", "Last week"),
    ("next_week", "Next week"),
    ("this_month", "This month"),
    ("last_month", "Last month"),
    ("next_month", "Next month"),
];

fn period_label(value: &str) -> &'static str {
    PERIOD_OPTIONS
        .iter()
        .find(|(v, _)| *v == value)
        .map(|(_, l)| *l)
        .unwrap_or("All")
}

const SORT_OPTIONS: &[(&str, &str)] = &[
    ("updated_at", "Updated"),
    ("created_at", "Created"),
    ("title", "Title"),
    ("id", "ID"),
    ("task_due", "Task due"),
    ("event_start", "Event start"),
];

fn sort_label(value: &str) -> &'static str {
    SORT_OPTIONS
        .iter()
        .find(|(v, _)| *v == value)
        .map(|(_, l)| *l)
        .unwrap_or("Updated")
}

const TASK_STATUS_OPTIONS: &[&str] = &[
    "open",
    "in_progress",
    "done",
    "cancelled",
    "archived",
];
