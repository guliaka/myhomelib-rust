//! egui rendering for [`crate::gui::state::State`]. Pure egui (no eframe/winit), so it compiles and
//! can be driven headlessly in tests via `egui::Context::default()`.

use egui::{
    Align, Align2, CentralPanel, Color32, CornerRadius, Layout, ProgressBar, Rect, RichText, ScrollArea,
    Sense, SidePanel, Stroke, TextEdit, TopBottomPanel, Visuals, Window,
};

use crate::core::Book;

use crate::librusec::queue::ItemStatus;

use super::state::{author_display, State, TABS};

/// Import action for the top-bar button. In the windowed build it opens a native folder picker and
/// imports everything under it; in headless (non-gui) builds it only reports — there's no display to
/// show a dialog from, and tests must never block on one.
#[cfg(feature = "gui")]
fn do_import(state: &mut State) {
    match pick_import_folder() {
        Some(dir) => match state.import_dir(&dir) {
            Ok(_) => {} // status is set by import_dir
            Err(e) => state.status = format!("Import failed: {e}"),
        },
        None => {} // user cancelled the picker
    }
}

#[cfg(not(feature = "gui"))]
fn do_import(state: &mut State) {
    state.status = "Import: pick a folder (the native picker is enabled in the windowed build)".into();
}

/// Native folder picker — the Windows common dialog on the target (no extra DLLs to bundle). Compiled
/// only with the `gui` feature.
#[cfg(feature = "gui")]
fn pick_import_folder() -> Option<std::path::PathBuf> {
    rfd::FileDialog::new()
        .set_title("Choose a folder of FB2/FBD e-books to import")
        .pick_folder()
}

/// Classic light-gray "desktop" styling to approximate the original's native Win32 look. egui draws its
/// own flat widgets (it can't render beveled common controls), but a light-gray chrome + white content
/// areas + boxy (near-zero) rounding + a classic selection blue reads as a conventional desktop app
/// rather than a neon modern one. Applied every frame so it survives any per-frame visual resets.
fn apply_classic_theme(ctx: &egui::Context) {
    let mut v = Visuals::light();
    // Chrome / panels ≈ clBtnFace (#F0F0F0); text fields and content areas white.
    v.panel_fill = Color32::from_rgb(240, 240, 240);
    v.window_fill = Color32::from_rgb(240, 240, 240);
    v.extreme_bg_color = Color32::WHITE;
    v.faint_bg_color = Color32::from_rgb(240, 240, 240);
    v.override_text_color = Some(Color32::BLACK);
    // Boxy controls: near-zero corner radius (the original has no rounded corners).
    let r = CornerRadius::same(1);
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.corner_radius = r;
    v.widgets.active.corner_radius = r;
    // Button faces: light gray, a touch darker on hover/press.
    v.widgets.inactive.bg_fill = Color32::from_rgb(225, 225, 225);
    v.widgets.hovered.bg_fill = Color32::from_rgb(236, 236, 236);
    v.widgets.active.bg_fill = Color32::from_rgb(204, 204, 204);
    // Classic selection blue (the original's FocusedSelectionColor ≈ #0976EA).
    let sel = Color32::from_rgb(9, 118, 234);
    v.selection.bg_fill = sel;
    v.selection.stroke = Stroke::new(1.0_f32, sel);
    ctx.set_visuals(v);
}

