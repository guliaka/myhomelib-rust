//! egui rendering for [`crate::gui::state::State`]. Pure egui (no eframe/winit), so it compiles and
//! can be driven headlessly in tests via `egui::Context::default()`.

use egui::{
    Align2, CentralPanel, Color32, CornerRadius, ProgressBar, Rect, RichText, ScrollArea,
    Sense, SidePanel, Stroke, TextEdit, TopBottomPanel, Visuals, Window,
};

use std::collections::HashMap;

use crate::core::Book;

use crate::librusec::queue::ItemStatus;

use super::state::{author_display, Selection, State, TABS};

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

/// Which action a toolbar button performs. Most mirror the original's view/collection toggles, which are
/// not yet wired in the port — they show their (original) tooltip; only a few map to real actions.
enum TbarAction {
    None,
    SelectAll,
    Settings,
}

/// One toolbar button: its bundled icon + original tooltip + dropdown flag + action.
struct TbarButton {
    name: &'static str,
    tip: &'static str,
    bytes: &'static [u8],
    dropdown: bool,
    action: TbarAction,
}

/// The toolbar in the original's exact order (16 buttons). Separators are drawn after the indices in
/// [`TBAR_SEP_AFTER`]. Icons are the original's bundled 32x32 PNGs.
const TBAR: [TbarButton; 16] = [
    TbarButton { name: "read", tip: "Читать", bytes: include_bytes!("../../assets/icons-32/read1.png"), dropdown: false, action: TbarAction::None },
    TbarButton { name: "toDownloadList", tip: "Добавить в список загрузок", bytes: include_bytes!("../../assets/icons-32/toDownloadList.png"), dropdown: false, action: TbarAction::None },
    TbarButton { name: "sndToDevice", tip: "Отправить на устройство", bytes: include_bytes!("../../assets/icons-32/sndToDevice1.png"), dropdown: true, action: TbarAction::None },
    TbarButton { name: "rusAlphabet", tip: "Кириллическая азбука", bytes: include_bytes!("../../assets/icons-32/ru1.png"), dropdown: false, action: TbarAction::None },
    TbarButton { name: "engAlphabet", tip: "Латинская азбука", bytes: include_bytes!("../../assets/icons-32/en1.png"), dropdown: false, action: TbarAction::None },
    TbarButton { name: "selectCollection", tip: "Выбрать коллекцию", bytes: include_bytes!("../../assets/icons-32/selectCollection1.png"), dropdown: true, action: TbarAction::None },
    TbarButton { name: "addFavorite", tip: "Добавить в избранное", bytes: include_bytes!("../../assets/icons-32/toFavorites1.png"), dropdown: true, action: TbarAction::None },
    TbarButton { name: "selectAll", tip: "Отметить все", bytes: include_bytes!("../../assets/icons-32/chekAll1.png"), dropdown: false, action: TbarAction::SelectAll },
    TbarButton { name: "collapseList", tip: "Развернуть/Свернуть список", bytes: include_bytes!("../../assets/icons-32/refreshList1.png"), dropdown: false, action: TbarAction::None },
    TbarButton { name: "tableMode", tip: "Переключить в режим \"таблица\"", bytes: include_bytes!("../../assets/icons-32/tableView1.png"), dropdown: false, action: TbarAction::None },
    TbarButton { name: "hideDeleted", tip: "Скрыть удалённые книги", bytes: include_bytes!("../../assets/icons-32/deletedBooks1.png"), dropdown: false, action: TbarAction::None },
    TbarButton { name: "localOnly", tip: "Показывать только загруженные книги", bytes: include_bytes!("../../assets/icons-32/local_files.png"), dropdown: false, action: TbarAction::None },
    TbarButton { name: "infoPanel", tip: "Показать/скрыть панель информации о книге", bytes: include_bytes!("../../assets/icons-32/infoPanel1.png"), dropdown: false, action: TbarAction::None },
    TbarButton { name: "updateCollections", tip: "Обновить коллекции", bytes: include_bytes!("../../assets/icons-32/update_collections.png"), dropdown: false, action: TbarAction::None },
    TbarButton { name: "settings", tip: "Настройки приложения", bytes: include_bytes!("../../assets/icons-32/settings1.png"), dropdown: false, action: TbarAction::Settings },
    TbarButton { name: "help", tip: "Помощь", bytes: include_bytes!("../../assets/icons-32/help.png"), dropdown: false, action: TbarAction::None },
];

/// Toolbar separator positions (0-based index of the button AFTER which a separator is drawn).
const TBAR_SEP_AFTER: [usize; 7] = [2, 4, 5, 6, 8, 12, 13];

