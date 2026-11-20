# MyHomeLib → Rust: Porting Notes

Living document. Updated at every phase boundary. Anything deferred or changed is recorded here
with a reason — nothing is dropped silently.

- **Original:** https://github.com/guliaka/MyHomeLib.git (FreePascal/Lazarus, ~122 `.pas`, ~47.5k LOC)
- **Target:** Windows 10 x86_64, single self-contained `.exe`, cold start < 10 s on a large library.
- **Stack:** egui/eframe (GUI) · rusqlite (bundled) · quick-xml (FB2/FBD) · reqwest (Librusec).

---

## Phase status

| Phase | Scope | Status | Notes |
|-------|-------|--------|-------|
| 1 | Skeleton + data layer (`core/`, `db/`) | ✅ done | rusqlite DAO, schema init, UDF/collation registration, CRUD + search + pagination. `cargo test` green; CLI smoke lists books. |
| 2 | FB2/FBD engine (`fb2/`) | ✅ done | Lossless element-tree model (quick-xml parse, hand-rolled serialize), typed accessors (title/authors/genre/lang/annotation/cover), FBD = ZIP read/write. `cargo test` green incl. round-trip. |
| 3 | Headless import/export (`import.rs`, `export.rs`) | ✅ done | Folder + FBD import (extract title/authors/genre/lang/series/annotation, dedupe by source-file identity), metadata export to FB2. INPX import, device export, and background threads/progress deferred to the GUI phases. `cargo test` green incl. import/export round-trip. |
| 4 | GUI shell + tree navigation (egui) | ✅ done | egui main window: category tabs (Authors/Genres/Series/Folders) + node tree with counts, book list, detail panel, search box, soft-delete. State is pure Rust (headless-tested); view is pure egui; eframe runner behind the `gui` feature. Add/edit dialogs deferred to Phase 5. |
| 5 | Search UI, settings, stats | ⬜ todo | Search presets, persisted settings, reader associations, statistics view. |
| 6 | Librusec client + download queue (`librusec/`) | ⬜ todo | Remote browse/search, queued downloads with progress/cancel/resume. |
| 7 | Polish + performance | ⬜ todo | Startup profiling (<10 s), lazy loading, Windows cross-build verified. |

---

## Decisions

### D1 — Register the original's SQLite UDFs and collation (do not strip them)
The original schema uses triggers that call `MHL_UPPER`, `MHL_FULLNAME`, `MHL_TRIGGERS_ON` and
columns/indexes declared `COLLATE MHL_SYSTEM_NOCASE`. **Existing user `.db` files already contain
these triggers**, so a Rust process must provide the same functions or any write/DDL fails.

→ We register all of them in `db/connection.rs` on open. Fresh collections are initialized from the
embedded schema; existing DBs open untouched and behave identically to the original.

### D2 — Compute `Search*` columns in-app *and* keep triggers on
The DAO writes the seven `Books.Search*` / `Authors.SearchName` / `Series.SearchSeriesTitle`
columns explicitly (via `core::search`), so correctness does not depend on trigger state. The
triggers still fire and write the same values — belt and suspenders, and it keeps existing-DB
compatibility exact. `Db::set_triggers_enabled(false)` is available to skip them during bulk imports.

### D3 — GUI = egui/eframe
Immediate-mode, pure Rust, single static binary, strong fit for a dense tree/table/detail app. The
entire UI sits behind one module so it can be swapped (iced/slint/Tauri) without touching data/logic.
*(Chosen to match the stack already validated in the prior interactive-verification run.)*

### D4 — FB2 model = lossless element tree + typed accessors (not a rigid struct per tag)
A generic `Element`/`Node` tree preserves **every** element, attribute and text run exactly as parsed,
so parse → serialize → parse is lossless by construction (verified by test). Typed accessors on
`Fb2` (`book_title`, `authors`, `genre`, `lang`, `annotation`, `cover`, `binary`, `body_sections`)
give the app the fields it imports without sacrificing fidelity for exotic markup. Serialization is
hand-rolled (no quick-xml Writer) so output is fully controlled and re-parses to an identical tree.
Dropped on parse (not book content): XML declaration, comments, processing instructions, CDATA,
doctype, general entity refs. FBD is a plain ZIP of `.fb2` entries + cover image(s), read/written via
the `zip` crate (`SimpleFileOptions`).

### D5 — Import dedupe key = source-file identity (folder + file name + position in container)
Matches the original's `InsertBook` (`unit_Database_SQLite.pas`): re-importing the same physical file is
a no-op, while a renamed/moved file creates a new entry. We do **not** dedupe on title+author — that would
silently drop legitimately distinct books and diverge from the original. `Importer` stores
`Folder/FileName/InsideNo`; FBD members get a distinct `InsideNo` per position in the container, so a
multi-book FBD is tracked as one source file with several catalog entries.

