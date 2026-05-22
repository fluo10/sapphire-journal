use std::collections::HashSet;
use std::path::PathBuf;

use eframe::egui;
use grain_id::GrainId;
use uuid::Uuid;

use sapphire_journal_core::{
    JournalState,
    entry::EntryHeader,
    entry_ref::EntryRef,
    journal::Journal,
    ops::{
        EntryFields, EntryFilter, EntryTreeNode, FieldSelector, SortField as CoreSortField,
        SortOrder as CoreSortOrder, UpdateOption, build_entry_tree, fix_entry, prepare_new_entry,
        remove_entry, update_entry,
    },
    parser::{read_entry, write_entry},
    period::parse_period,
};

use crate::app::{App, AppState, EditorState, HomeState, ViewMode};
use crate::icons;

pub fn show(app: &mut App, ui: &mut egui::Ui, journal_id: Uuid) {
    // ── Ensure HomeState matches the requested journal ────────────────────
    let needs_init = app.home.as_ref().map(|h| h.journal_id) != Some(journal_id);
    if needs_init {
        match app.registry.journals.iter().find(|e| e.id == journal_id).cloned() {
            Some(entry) => {
                app.home = Some(HomeState::new(entry));
                app.remember_last_opened(Some(journal_id));
            }
            None => {
                app.home = None;
                *app.journal_state.lock().unwrap() = None;
                show_journal_missing(app, ui);
                return;
            }
        }
    }

    // ── Ensure JournalState is open for the current journal ───────────────
    // Idempotent — bails out immediately when state.journal.root already
    // matches.  Called unconditionally so the startup auto-open path (which
    // builds HomeState in `App::new` but never enters the `if needs_init`
    // branch here) still gets its state initialised, which is what the
    // periodic-sync task needs to see in order to do anything.
    let root_opt = app.home.as_ref().map(|h| h.journal_root.clone());
    if let Some(root) = root_opt {
        ensure_journal_state(app, &root);
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
                draw_journal_switcher(app, ui, journal_id);
                if let Some(home) = &app.home {
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

    // Settings panel (opened from the journal-switcher menu)
    if app.settings_panel.is_some() {
        let ctx = ui.ctx().clone();
        crate::screens::settings_panel::show(app, &ctx);
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
    let mut new_entry_path: Option<PathBuf> = None;
    let home = match app.home.as_mut() {
        Some(h) => h,
        None => return,
    };

    // Toolbar: New Entry + Refresh + view mode toggle
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if icon_text_btn(ui, icons::plus(), "New Entry").clicked() {
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
                        new_entry_path = Some(path);
                    }
                    Err(e) => home.error_msg = Some(e.to_string()),
                },
                Err(e) => home.error_msg = Some(e.to_string()),
            }
        }
        if icon_btn(ui, icons::refresh(), "Refresh entry list").clicked() {
            home.needs_reload = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Show the icon for the *other* mode (i.e. what clicking would switch to).
            let (icon, tooltip) = match home.view_mode {
                ViewMode::Tree => (icons::list_view(), "Switch to list view"),
                ViewMode::List => (icons::tree_view(), "Switch to tree view"),
            };
            if icon_btn(ui, icon, tooltip).clicked() {
                home.view_mode = match home.view_mode {
                    ViewMode::Tree => ViewMode::List,
                    ViewMode::List => ViewMode::Tree,
                };
            }
            let filters_tooltip = if home.show_filters {
                "Hide filters"
            } else {
                "Show filters"
            };
            if icon_toggle_btn(ui, icons::funnel(), home.show_filters, filters_tooltip).clicked() {
                home.show_filters = !home.show_filters;
            }
        });
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

    if home.show_filters {
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
    }

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

    let mut reparent_action: Option<(PathBuf, Option<GrainId>)> = None;
    let mut entry_action: Option<EntryAction> = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            let mut want_select: Option<PathBuf> = None;
            match home.view_mode {
                ViewMode::List => {
                    for header in &filtered {
                        let path = PathBuf::from(header.path.clone());
                        let is_active = home.selected_path.as_ref() == Some(&path);
                        if draw_entry_row(ui, header, is_active, &mut entry_action).clicked() {
                            want_select = Some(path);
                        }
                    }
                }
                ViewMode::Tree => {
                    // Root drop zone: dropping above the first row clears `parent_id`.
                    draw_root_drop_zone(ui, &home.entries, &mut reparent_action);
                    let pairs = filtered.into_iter().map(|h| (h, Vec::new())).collect();
                    let tree = build_entry_tree(pairs);
                    draw_tree(
                        ui,
                        &tree,
                        0,
                        &mut home.collapsed,
                        &home.selected_path,
                        &mut want_select,
                        &home.entries,
                        &mut reparent_action,
                        &mut entry_action,
                    );
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

    match entry_action {
        Some(EntryAction::AddChild(parent_id)) => {
            match Journal::from_root(home.journal_root.clone()) {
                Ok(journal) => match prepare_new_entry(&journal, Some(parent_id)) {
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
                        new_entry_path = Some(path);
                    }
                    Err(e) => home.error_msg = Some(e.to_string()),
                },
                Err(e) => home.error_msg = Some(e.to_string()),
            }
        }
        Some(EntryAction::RequestDelete(path)) => {
            if home.selected_path.as_ref() != Some(&path) {
                home.editor = match load_editor(&path) {
                    Ok(e) => Some(e),
                    Err(msg) => {
                        home.error_msg = Some(msg);
                        None
                    }
                };
                home.selected_path = Some(path);
                home.info_msg = None;
            }
            home.confirm_delete_entry = true;
        }
        None => {}
    }

    // Stage the freshly-created entry file with the sync backend (if any).
    // Done after the `home` borrow ends so we can take `&mut app` here.
    if let Some(path) = new_entry_path {
        notify_file_updated(app, &path);
    }
    if let Some((source_path, new_parent)) = reparent_action {
        apply_reparent(app, source_path, new_parent);
    }
}