/// Per-context cache of decoded+uploaded toolbar-icon textures. egui's texture manager allocates a NEW id
/// on every `load_texture`, so we load each icon once per context and reuse the handle afterwards.
pub struct IconCache {
    handles: HashMap<&'static str, egui::TextureHandle>,
}

impl Default for IconCache {
    fn default() -> Self {
        Self { handles: HashMap::new() }
    }
}

/// Get (decoding + uploading once per context) the texture handle for a bundled toolbar icon.
fn icon_texture(ctx: &egui::Context, cache: &mut IconCache, name: &'static str, bytes: &[u8]) -> egui::TextureHandle {
    if let Some(h) = cache.handles.get(name) {
        return h.clone();
    }
    let color_image = decode_icon(bytes);
    let handle = ctx.load_texture(name, color_image, egui::TextureOptions::LINEAR);
    cache.handles.insert(name, handle.clone());
    handle
}

/// Decode a bundled toolbar-icon PNG into an egui image. The `image` crate is gui-only, so headless
/// (non-gui) builds render a neutral placeholder instead — the toolbar layout still renders in tests.
#[cfg(feature = "gui")]
fn decode_icon(bytes: &[u8]) -> egui::ColorImage {
    match image::load_from_memory(bytes) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba.into_raw())
        }
        Err(_) => icon_placeholder(),
    }
}

#[cfg(not(feature = "gui"))]
fn decode_icon(_bytes: &[u8]) -> egui::ColorImage {
    icon_placeholder()
}

/// A neutral 16x16 gray block used when an icon can't be (or needn't be) decoded.
fn icon_placeholder() -> egui::ColorImage {
    let rgb = vec![180u8; 16 * 16 * 3];
    egui::ColorImage::from_rgb([16, 16], &rgb)
}