/// Render one frame of the main window from `state`.
pub fn render(ctx: &egui::Context, state: &mut State) {
    apply_classic_theme(ctx);
    // Top bar: title + count + actions.
    TopBottomPanel::top("topbar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("MyHomeLib").strong().size(18.));
            ui.separator();
            ui.label(format!("{} books", state.total_books()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Import…").clicked() {
                    do_import(state);
                }
                if ui.button("Statistics").clicked() {
                    state.show_stats = true;
                }
                if ui.button("Settings").clicked() {
                    state.show_settings = true;
                }
                if ui.button("Online").clicked() {
                    state.show_online = true;
                }
            });
        });
    });

    // Search panel: multi-field filters (title / author / lang / ext / local-only).
    TopBottomPanel::top("searchbar").show(ctx, |ui| {
        let prev = state.filters.clone();
        ui.horizontal(|ui| {
            ui.label("Title");
            ui.add(TextEdit::singleline(&mut state.filters.title).desired_width(150.).hint_text("…"));
            ui.separator();
            ui.label("Author");
            ui.add(TextEdit::singleline(&mut state.filters.author).desired_width(150.));
            ui.separator();
            ui.label("Lang");
            ui.add(TextEdit::singleline(&mut state.filters.lang).desired_width(48.).hint_text("ru"));
            ui.separator();
            ui.label("Ext");
            ui.add(TextEdit::singleline(&mut state.filters.ext).desired_width(48.).hint_text("fb2"));
            ui.separator();
            ui.checkbox(&mut state.filters.only_local, "local only");
            if ui.button("Clear").clicked() {
                state.clear_search();
            }
        });
        if state.filters != prev {
            state.reload();
        }
    });

    // Left panel: category tabs + tree.
    SidePanel::left("tree")
        .resizable(true)
        .default_width(260.)
        .min_width(160.)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (i, name) in TABS.iter().enumerate() {
                    if ui.selectable_label(state.tab == i, *name).clicked() {
                        state.set_tab(i);
                    }
                }
            });
            ui.separator();
            let nodes = state.tree_nodes();
            if nodes.is_empty() {
                ui.label(RichText::new("(empty)").weak());
            } else {
                ScrollArea::vertical().show(ui, |ui| {
                    for node in nodes.iter() {
                        let is_sel = node.selection == state.selection;
                        if ui.selectable_label(is_sel, format!("{}  ({})", node.label, node.count)).clicked() {
                            state.select(node.selection.clone());
                        }
                    }
                });
            }
        });

    // Status bar.
    TopBottomPanel::bottom("statusbar").show(ctx, |ui| {
        ui.label(RichText::new(state.status.as_str()).weak());
    });

    // Bottom info band for the selected book — mirrors the original's dark TInfoPanel that sits under the
    // book grid on every tab. Data is gathered into owned locals first so no borrow on `state` is held
    // across the Delete button.
    TopBottomPanel::bottom("bookinfo")
        .min_height(36.0)
        .max_height(170.0)
        .show(ctx, |ui| {
            if let Some(id) = state.selected_book_id {
                if let Some(b) = state.db.get_book(id).ok().flatten() {
                    let title = b.title.clone();
                    let authors = state
                        .db
                        .book_authors(id)
                        .map(|as_| as_.iter().map(author_display).collect::<Vec<String>>())
                        .unwrap_or_default();
                    let file = format!(
                        "{}{}",
                        b.file_name,
                        b.ext.as_ref().map(|e| format!(".{e}")).unwrap_or_default()
                    );
                    let folder = b.folder.clone();
                    let ann = b.annotation.clone();

                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(title.as_str()).strong());
                        if !authors.is_empty() {
                            ui.label(format!("— {}", authors.join(", ")));
                        }
                        ui.label(format!("· {file}"));
                        if let Some(f) = folder {
                            ui.label(format!("· {f}"));
                        }
                    });
                    if let Some(a) = ann {
                        if !a.trim().is_empty() {
                            ScrollArea::vertical().max_height(80.0).show(ui, |ui| {
                                ui.label(a.as_str());
                            });
                        }
                    }
                    if ui.button("Delete book").clicked() {
                        state.delete_selected_book();
                    }
                }
            } else {
                ui.add_space(6.0);
                ui.label(RichText::new("Select a book to view information").weak());
            }
        });

    // Central: heading + virtualized multi-column book grid (the original's TBookTree). Only the visible
    // slice is materialized, so it stays fast and low-memory at any library size (1M+ rows).
    CentralPanel::default().show(ctx, |ui| {
        let heading = format!(
            "{} — {} books",
            if state.filters.is_empty() {
                state.selection.label()
            } else {
                "Search".to_string()
            },
            state.list_total
        );
        ui.label(RichText::new(heading).strong());
        ui.separator();
        render_book_grid(ui, state);
    });

    // Statistics dialog. Data is gathered into an owned struct first; the dialog's open/close state is a
    // local bool so the closure can capture `state` without a borrow conflict with `.open(..)`.
    if state.show_stats {
        let data = state.stats_data();
        let mut open = true;
        Window::new("Collection statistics")
            .open(&mut open)
            .min_size(egui::vec2(340.0, 280.0))
            .show(ctx, |ui| {
                ui.label(format!("Books: {}", data.books));
                ui.label(format!("Authors: {}", data.authors));
                ui.label(format!("Series: {}", data.series));
                ui.label(format!("Genres: {}", data.genres));
                ui.separator();
                ui.label(RichText::new("By language").strong());
                if data.by_lang.is_empty() {
                    ui.label(RichText::new("(none)").weak());
                } else {
                    for (lang, n) in &data.by_lang {
                        ui.horizontal(|ui| {
                            ui.monospace(lang.as_str());
                            ui.label(format!("{n}"));
                        });
                    }
                }
                ui.separator();
                ui.label(RichText::new("By extension").strong());
                if data.by_ext.is_empty() {
                    ui.label(RichText::new("(none)").weak());
                } else {
                    for (ext, n) in &data.by_ext {
                        ui.horizontal(|ui| {
                            ui.monospace(format!(".{ext}"));
                            ui.label(format!("{n}"));
                        });
                    }
                }
            });
        state.show_stats = open;
    }

    // Settings dialog: window size, reader associations (extension → command), last import dir.
    if state.show_settings {
        let mut open = true;
        Window::new("Settings")
            .open(&mut open)
            .min_size(egui::vec2(440.0, 320.0))
            .show(ctx, |ui| {
                let ws = state.settings.window_size();
                ui.horizontal(|ui| {
                    ui.label(format!("Window: {:.0} × {:.0}", ws[0], ws[1]));
                });
                if let Some(dir) = state.settings.last_import_dir() {
                    ui.label(format!("Last import dir: {dir}"));
                }
                ui.separator();
                ui.label(RichText::new("Reader associations (extension → command)").strong());
                let readers = state.settings.readers();
                if readers.is_empty() {
                    ui.label(RichText::new("(none configured)").weak());
                } else {
                    for (ext, cmd) in &readers {
                        ui.horizontal(|ui| {
                            ui.monospace(format!(".{ext}"));
                            ui.separator();
                            ui.label(cmd.as_str());
                        });
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Ext");
                    ui.add(TextEdit::singleline(&mut state.reader_ext).desired_width(50.).hint_text("fb2"));
                    ui.label("Command");
                    ui.add(TextEdit::singleline(&mut state.reader_cmd).desired_width(240.).hint_text("/path/reader %f"));
                });
                if ui.button("Save reader").clicked() {
                    state.apply_reader();
                }
            });
        state.show_settings = open;
    }

    // Online (Librusec) panel: connection config + search + results + download queue.
    state.poll_online();
    if state.show_online {
        let mut open = true;
        Window::new("Online library (Lib.rus.ec)")
            .open(&mut open)
            .min_size(egui::vec2(560.0, 440.0))
            .show(ctx, |ui| {
                ui.collapsing("Connection", |ui| {
                    ui.horizontal(|ui| {
                        ui.label("User");
                        ui.add(TextEdit::singleline(&mut state.lib_user).desired_width(180.).hint_text("login"));
                    });
                    ui.horizontal(|ui| {
                        ui.label("Password");
                        ui.add(TextEdit::singleline(&mut state.lib_pass).desired_width(180.).password(true));
                    });
                    ui.horizontal(|ui| {
                        ui.label("URL");
                        ui.add(TextEdit::singleline(&mut state.lib_url)
                            .desired_width(300.)
                            .hint_text("https://lib.rus.ec/…"));
                    });
                    ui.label("Connection script (one command per line: GET / POST / ADD / CHECK …)");
                    ui.add(TextEdit::multiline(&mut state.lib_script)
                        .desired_width(ui.available_width())
                        .desired_rows(3));
                    if ui.button("Save connection").clicked() {
                        state.save_lib_config();
                    }
                });

                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Search");
                    ui.add(TextEdit::singleline(&mut state.online_query)
                        .desired_width(260.)
                        .hint_text("title or author"));
                    if ui.button("Go").clicked() {
                        state.start_online_search();
                    }
                });

                // Results — gathered into owned rows first so the (mutating) download button doesn't
                // fight a borrow of `state.online_catalog`.
                let result_rows: Vec<(usize, String)> = if let Some(cat) = &state.online_catalog {
                    state
                        .online_results
                        .iter()
                        .filter_map(|&idx| {
                            if idx < cat.books.len() {
                                let b = &cat.books[idx];
                                let author =
                                    b.authors.first().map(|(l, f, _)| format!("{l} {f}")).unwrap_or_default();
                                Some((idx, format!("{} — {}", author.trim(), b.title)))
                            } else {
                                None
                            }
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                if !result_rows.is_empty() {
                    ui.separator();
                    ui.label(RichText::new(format!("{} result(s)", result_rows.len())).strong());
                    ScrollArea::vertical().max_height(150.).show(ui, |ui| {
                        for (idx, label) in &result_rows {
                            ui.horizontal(|ui| {
                                ui.label(label.as_str());
                                if ui.button("↓ download").clicked() {
                                    state.queue_download(*idx);
                                }
                            });
                        }
                    });
                }

                // Download queue.
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Downloads ({})", state.downloads.len())).strong());
                    if ui.add_enabled(state.downloads.pending() > 0, egui::Button::new("Cancel all")).clicked() {
                        state.downloads.cancel_all();
                    }
                });
                let rows: Vec<(u64, String, f64, String, bool)> = state
                    .downloads
                    .iter()
                    .map(|it| {
                        let (kind, frac) = match &it.status {
                            ItemStatus::Queued => ("queued".to_string(), 0.0),
                            ItemStatus::Active { .. } => ("downloading".to_string(), it.status.fraction()),
                            ItemStatus::Done => ("done".to_string(), 1.0),
                            ItemStatus::Failed(e) => (format!("failed: {e}"), 1.0),
                            ItemStatus::Cancelled => ("cancelled".to_string(), 1.0),
                        };
                        let cancellable = matches!(it.status, ItemStatus::Queued | ItemStatus::Active { .. });
                        (it.id, it.label.clone(), frac, kind, cancellable)
                    })
                    .collect();
                if rows.is_empty() {
                    ui.label(RichText::new("(nothing queued)").weak());
                } else {
                    ScrollArea::vertical().max_height(140.).show(ui, |ui| {
                        for (id, label, frac, kind, cancellable) in rows {
                            ui.horizontal(|ui| {
                                if kind == "downloading" {
                                    ui.add(ProgressBar::new(frac as f32).text(format!("{:.0}%", frac * 100.0)));
                                } else {
                                    let color = if kind.starts_with("failed") || kind == "cancelled" {
                                        Color32::RED
                                    } else {
                                        Color32::LIGHT_GRAY
                                    };
                                    ui.colored_label(color, &kind);
                                }
                                ui.label(&label);
                                if cancellable && ui.small_button("cancel").clicked() {
                                    state.downloads.cancel(id);
                                }
                            });
                        }
                    });
                }
            });
        state.show_online = open;
    }
}