#[derive(Clone)]
enum EntryAction {
    AddChild(GrainId),
    RequestDelete(PathBuf),
}

fn entry_row_context_menu(
    response: &egui::Response,
    header: &EntryHeader,
    pending: &mut Option<EntryAction>,
) {
    response.context_menu(|ui| {
        if ui.button("Add child entry").clicked() {
            *pending = Some(EntryAction::AddChild(header.id()));
            ui.close();
        }
        if ui.button("Delete entry…").clicked() {
            *pending = Some(EntryAction::RequestDelete(PathBuf::from(
                header.path.clone(),
            )));
            ui.close();
        }
    });
}

fn draw_tree(
    ui: &mut egui::Ui,
    nodes: &[EntryTreeNode],
    depth: usize,
    collapsed: &mut HashSet<GrainId>,
    selected: &Option<PathBuf>,
    want_select: &mut Option<PathBuf>,
    entries: &[EntryHeader],
    reparent: &mut Option<(PathBuf, Option<GrainId>)>,
    pending: &mut Option<EntryAction>,
) {
    for node in nodes {
        let id = node.entry.frontmatter.id;
        let has_children = !node.children.is_empty();
        let is_collapsed = collapsed.contains(&id);
        let path = PathBuf::from(node.entry.path.clone());
        let is_active = selected.as_ref() == Some(&path);

        let row = ui
            .horizontal(|ui| {
                // Indent + collapse toggle (or spacer for leaves). The spacer
                // is allocated as a widget (not raw `add_space`) so the same
                // `item_spacing.x` is inserted on each side as for the chevron
                // button — keeping leaf rows aligned with parent rows.
                ui.add_space(depth as f32 * 12.0);
                if has_children {
                    let icon = if is_collapsed {
                        icons::chevron_right()
                    } else {
                        icons::chevron_down()
                    };
                    if tree_toggle(ui, icon).clicked() {
                        if is_collapsed {
                            collapsed.remove(&id);
                        } else {
                            collapsed.insert(id);
                        }
                    }
                } else {
                    ui.allocate_exact_size(
                        egui::vec2(TREE_TOGGLE_SIZE, TREE_TOGGLE_SIZE),
                        egui::Sense::hover(),
                    );
                }
                let dnd_id = egui::Id::new(("entry_dnd", id));
                // Inlined `dnd_drag_source` using `Sense::click_and_drag()` so
                // a press-and-release without movement registers as a click
                // (for selection) while a press-and-drag still starts a drag.
                // The stock `dnd_drag_source` overlays a `Sense::drag()` widget
                // that shadows the row's click sense, so clicks were lost.
                if ui.ctx().is_being_dragged(dnd_id) {
                    egui::DragAndDrop::set_payload(ui.ctx(), id);
                    let layer_id = egui::LayerId::new(egui::Order::Tooltip, dnd_id);
                    let inner = ui.scope_builder(
                        egui::UiBuilder::new().layer_id(layer_id),
                        |ui| draw_entry_row(ui, &node.entry, is_active, pending),
                    );
                    if let Some(pos) = ui.ctx().pointer_interact_pos() {
                        let delta = pos - inner.response.rect.center();
                        ui.ctx().transform_layer_shapes(
                            layer_id,
                            egui::emath::TSTransform::from_translation(delta),
                        );
                    }
                    inner.response
                } else {
                    let inner =
                        ui.scope(|ui| draw_entry_row(ui, &node.entry, is_active, pending));
                    let outer = ui
                        .interact(
                            inner.response.rect,
                            dnd_id,
                            egui::Sense::click_and_drag(),
                        )
                        .on_hover_cursor(egui::CursorIcon::Grab);
                    entry_row_context_menu(&outer, &node.entry, pending);
                    outer
                }
            })
            .inner;

        if row.clicked() {
            *want_select = Some(path.clone());
        }

        // ── drag-and-drop drop target ────────────────────────────────────
        // Highlight the row when something is being dragged over it.
        if let Some(payload) = row.dnd_hover_payload::<GrainId>() {
            let src = *payload;
            let would_be_cycle = is_descendant_or_self(entries, src, id);
            let stroke = if would_be_cycle {
                egui::Stroke::new(1.5, egui::Color32::DARK_RED)
            } else {
                egui::Stroke::new(1.5, ui.visuals().selection.bg_fill)
            };
            ui.painter()
                .rect_stroke(row.rect, 4.0, stroke, egui::StrokeKind::Inside);
        }
        if let Some(payload) = row.dnd_release_payload::<GrainId>() {
            let src = *payload;
            if src != id && !is_descendant_or_self(entries, src, id) {
                if let Some(header) = entries.iter().find(|e| e.frontmatter.id == src) {
                    // Only act when the parent actually changes.
                    if header.frontmatter.parent_id != Some(id) {
                        *reparent = Some((PathBuf::from(header.path.clone()), Some(id)));
                    }
                }
            }
        }

        if has_children && !is_collapsed {
            draw_tree(
                ui,
                &node.children,
                depth + 1,
                collapsed,
                selected,
                want_select,
                entries,
                reparent,
                pending,
            );
        }
    }
}

