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
| 2 | FB2/FBD engine (`fb2/`) | ⬜ todo | Streaming parser (quick-xml) → typed model → serializer; FBD container support. |
| 3 | Headless import/export (`import/`, `export/`) | ⬜ todo | FB2/FBD/INPX/folder import; export to FB2/FBD/device. Background threads + progress. |
| 4 | GUI shell + tree navigation (egui) | ⬜ todo | Main window, Author/Genre/Series/Folder trees, detail panel, add/edit/delete dialogs. |
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

- **Built-in FB2 genre tree** (`InternalLoadGenres` in the original) is **not yet ported** — a fresh
  collection starts with an empty `Genres` table. Populate via import or add the seed data in Phase 3.
  *Action: locate the original's default-genre source file and embed it.*
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