/// Human-readable file size.
fn size_str(bytes: Option<i64>) -> String {
    let Some(b) = bytes else {
        return "—".to_string();
    };
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    if b as f64 >= MB {
        format!("{:.1} MB", b as f64 / MB)
    } else if b as f64 >= KB {
        format!("{:.0} KB", b as f64 / KB)
    } else {
        format!("{b} B")
    }
}

/// A column in the book grid: header label, pixel width, and whether its cells are center-aligned.
struct GridCol {
    header: &'static str,
    width: f32,
    center: bool,
}

/// Column layout for the book grid. The narrow columns have fixed widths; "Title" absorbs the leftover
/// panel width so the grid spans the full central area like the original's TBookTree.
fn book_grid_columns(available: f32) -> Vec<GridCol> {
    let fixed = 60.0 + 45.0 + 78.0 + 64.0; // language, #, size, rating
    let title_w = (available - fixed).max(180.0);
    vec![
        GridCol { header: "Title", width: title_w, center: false },
        GridCol { header: "Lang", width: 60.0, center: true },
        GridCol { header: "#", width: 45.0, center: true },
        GridCol { header: "Size", width: 78.0, center: true },
        GridCol { header: "Rating", width: 64.0, center: true },
    ]
}

/// Cell text for a book, in the same order as [`book_grid_columns`].
fn book_cell_texts(b: &Book) -> Vec<String> {
    vec![
        b.title.clone(),
        b.lang.clone().unwrap_or_else(|| "-".to_string()),
        b.seq_number.map(|n| n.to_string()).unwrap_or_else(|| "—".to_string()),
        size_str(b.book_size),
        if b.rate > 0 { b.rate.to_string() } else { "—".to_string() },
    ]
}