/// Render one frame of the main window from `state` (and its per-context icon cache).
pub fn render(ctx: &egui::Context, state: &mut State, icons: &mut IconCache) {
    apply_classic_theme(ctx);
    // Menu bar — the original's TMainMenu (Книга / Коллекция / Группа / Редактирование / Вид / Инструменты / Помощь).
    TopBottomPanel::top("menubar").show(ctx, |ui| {
        render_menu_bar(ui, state);
    });

    // Icon toolbar — the original's TToolBar: its exact set of icon buttons in order, grouped by
    // separators, each with its original tooltip. Icons are the original's bundled 32x32 PNGs.
    TopBottomPanel::top("toolbar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            for (i, btn) in TBAR.iter().enumerate() {
                let handle = icon_texture(ctx, icons, btn.name, btn.bytes);
                let resp = ui.add_sized(egui::vec2(30.0, 30.0), egui::Image::new(&handle).sense(Sense::click()));
                if btn.dropdown {
                    // small down-arrow at the button's right edge (the original's tbsDropDown buttons)
                    let r = resp.rect;
                    let ax = r.max.x - 2.0;
                    let ay = r.center().y;
                    let color = Color32::from_rgb(90, 90, 90);
                    ui.painter().add(egui::Shape::Path(egui::epaint::PathShape {
                        points: vec![
                            egui::pos2(ax - 3.0, ay - 2.0),
                            egui::pos2(ax + 1.0, ay - 2.0),
                            egui::pos2(ax - 1.0, ay + 2.0),
                        ],
                        closed: true,
                        fill: color,
                        stroke: egui::epaint::PathStroke::from(Stroke::new(1.0_f32, color)),
                    }));
                }
                if resp.clicked() {
                    match btn.action {
                        TbarAction::SelectAll => state.select(Selection::All),
                        TbarAction::Settings => state.show_settings = true,
                        TbarAction::None => {}
                    }
                }
                resp.on_hover_text(btn.tip);
                if TBAR_SEP_AFTER.contains(&i) {
                    ui.separator();
                }
            }
        });
    });

    // Page-control tab bar — the original's TPageControl (Авторы / Серии / Жанры / Поиск / Группы / Список загрузок).
    TopBottomPanel::top("tabbar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            for (i, name) in TABS.iter().enumerate() {
                if ui.selectable_label(state.tab == i, *name).clicked() {
                    state.set_tab(i);
                }
            }
        });
    });

    // Left nav tree — shown only on the category tabs (Авторы / Серии / Жанры / Группы).
    if state.is_category_tab() {
        SidePanel::left("nav")
            .resizable(true)
            .default_width(260.)
            .min_width(160.)
            .show(ctx, |ui| {
                let nodes = state.tree_nodes();
                if nodes.is_empty() {
                    ui.label(RichText::new("(пусто)").weak());
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
    }

    // Status bar — the original's TStatusBar.
    TopBottomPanel::bottom("statusbar").show(ctx, |ui| {
        ui.label(RichText::new(state.status.as_str()).weak());
    });

    // Bottom info band for the selected book — mirrors the original's dark TInfoPanel that sits under the
    // book grid on every tab (hidden on the download-queue tab). Data is gathered into owned locals first
    // so no borrow on `state` is held across the Delete button.
    if !state.is_downloads_tab() {
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
                    let genres = state.db.book_genres(id).unwrap_or_default();

                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(title.as_str()).strong());
                        if !authors.is_empty() {
                            ui.label(format!("— {}", authors.join(", ")));
                        }
                        if !genres.is_empty() {
                            ui.label(format!("· {}", genres.join(", ")));
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
    }

    // Central content — depends on the active tab. Category tabs show a heading + the virtualized
    // multi-column book grid (the original's TBookTree); the Поиск tab shows the search form + results;
    // the download tab shows the queue table. Only the visible slice is ever materialized, so it stays
    // fast and low-memory at any library size (1M+ rows).
    CentralPanel::default().show(ctx, |ui| {
        if state.is_search_tab() {
            render_search_view(ui, state);
        } else if state.is_downloads_tab() {
            render_downloads_view(ui, state);
        } else {
            let heading = format!("{} — {} книг", TABS[state.tab], state.list_total);
            ui.label(RichText::new(heading).strong());
            ui.separator();
            render_book_grid(ui, state);
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

/// How a grid column's cells are drawn.
#[derive(Clone, Copy, PartialEq)]
enum ColKind {
    Text,
    /// The rating column — drawn as five vector stars (the original's "Моя оценка").
    Stars,
}

/// A column in the book grid: header label, pixel width, alignment, and how its cells render.
struct GridCol {
    header: &'static str,
    width: f32,
    center: bool,
    kind: ColKind,
}

/// Column layout for the book grid — the original's set/order: Title (no header) / № / Размер /
/// Моя оценка / Добавлено / Жанр. The narrow columns have fixed widths; "Title" absorbs the leftover
/// panel width. If the total exceeds the panel, the grid scrolls horizontally like the original.
fn book_grid_columns(available: f32) -> Vec<GridCol> {
    let fixed = 45.0 + 78.0 + 92.0 + 92.0 + 210.0; // №, size, rating(stars), added-date, genre
    let title_w = (available - fixed).max(180.0);
    vec![
        GridCol { header: "", width: title_w, center: false, kind: ColKind::Text },
        GridCol { header: "№", width: 45.0, center: true, kind: ColKind::Text },
        GridCol { header: "Размер", width: 78.0, center: true, kind: ColKind::Text },
        GridCol { header: "Моя оценка", width: 92.0, center: true, kind: ColKind::Stars },
        GridCol { header: "Добавлено", width: 92.0, center: false, kind: ColKind::Text },
        GridCol { header: "Жанр", width: 210.0, center: false, kind: ColKind::Text },
    ]
}

/// Cell text for a book, in the same order as [`book_grid_columns`]. The rating column returns an empty
/// string — it is drawn as stars by [`render_book_row`], not as text.
fn book_cell_texts(b: &Book, genre: &str) -> Vec<String> {
    vec![
        b.title.clone(),
        b.seq_number.map(|n| n.to_string()).unwrap_or_else(|| "—".to_string()),
        size_str(b.book_size),
        String::new(), // rating column — drawn as stars, not text
        // update_date is a formatted string (e.g. "2026-01-01 00:00:00"); show the date part only.
        b.update_date.split(' ').next().unwrap_or("").to_string(),
        genre.to_string(),
    ]
}

/// Render the virtualized multi-column book grid: a fixed header row, then only the visible slice of
/// rows (via `show_rows`), each drawn as clipped fixed-width cells. Selection clicks are applied to
/// `state`. Stays O(visible rows) at any library size (1M+).
fn render_book_grid(ui: &mut egui::Ui, state: &mut State) {
    let cols = book_grid_columns(ui.available_width());
    let total_w: f32 = cols.iter().map(|c| c.width).sum();
    let row_h = ui.text_style_height(&egui::TextStyle::Body);

    // Horizontal scroll so the Жанр column can extend past a narrow window, like the original. Inside,
    // the header + vertical `show_rows` grid share the same (scrollable) width.
    ScrollArea::horizontal().show(ui, |ui| {
        // Header row (scrolls horizontally with the body) with a bottom rule.
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
                if !c.header.is_empty() {
                    let (anchor_pos, anchor) = if c.center {
                        (cell.center(), Align2::CENTER_CENTER)
                    } else {
                        (cell.min, Align2::LEFT_CENTER)
                    };
                    ui.painter()
                        .with_clip_rect(cell)
                        .text(anchor_pos, anchor, c.header, font_id.clone(), Color32::BLACK);
                }
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
            // Genre labels for this window (one batch query per scroll position; cached in state).
            let genres: HashMap<i64, String> = state.genre_map().clone();
            for b in win.iter() {
                let g = genres.get(&b.book_id).map(|s| s.as_str()).unwrap_or("");
                let is_sel = state.selected_book_id == Some(b.book_id); // shared borrow; ends here
                if render_book_row(ui, b, &cols, is_sel, g) {
                    state.select_book_id(b.book_id); // mutable borrow
                }
            }
        });
    });
}

/// Draw one grid row as clipped fixed-width cells with a full-row selection/hover highlight (the
/// original uses toFullRowSelect). Returns true if the row was clicked.
fn render_book_row(ui: &mut egui::Ui, b: &Book, cols: &[GridCol], is_selected: bool, genre: &str) -> bool {
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
    let texts = book_cell_texts(b, genre);
    let mut x = 0.0_f32;
    for (c, t) in cols.iter().zip(texts.iter()) {
        let cell = Rect::from_min_size(
            egui::pos2(rect.min.x + x, rect.min.y),
            egui::vec2(c.width, row_h),
        );
        if c.kind == ColKind::Stars {
            draw_stars(ui.painter(), cell, b.rate, is_selected);
        } else {
            let (anchor_pos, anchor) = if c.center {
                (cell.center(), Align2::CENTER_CENTER)
            } else {
                (cell.min, Align2::LEFT_CENTER)
            };
            ui.painter()
                .with_clip_rect(cell)
                .text(anchor_pos, anchor, t, font_id.clone(), text_color);
        }
        x += c.width;
    }
    resp.clicked()
}

/// Draw a 5-star rating (the original's "Моя оценка") as small vector stars — filled for rated, outline
/// for unrated. Vector shapes (not glyphs) so it renders regardless of the font's symbol coverage.
fn draw_stars(painter: &egui::Painter, cell: Rect, rate: i32, selected: bool) {
    const N: usize = 5;
    let r_outer = 5.0_f32;
    let gap = 4.0_f32;
    let step = r_outer * 2.0 + gap;
    let total_w = (N as f32 - 1.0) * step + r_outer * 2.0;
    let cy = cell.center().y;
    let x0 = cell.center().x - total_w / 2.0;
    let fill = if selected { Color32::WHITE } else { Color32::from_rgb(255, 196, 0) };
    let outline = if selected { Color32::WHITE } else { Color32::from_rgb(160, 160, 160) };
    let rated = rate.clamp(0, 5) as usize;
    for i in 0..N {
        let cx = x0 + r_outer + i as f32 * step;
        // A closed PathShape tessellates the concave star correctly (a convex polygon would not).
        let pts = star_points(cx, cy, r_outer);
        if i < rated {
            painter.add(egui::Shape::Path(egui::epaint::PathShape {
                points: pts,
                closed: true,
                fill,
                stroke: egui::epaint::PathStroke::from(Stroke::new(1.0_f32, fill)),
            }));
        } else {
            painter.add(egui::Shape::Path(egui::epaint::PathShape {
                points: pts,
                closed: true,
                fill: Color32::TRANSPARENT,
                stroke: egui::epaint::PathStroke::from(Stroke::new(1.0_f32, outline)),
            }));
        }
    }
}

/// The 10 vertices (outer/inner alternating) of a 5-pointed star centered at `(cx, cy)`, point-up.
fn star_points(cx: f32, cy: f32, r_outer: f32) -> Vec<egui::Pos2> {
    let r_inner = r_outer * 0.4;
    (0..10)
        .map(|i| {
            let ang = (-90.0_f32 + i as f32 * 36.0).to_radians();
            let r = if i % 2 == 0 { r_outer } else { r_inner };
            egui::pos2(cx + ang.cos() * r, cy + ang.sin() * r)
        })
        .collect()
}

/// The main menu bar (the original's TMainMenu). Each top-level item opens a dropdown wired to the same
/// `State` methods the toolbar buttons use. In headless tests the menus never open, so only the buttons
/// render (no dialogs, no blocking).
fn render_menu_bar(ui: &mut egui::Ui, state: &mut State) {
    ui.horizontal(|ui| {
        ui.menu_button("Книга", |ui| {
            if ui.button("Импорт…").clicked() {
                do_import(state);
            }
            if ui.button("Удалить книгу").clicked() {
                state.delete_selected_book();
            }
        });
        ui.menu_button("Коллекция", |ui| {
            if ui.button("Статистика").clicked() {
                state.show_stats = true;
            }
            if ui.button("Настройки").clicked() {
                state.show_settings = true;
            }
        });
        ui.menu_button("Группа", |ui| {
            if ui.button("Обновить").clicked() {
                state.reload();
            }
        });
        ui.menu_button("Редактирование", |ui| {
            if ui.button("Настройки").clicked() {
                state.show_settings = true;
            }
        });
        ui.menu_button("Вид", |ui| {
            for (i, name) in TABS.iter().enumerate() {
                if ui.button(*name).clicked() {
                    state.set_tab(i);
                }
            }
        });
        ui.menu_button("Инструменты", |ui| {
            if ui.button("Онлайн-библиотека").clicked() {
                state.show_online = true;
            }
            if ui.button("Импорт…").clicked() {
                do_import(state);
            }
        });
        ui.menu_button("Помощь", |ui| {
            if ui.button("О программе").clicked() {
                state.status = "MyHomeLib (Rust) — порт оригинального MyHomeLib".into();
            }
        });
    });
}

/// The Поиск tab: a multi-field search form (moved here from the old top bar, matching the original) plus
/// the virtualized results grid.
fn render_search_view(ui: &mut egui::Ui, state: &mut State) {
    let prev = state.filters.clone();
    ui.horizontal_wrapped(|ui| {
        ui.label("Название");
        ui.add(TextEdit::singleline(&mut state.filters.title).desired_width(160.).hint_text("…"));
        ui.separator();
        ui.label("Автор");
        ui.add(TextEdit::singleline(&mut state.filters.author).desired_width(160.));
        ui.separator();
        ui.label("Язык");
        ui.add(TextEdit::singleline(&mut state.filters.lang).desired_width(48.).hint_text("ru"));
        ui.separator();
        ui.label("Файл");
        ui.add(TextEdit::singleline(&mut state.filters.ext).desired_width(48.).hint_text("fb2"));
        ui.separator();
        ui.checkbox(&mut state.filters.only_local, "только локальные");
    });
    if ui.button("Очистить").clicked() {
        state.clear_search();
    }
    if state.filters != prev {
        state.reload();
    }
    ui.separator();
    if state.list_total == 0 && state.filters.is_empty() {
        ui.label(RichText::new("Введите условия поиска").weak());
    } else {
        render_book_grid(ui, state);
    }
}

/// The Список загрузок tab: the download queue (title / progress / status), mirroring the original's
/// queue columns. Rows are gathered into owned data first so the cancel button can mutate `state` without
/// a borrow conflict.
fn render_downloads_view(ui: &mut egui::Ui, state: &mut State) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("Список загрузок ({})", state.downloads.len())).strong());
        if ui.add_enabled(state.downloads.pending() > 0, egui::Button::new("Отменить все")).clicked() {
            state.downloads.cancel_all();
        }
    });
    ui.separator();
    let rows: Vec<(u64, String, f64, String, bool)> = state
        .downloads
        .iter()
        .map(|it| {
            let (kind, frac) = match &it.status {
                ItemStatus::Queued => ("в очереди".to_string(), 0.0),
                ItemStatus::Active { .. } => ("загрузка".to_string(), it.status.fraction()),
                ItemStatus::Done => ("готово".to_string(), 1.0),
                ItemStatus::Failed(e) => (format!("ошибка: {e}"), 1.0),
                ItemStatus::Cancelled => ("отменено".to_string(), 1.0),
            };
            let cancellable = matches!(it.status, ItemStatus::Queued | ItemStatus::Active { .. });
            (it.id, it.label.clone(), frac, kind, cancellable)
        })
        .collect();
    if rows.is_empty() {
        ui.label(RichText::new("(нет загрузок)").weak());
    } else {
        ScrollArea::vertical().show(ui, |ui| {
            for (id, label, frac, kind, cancellable) in rows {
                ui.horizontal(|ui| {
                    if kind == "загрузка" {
                        ui.add(ProgressBar::new(frac as f32).text(format!("{:.0}%", frac * 100.0)));
                    } else {
                        let color = if kind.starts_with("ошибка") || kind == "отменено" {
                            Color32::RED
                        } else {
                            Color32::LIGHT_GRAY
                        };
                        ui.colored_label(color, &kind);
                    }
                    ui.label(&label);
                    if cancellable && ui.small_button("отменить").clicked() {
                        state.downloads.cancel(id);
                    }
                });
            }
        });
    }
}
