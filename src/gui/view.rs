//! egui rendering for [`crate::gui::state::State`]. Pure egui (no eframe/winit), so it compiles and
//! can be driven headlessly in tests via `egui::Context::default()`.

use egui::{Align, CentralPanel, Layout, RichText, ScrollArea, SidePanel, TextEdit, TopBottomPanel};

use super::state::{author_display, State, TABS};

/// Render one frame of the main window from `state`.
pub fn render(ctx: &egui::Context, state: &mut State) {
    // Top bar: title + search + count + import.
    TopBottomPanel::top("topbar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("MyHomeLib").strong().size(18.));
            let prev = state.search.clone();
            ui.add(TextEdit::singleline(&mut state.search).desired_width(240.).hint_text("Search title…"));
            if state.search != prev {
                state.reload();
            }
            ui.separator();
            ui.label(format!("{} books", state.total_books()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Import…").clicked() {
                    state.status = "Import: run `myhomelib import <db> <dir>` (GUI file picker lands in Phase 5)".into();
                }
            });
        });
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
                        state.tab = i;
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
        ui.label(RichText::new(format!("{} — {} books", state.selection.label(), state.books.len())).strong());
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