/// Render the virtualized multi-column book grid: a fixed header row, then only the visible slice of
/// rows (via `show_rows`), each drawn as clipped fixed-width cells. Selection clicks are applied to
/// `state`. Stays O(visible rows) at any library size (1M+).
fn render_book_grid(ui: &mut egui::Ui, state: &mut State) {
    let cols = book_grid_columns(ui.available_width());
    let total_w: f32 = cols.iter().map(|c| c.width).sum();
    let row_h = ui.text_style_height(&egui::TextStyle::Body);

    // Header row (non-scrolling) with a bottom rule.
    {
        let (hrect, _hresp) = ui.allocate_exact_size(egui::vec2(total_w, row_h), Sense::hover());
        let origin = hrect.min;
        let font_id = egui::TextStyle::Body.resolve(ui.ctx().style().as_ref());
        let mut x = 0.0_f32;
        for c in &cols {
            let cell = Rect::from_min_size(
                egui::pos2(origin.x + x, origin.y),
                egui::vec2(c.width, row_h),
            );
            let (anchor_pos, anchor) = if c.center {
                (cell.center(), Align2::CENTER_CENTER)
            } else {
                (cell.min, Align2::LEFT_CENTER)
            };
            ui.painter()
                .with_clip_rect(cell)
                .text(anchor_pos, anchor, c.header, font_id.clone(), Color32::BLACK);
            x += c.width;
        }
        let y = origin.y + row_h;
        ui.painter().line_segment(
            [egui::pos2(origin.x, y), egui::pos2(origin.x + total_w, y)],
            Stroke::new(1.0_f32, Color32::from_rgb(160, 160, 160)),
        );
    }

    let total = state.list_total as usize;
    if total == 0 {
        ui.add_space(4.0);
        ui.label(RichText::new("No books.").weak());
        return;
    }
    let grid_row_h = row_h + 3.0;
    ScrollArea::vertical().show_rows(ui, grid_row_h, total, |ui, range| {
        // Materialize only the visible slice (cached) — O(visible rows), not O(library).
        let win = state.books_range(range.start as i64, (range.end - range.start) as i64);
        for b in win.iter() {
            let is_sel = state.selected_book_id == Some(b.book_id); // shared borrow; ends here
            if render_book_row(ui, b, &cols, is_sel) {
                state.select_book_id(b.book_id); // mutable borrow
            }
        }
    });
}