### D6 — Export is metadata-only
The DB stores catalog metadata, not body text (that lives in the source file). `Exporter::book_to_fb2`
rebuilds an FB2 from title-info + annotation only — for catalog exchange and round-trip verification, not
a full-content backup. Re-reading the source file to export full bodies is deferred with the GUI.

### D7 — GUI = pure-egui state/view + thin eframe runner behind a `gui` feature
The navigation logic (`gui::state`) depends only on the data layer (no egui), and the rendering
(`gui::view`) depends only on pure `egui` — so both compile and are unit-tested headlessly with no display.
Only the window runner (`gui::app` + the `myhomelib-gui` binary) needs `eframe`/winit/GL, gated behind an
optional `gui` feature (off by default). This keeps `cargo build`/`cargo test` working on headless CI/dev
boxes while still producing the native window on a display or the Windows target. egui/eframe are pinned to
**0.31** — the version validated in the prior interactive run and the last release with the classic
`.show(ctx, |ui| …)` panel API (0.37 reworked panels onto a `&mut Ui`).

---

## Faithful behavior recovered from the original source

These were read from the FreePascal source, not guessed — implementers should preserve them:

- **`Author.SearchName`** = `UPPERCASE(FormatName(Last, First, Middle))`.
  - `FormatName` (`unit_FB2Utils.pas`): `Result := Last; if First<>'' then Result += ' '+First; if Middle<>'' then Result += ' '+Middle;` (nickName and onlyInitials are unused on the UDF path). Preserves a leading space when `Last` is empty — kept as-is.
- **`MHL_UPPER` / `MHL_LOWER`** = Unicode upper/lowercase (`SystemUpperString`). Rust port uses `to_uppercase()`/`to_lowercase()`, which (unlike SQLite's built-in) handles Cyrillic.
- **`MHL_SYSTEM_NOCASE`** collation = Unicode case-insensitive compare (the built-in `NOCASE` is ASCII-only). Registered so Cyrillic identifiers sort/compare like the original.
- **Soft delete:** books carry an `IsDeleted` flag; views filter `IsDeleted = 0` by default. Hard deletes cascade via triggers (book → its author/genre links; orphan series cleanup).

---

## Known deltas / open questions (resolve or confirm per phase)

- **Built-in FB2 genre tree** (`InternalLoadGenres` in the original) is **not yet ported**. Import now
  populates `Genres` from each book's raw FB2 `<genre>` code (upserted by code), so a collection grows its
  genres as books are imported — but a *fresh empty* collection still has none until something is imported.
  *Action: embed the original's default-genre seed file (Phase 6 or a data file) for parity on new installs.*
- **`UpdateDate` format** is stored as a `VARCHAR(23)` string; kept verbatim for compatibility. Confirm
  the exact format string before any date re-formatting in the GUI (Phase 4/5).
- **`Review` column** is a BLOB in the original (possibly non-UTF-8); carried as `Option<Vec<u8>>`.
  Confirm encoding before rendering it (Phase 4).
- **`journal_mode = OFF`** comes from the original schema. It favors speed at the cost of crash safety;
  revisit for the Rust app in Phase 7 (WAL is a candidate) — but only after confirming no behavior depends on it.

---

## Verification log

- **Phase 1** — `cargo test` (integration: schema init idempotency, author/series dedup, search-column
  population via UDFs, paginated title/author search, soft-delete filtering). CLI smoke:
  `myhomelib init demo.db --demo && myhomelib books demo.db`.
- **Phase 2** — `cargo test` (fb2: typed extraction of title/authors/genre/lang/annotation/cover on a
  Cyrillic fixture with nested sections + inline markup; lossless parse→serialize→parse tree equality;
  FBD write→open round-trip preserving the book and cover). 6 fb2 tests + 5 dao tests all green.
- **Phase 3** — `cargo test` (import_export: folder + FBD import extracting title/authors/genre/lang/series/
  annotation, source-file dedupe so a re-import adds nothing, and export that round-trips metadata back
  through the FB2 engine). CLI e2e verified: `init → import → books → export`. 14 tests green total
  (2 lib + 5 dao + 6 fb2 + 1 import/export).
- **Phase 4** — `cargo test` (gui: navigation by author/genre/series/folder with correct per-node book sets
  and counts, search filtering, soft-delete removal, and a headless egui render smoke across two frames).
  The `gui` feature (eframe/winit) also builds + links on this box; the binary runs and exits cleanly with a
  "no display" error (expected headless). 17 tests green total.