/// Returns `true` if `candidate` is `ancestor` itself, or any descendant of
/// `ancestor`, by walking the `parent_id` chain on `candidate`.  Used to
/// prevent the user from dropping an entry into its own subtree.
fn is_descendant_or_self(
    entries: &[EntryHeader],
    ancestor: GrainId,
    candidate: GrainId,
) -> bool {
    if candidate == ancestor {
        return true;
    }
    let mut current = candidate;
    let mut seen: HashSet<GrainId> = HashSet::new();
    loop {
        if !seen.insert(current) {
            return false;
        }
        let entry = match entries.iter().find(|e| e.frontmatter.id == current) {
            Some(e) => e,
            None => return false,
        };
        match entry.frontmatter.parent_id {
            Some(pid) if pid == ancestor => return true,
            Some(pid) => current = pid,
            None => return false,
        }
    }
}

/// Thin strip at the top of the tree that accepts drops to clear `parent_id`
/// (make the entry top-level).  Only renders visibly when a drag is in
/// progress, so it doesn't add chrome the rest of the time.
fn draw_root_drop_zone(
    ui: &mut egui::Ui,
    entries: &[EntryHeader],
    reparent: &mut Option<(PathBuf, Option<GrainId>)>,
) {
    let is_dragging = egui::DragAndDrop::has_payload_of_type::<GrainId>(ui.ctx());
    if !is_dragging {
        return;
    }
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 20.0),
        egui::Sense::hover(),
    );
    let visuals = ui.visuals();
    let is_hover = resp.contains_pointer();
    let bg = if is_hover {
        visuals.selection.bg_fill
    } else {
        visuals.widgets.inactive.bg_fill
    };
    ui.painter().rect_filled(rect, 4.0, bg);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "Drop here for top level",
        egui::FontId::proportional(11.0),
        visuals.text_color(),
    );
    if let Some(payload) = resp.dnd_release_payload::<GrainId>() {
        let src = *payload;
        if let Some(header) = entries.iter().find(|e| e.frontmatter.id == src) {
            if header.frontmatter.parent_id.is_some() {
                *reparent = Some((PathBuf::from(header.path.clone()), None));
            }
        }
    }
}

