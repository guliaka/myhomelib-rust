//! egui rendering for [`crate::gui::state::State`]. Pure egui (no eframe/winit), so it compiles and
//! can be driven headlessly in tests via `egui::Context::default()`.

use egui::{
    Align, CentralPanel, Layout, RichText, ScrollArea, SidePanel, TextEdit, TopBottomPanel, Window,
};

use super::state::{author_display, State, TABS};

/// Render one frame of the main window from `state`.
pub fn render(ctx: &egui::Context, state: &mut State) {
    // Top bar: title + count + actions.
    TopBottomPanel::top("topbar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("MyHomeLib").strong().size(18.));
            ui.separator();
            ui.label(format!("{} books", state.total_books()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Import…").clicked() {
                    state.status = "Import: run `myhomelib import <db> <dir>` (GUI file picker lands in Phase 5)".into();
                }
                if ui.button("Statistics").clicked() {
                    state.show_stats = true;
                }
                if ui.button("Settings").clicked() {
                    state.show_settings = true;
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

    // Central: book list + detail panel.
    CentralPanel::default().show(ctx, |ui| {
        let heading = format!(
            "{} — {} books",
            if state.filters.is_empty() {
                state.selection.label()
            } else {
                "Search".to_string()
            },
            state.books.len()
        );
        ui.label(RichText::new(heading).strong());
        ui.separator();

        let rows: Vec<(String, bool)> = state
            .books
            .iter()
            .enumerate()
            .map(|(i, b)| {
                (
                    format!("{}  [{}] {}", b.title, b.lang.as_deref().unwrap_or("-"), size_str(b.book_size)),
                    state.selected_book == Some(i),
                )
            })
            .collect();

        ScrollArea::vertical().show(ui, |ui| {
            if rows.is_empty() {
                ui.label(RichText::new("No books.").weak());
            } else {
                for (i, (label, is_sel)) in rows.into_iter().enumerate() {
                    if ui.selectable_label(is_sel, label).clicked() {
                        state.select_book(i);
                    }
                }
            }
        });

        // Detail panel for the selected book. Data is gathered into owned locals first so no borrow on
        // `state` is held across the (mutating) Delete button.
        if let Some(i) = state.selected_book {
            if i < state.books.len() {
                let title = state.books[i].title.clone();
                let authors = state
                    .db
                    .book_authors(state.books[i].book_id)
                    .map(|as_| as_.iter().map(author_display).collect::<Vec<String>>())
                    .unwrap_or_default();
                let file = {
                    let b = &state.books[i];
                    format!("{}{}", b.file_name, b.ext.as_ref().map(|e| format!(".{e}")).unwrap_or_default())
                };
                let folder = state.books[i].folder.clone();
                let ann = state.books[i].annotation.clone();

                ui.separator();
                ui.label(RichText::new(title.as_str()).strong().size(16.));
                if !authors.is_empty() {
                    ui.label(format!("Authors: {}", authors.join(", ")));
                }
                ui.label(format!("File: {file}"));
                if let Some(f) = folder {
                    ui.label(format!("Folder: {f}"));
                }
                if let Some(a) = ann {
                    if !a.trim().is_empty() {
                        ui.add_space(4.0);
                        ui.label(RichText::new("Annotation").strong());
                        ui.label(a.as_str());
                    }
                }
                ui.add_space(6.0);
                if ui.button("Delete book").clicked() {
                    state.delete_selected_book();
                }
            }
        }
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