/// Draw one grid row as clipped fixed-width cells with a full-row selection/hover highlight (the
/// original uses toFullRowSelect). Returns true if the row was clicked.
fn render_book_row(ui: &mut egui::Ui, b: &Book, cols: &[GridCol], is_selected: bool) -> bool {
    let row_h = ui.text_style_height(&egui::TextStyle::Body) + 3.0;
    let total_w: f32 = cols.iter().map(|c| c.width).sum();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(total_w, row_h), Sense::click());

    if is_selected {
        ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(9, 118, 234));
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(229, 243, 255));
    }

    let font_id = egui::TextStyle::Body.resolve(ui.ctx().style().as_ref());
    let text_color = if is_selected { Color32::WHITE } else { Color32::BLACK };
    let texts = book_cell_texts(b);
    let mut x = 0.0_f32;
    for (c, t) in cols.iter().zip(texts.iter()) {
        let cell = Rect::from_min_size(
            egui::pos2(rect.min.x + x, rect.min.y),
            egui::vec2(c.width, row_h),
        );
        let (anchor_pos, anchor) = if c.center {
            (cell.center(), Align2::CENTER_CENTER)
        } else {
            (cell.min, Align2::LEFT_CENTER)
        };
        ui.painter()
            .with_clip_rect(cell)
            .text(anchor_pos, anchor, t, font_id.clone(), text_color);
        x += c.width;
    }
    resp.clicked()
}