/// Apply a drag-and-drop reparenting action: rewrite the source entry's
/// `parent_id` via [`update_entry`], update any path rename, and notify the
/// sync backend.  Runs outside the `home` borrow so it can take `&mut App`.
fn apply_reparent(app: &mut App, source_path: PathBuf, new_parent: Option<GrainId>) {
    let conn_result = {
        let guard = app.journal_state.lock().unwrap();
        guard.as_ref().map(|s| s.open_conn())
    };
    let conn = match conn_result {
        Some(Ok(c)) => c,
        Some(Err(e)) => {
            if let Some(home) = app.home.as_mut() {
                home.error_msg = Some(format!("Failed to open cache: {e}"));
            }
            return;
        }
        None => {
            if let Some(home) = app.home.as_mut() {
                home.error_msg = Some("No journal is open".to_string());
            }
            return;
        }
    };

    let parent_field = match new_parent {
        Some(id) => UpdateOption::Set(EntryRef::Id(id)),
        None => UpdateOption::Clear,
    };
    let fields = EntryFields {
        parent: parent_field,
        ..Default::default()
    };

    let renamed = match update_entry(&source_path, &conn, fields) {
        Ok(maybe_new) => maybe_new,
        Err(e) => {
            if let Some(home) = app.home.as_mut() {
                home.error_msg = Some(format!("Reparent failed: {e}"));
            }
            return;
        }
    };
    drop(conn);

    let final_path = renamed.clone().unwrap_or_else(|| source_path.clone());
    // Track selection if it moved with the rename.
    if let Some(home) = app.home.as_mut() {
        if home.selected_path.as_deref() == Some(source_path.as_path()) {
            home.selected_path = Some(final_path.clone());
        }
        home.needs_reload = true;
        home.info_msg = Some("Reparented.".to_string());
    }
    if let Some(new_path) = renamed.as_ref() {
        if new_path != &source_path {
            notify_file_deleted(app, &source_path);
        }
    }
    notify_file_updated(app, &final_path);
}

fn draw_entry_row(
    ui: &mut egui::Ui,
    header: &EntryHeader,
    is_active: bool,
    pending: &mut Option<EntryAction>,
) -> egui::Response {
    let title = if header.title().is_empty() {
        "(untitled)".to_string()
    } else {
        header.title().to_string()
    };
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
                    let tint = ui.visuals().text_color();
                    for &flag in &header.flags {
                        ui.add(
                            egui::Image::new(icons::flag_icon(flag))
                                .fit_to_exact_size(egui::vec2(14.0, 14.0))
                                .tint(tint),
                        )
                        .on_hover_text(flag.as_str());
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
    entry_row_context_menu(&resp, header, pending);
    resp
}

// ── Editor panel ─────────────────────────────────────────────────────────────

fn draw_editor_panel(app: &mut App, ui: &mut egui::Ui) {
    let do_save = {
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

        draw_editor_panel_inner(home, ui)
    };
    if do_save {
        save_current_entry(app);
    }
}

/// Renders the editor body using an exclusive `&mut HomeState`.  Returns
/// `true` when the Save button was clicked so the caller can apply the
/// disk write outside the home borrow.
fn draw_editor_panel_inner(home: &mut HomeState, ui: &mut egui::Ui) -> bool {
    let mut do_save = false;

    // Header bar: id + action buttons
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
                if icon_text_btn(ui, icons::trash(), "Delete").clicked() {
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

    do_save
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

    let mut deleted_path: Option<PathBuf> = None;
    {
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
                        deleted_path = Some(path);
                    }
                    Err(e) => home.error_msg = Some(e.to_string()),
                }
            }
        }
    }

    if let Some(path) = deleted_path {
        notify_file_deleted(app, &path);
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
                fields: FieldSelector::active(),
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

fn save_current_entry(app: &mut App) {
    let mut updated_path: Option<PathBuf> = None;
    let mut renamed_from: Option<PathBuf> = None;
    {
        let home = match app.home.as_mut() {
            Some(h) => h,
            None => return,
        };
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
                let final_path = maybe_new.clone().unwrap_or(entry.path.clone());
                if let Some(new_path) = maybe_new.as_ref() {
                    if new_path != &entry.path {
                        renamed_from = Some(entry.path.clone());
                    }
                }
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
                updated_path = Some(final_path);
            }
            Err(e) => home.error_msg = Some(format!("fix failed: {e}")),
        }
    }

    // Notify the sync backend (outside the home borrow).  When `fix_entry`
    // renamed the file we also untrack the previous path.
    if let Some(old) = renamed_from {
        notify_file_deleted(app, &old);
    }
    if let Some(path) = updated_path {
        notify_file_updated(app, &path);
    }
}

// ── Journal-state lifecycle ────────────────────────────────────────────────

/// Open the journal's [`JournalState`] (cache DB + retrieve DB + sync backend)
/// and stash it in `app.journal_state` so the periodic-sync task can use it.
/// Idempotent: re-opening for the same `journal_root` is a no-op.
///
/// On a successful transition (None → Some, or different journal → this one)
/// kicks off an initial sync so the user gets fresh remote state immediately
/// rather than waiting up to `sync_interval_minutes` for the first tick.
fn ensure_journal_state(app: &mut App, journal_root: &std::path::Path) {
    let already_open = {
        let guard = app.journal_state.lock().unwrap();
        guard
            .as_ref()
            .map(|s| s.journal.root.as_path() == journal_root)
            .unwrap_or(false)
    };
    if already_open {
        return;
    }
    let result = Journal::from_root(journal_root.to_path_buf())
        .map_err(|e| e.to_string())
        .and_then(|j| JournalState::open(j).map_err(|e| e.to_string()));
    match result {
        Ok(state) => {
            *app.journal_state.lock().unwrap() = Some(state);
            trigger_manual_sync(app);
        }
        Err(msg) => {
            *app.journal_state.lock().unwrap() = None;
            if let Some(home) = app.home.as_mut() {
                home.error_msg = Some(format!("Failed to open journal state: {msg}"));
            }
        }
    }
}

/// Notify the sync backend that `path` was created or modified.
/// Failures are reported to the home error banner.
fn notify_file_updated(app: &mut App, path: &std::path::Path) {
    let result = {
        let guard = app.journal_state.lock().unwrap();
        guard.as_ref().map(|s| s.on_file_updated(path))
    };
    if let Some(Err(e)) = result {
        if let Some(home) = app.home.as_mut() {
            home.error_msg = Some(format!("Failed to stage file: {e}"));
        }
    }
}

/// Notify the sync backend that `path` was deleted.
fn notify_file_deleted(app: &mut App, path: &std::path::Path) {
    let result = {
        let guard = app.journal_state.lock().unwrap();
        guard.as_ref().map(|s| s.on_file_deleted(path))
    };
    if let Some(Err(e)) = result {
        if let Some(home) = app.home.as_mut() {
            home.error_msg = Some(format!("Failed to unstage file: {e}"));
        }
    }
}

// ── Journal switcher ────────────────────────────────────────────────────────

/// Top-left "current journal" dropdown.  Shows other registered journals as
/// clickable switch targets and a "Manage Journals…" entry that drops back to
/// the list screen for create / clone / delete actions.
fn draw_journal_switcher(app: &mut App, ui: &mut egui::Ui, current_id: Uuid) {
    let current_name = app
        .home
        .as_ref()
        .map(|h| h.journal_name.clone())
        .unwrap_or_else(|| "Journal".to_string());
    let others: Vec<(Uuid, String)> = app
        .registry
        .journals
        .iter()
        .filter(|e| e.id != current_id)
        .map(|e| (e.id, e.name.clone()))
        .collect();

    let mut switch_to: Option<Uuid> = None;
    let mut go_manage = false;
    let mut sync_now = false;
    let mut open_settings = false;

    let response = ui.menu_button(format!("{current_name} ▾"), |ui| {
        if !others.is_empty() {
            ui.label("Other journals");
            for (id, name) in &others {
                if ui.button(name).clicked() {
                    switch_to = Some(*id);
                    ui.close();
                }
            }
            ui.separator();
        }
        if ui.button("Sync now").clicked() {
            sync_now = true;
            ui.close();
        }
        if ui.button("Settings…").clicked() {
            open_settings = true;
            ui.close();
        }
        if ui.button("Manage Journals…").clicked() {
            go_manage = true;
            ui.close();
        }
    });
    response.response.on_hover_text("Switch journal");

    if let Some(id) = switch_to {
        app.home = None;
        *app.journal_state.lock().unwrap() = None;
        app.remember_last_opened(Some(id));
        app.screen = AppState::Home { journal_id: id };
    } else if go_manage {
        app.previous_journal_id = Some(current_id);
        app.screen = AppState::List;
    } else if sync_now {
        trigger_manual_sync(app);
    } else if open_settings {
        if let Some(home) = app.home.as_ref() {
            let root = home.journal_root.clone();
            app.settings_panel = Some(
                crate::screens::settings_panel::SettingsPanelState::open(root),
            );
        }
    }
}

/// Run cache + git sync once, off-thread, and report success/failure via
/// `AppEvent`.  Lets the user verify sync independently of the 10-minute
/// periodic tick.
fn trigger_manual_sync(app: &mut App) {
    let state_arc = std::sync::Arc::clone(&app.journal_state);
    let tx = app.event_tx.clone();
    app.runtime.spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            let guard = state_arc.lock().unwrap();
            let Some(state) = guard.as_ref() else {
                return Err("no journal is open".to_string());
            };
            state.sync().map_err(|e| format!("cache sync: {e}"))?;
            state.git_sync().map_err(|e| format!("git sync: {e}"))?;
            Ok::<(), String>(())
        })
        .await;
        match result {
            Ok(Ok(())) => {
                let _ = tx.send(crate::app::AppEvent::EntriesNeedReload);
            }
            Ok(Err(msg)) => {
                let _ = tx.send(crate::app::AppEvent::Error(format!(
                    "Sync failed: {msg}"
                )));
            }
            Err(join_err) => {
                let _ = tx.send(crate::app::AppEvent::Error(format!(
                    "Sync task panicked: {join_err}"
                )));
            }
        }
    });
}

// ── Display helpers ─────────────────────────────────────────────────────────

/// Square icon-only button (16×16) with a tooltip.
fn icon_btn(
    ui: &mut egui::Ui,
    src: egui::ImageSource<'static>,
    tooltip: &str,
) -> egui::Response {
    let tint = ui.visuals().text_color();
    let img = egui::Image::new(src)
        .fit_to_exact_size(egui::vec2(16.0, 16.0))
        .tint(tint);
    ui.add(egui::Button::image(img)).on_hover_text(tooltip)
}

/// Toggleable icon button — renders with a pressed/highlighted background
/// when `active` is true.
fn icon_toggle_btn(
    ui: &mut egui::Ui,
    src: egui::ImageSource<'static>,
    active: bool,
    tooltip: &str,
) -> egui::Response {
    let tint = ui.visuals().text_color();
    let img = egui::Image::new(src)
        .fit_to_exact_size(egui::vec2(16.0, 16.0))
        .tint(tint);
    ui.add(egui::Button::image(img).selected(active))
        .on_hover_text(tooltip)
}

/// Icon + text button (14×14 icon to align with the label height).
fn icon_text_btn(
    ui: &mut egui::Ui,
    src: egui::ImageSource<'static>,
    text: &str,
) -> egui::Response {
    let tint = ui.visuals().text_color();
    let img = egui::Image::new(src)
        .fit_to_exact_size(egui::vec2(14.0, 14.0))
        .tint(tint);
    ui.add(egui::Button::image_and_text(img, text))
}

/// Exact width reserved for the tree expand/collapse toggle (and the
/// matching leaf spacer).  Kept narrow so titles align with leaf rows.
const TREE_TOGGLE_SIZE: f32 = 14.0;

/// Frameless expand/collapse triangle for tree rows.  Matches `TREE_TOGGLE_SIZE`
/// exactly so leaf rows (which insert a spacer of the same width) stay aligned.
fn tree_toggle(ui: &mut egui::Ui, src: egui::ImageSource<'static>) -> egui::Response {
    let tint = ui.visuals().weak_text_color();
    let img = egui::Image::new(src)
        .fit_to_exact_size(egui::vec2(TREE_TOGGLE_SIZE, TREE_TOGGLE_SIZE))
        .tint(tint);
    ui.add_sized(
        [TREE_TOGGLE_SIZE, TREE_TOGGLE_SIZE],
        egui::Button::image(img).frame(false),
    )
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
