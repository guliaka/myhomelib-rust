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
| 4 | GUI shell + tree navigation (egui) | ✅ done | egui main window: category tabs (Authors/Genres/Series/Folders) + node tree with counts, book list, detail panel, search box, soft-delete. State is pure Rust (headless-tested); view is pure egui; eframe runner behind the `gui` feature. Import wired to a native folder picker (D13); add/edit book dialogs still deferred. |
| 5 | Search UI, settings, stats | ✅ done | Multi-field search (title/author/lang/ext/local-only), statistics dialog (totals + by-lang/by-ext breakdowns via in-use counts), settings persisted to `<db>.ini` (readers, active tab, window size) matching the original's `TMHLSettings`. Named search presets deferred. `cargo test` green incl. search/stats/settings round-trip. |
| 6 | Librusec client + download queue (`librusec/`) | ✅ done | Connection-script engine (GET/POST/ADD/CHECK/REDIR/PAUSE) + macro substitution, INPX catalog parser, background search + download queue with progress/cancel. GUI Online tab wired. Engine is pure Rust, unit-tested vs a mock transport; live HTTP via reqwest (rustls). `cargo test` green (7 librusec + online GUI). |
| 7 | Polish + performance | ✅ done | Book list **virtualized** (`show_rows` + `LIMIT/OFFSET` window fetch, selection by stable id) and left-panel trees **cached per tab**, so cold start is a single `COUNT(*)` — flat at any library size. Benchmark at 1M books: open+state 27 ms, first frame ~230 ms (egui init). Windows build via CI (`rustls`, no OpenSSL). |


> **Scope correction.** "Done" above means the phase's *planned slice* shipped and is tested — not
> that the original is fully ported. Measured against the original's 76 main-form actions the port
> covers roughly a third. The gap list and estimates are in **Remaining work** below.

## Remaining work (dev-days, one experienced Rust developer)

| # | Area | State | Left |
|---|------|-------|------|
| 1 | INPX + archive-backed collections, background import | **engine, CLI, GUI job done** (D16, D18). fb2.zip archive *scan* import (no INPX) not ported | 2 |
| 2 | Multiple collections | **system DB + CLI done** (D17). GUI: collection switcher, new-collection wizard, properties, delete | 4–5 |
| 3 | Open a book in a reader (extract from archive, launch), book-info dialog with cover | not started | 3–4 |
| 4 | User data: groups UI + membership, favourites, rating, read flag, reviews, import/export | group registry only (D17) | 4–5 |
| 5 | Editing: book / author / series / genre dialogs, add non-FB2, convert to FBD | not started | 6–8 |
| 6 | Export to device (filename templater, zip), HTML / TXT / RTF / INPX export | metadata-only FB2 export | 5–7 |
| 7 | View: alphabet bars, tree/table view, column picker, hide-deleted / local-only, presets, full settings | not started | 6–8 |
| 8 | Downloads into the collection, online collection updates, sync / repair / compact | script engine + queue only | 4–5 |
| 9 | Windows verification, test against a real existing library, bug fixing | never run on Windows | 6–10 |

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

### D8 — Settings persist to an INI at `<db>.ini` (matching the original's `TMHLSettings`)
The original (`unit_Settings.pas`) writes user settings to `DBFileName + '.ini'` with sections like
`[Readers]` (extension → open command). We match that exactly: a minimal ordered INI reader/writer in
`settings.rs` (no extra dependency) stores the same file, so an existing collection keeps its settings.
Unknown sections/keys are preserved on load and re-emitted verbatim on save (forward-compatible with the
original's richer file). Window size is read from the INI on launch; auto-save of a resized window is
deferred — egui 0.31's viewport size isn't a stable proxy for the outer window, so persisting it per-frame
would be wrong.

### D9 — Statistics use "in-use" counts, not raw catalog row counts
`State::stats_data()` reports distinct authors/series/genres **linked to non-deleted books** (via the
`Author_List`, `Books.SeriesID`, and `Genre_List` link tables) rather than `COUNT(*)` on the catalog tables.
This is more meaningful than raw catalog counts, which can include orphan rows or miss codes that exist only
as links. The `Genres` catalog itself is populated by the embedded built-in FB2 tree on new collections (D10).

### D10 — Genre catalog: embedded seed + reload/update + pull-from-FB2
The original loads its genre list from a UTF-8 file (`TBookCollection.LoadGenres`, `ReloadGenres`); that data
file is not committed to the repo, so we embed a standard FB2 tree (`assets/fb2_genres.txt`) in the same
format and parse it with an exact port of the original's algorithm (`genres::parse_genre_lines`). Three paths:
- **Seed on new collection** — `Db::open` seeds the built-in tree when it initializes a fresh schema (the
  original's `CreateCollection` does the same); existing collections are never touched.
- **Load/update** — `myhomelib genres <db> [--seed | --file <path>]`: `--seed` re-applies the built-in list
  (idempotent upsert), `--file` replaces the whole catalog from a custom file and drops dangling links (the
  original's `ReloadGenres`).
- **Pull from FB2 on import** — `myhomelib import <db> <dir> --register-genres` also registers each book's
  `<genre>` code in the catalog if missing, so the list grows to reflect imported books.

### D11 — Librusec = faithful connection-script engine + INPX catalog; pure-Rust core, thin live-HTTP edge
The original's downloader (`Program/DwnldImpl/unit_Downloader.pas`) is not a fixed API client but a small
**connection-script interpreter**: the user supplies a script of `GET url` / `POST url` / `ADD name value` /
`CHECK` / `REDIR` / `PAUSE ms` commands, and macros (`%USER%`, `%PASS%`, `%URL%`, plus book fields) are
substituted in. Two semantics carried over exactly:
- **`%RESURL%` is substituted at execution time** (after a `GET` that redirects to the real download target),
  not at parse time — so it must survive macro substitution untouched until the redirect is captured. A
  `REDIR` command asserts such a redirect was seen (else `WrongCredentials`).
- **`CHECK` validates the last response** (error-page detection) and saves it as the result body.

The catalog feed is an **INPX** delimited text file: fields separated by `Chr(4)` (`\u{04}`), author parts by
`:` then `,`, genre codes by `:`; a leading `Structure:` header declares field order + base URL, and each later
line containing the field separator is one book record (see `librusec::catalog`).

We keep the engine **network-free** (`command` / `catalog` / `queue` / `client`) and unit-test it against a mock
`Transport`; only `librusec::service.rs` touches real HTTP, via `reqwest` pinned to `rustls-tls` + `blocking`
(no OpenSSL DLLs on the Windows `.exe`, and the blocking API lets the GUI drive downloads from a worker thread).
Resume / partial-download is deferred to Phase 7 polish.

### D12 — Virtualized book list + per-tab tree cache (targeting 1M+ book libraries)
The GUI must stay responsive on collections of a million+ books. Two changes make startup and interaction
cost independent of library size:
- **The book list is virtualized.** The view uses egui's `ScrollArea::show_rows`, which hands back only the
  *visible* row range; `State::books_range(start, count)` fetches exactly that slice with `LIMIT/OFFSET`
  (cached while unchanged). Startup therefore does one `COUNT(*)` (`list_total`) + a ~30-row window fetch —
  never a full load. Selection is by stable `BookID` (not a row index), so it survives scrolling/pagination,
  and the detail panel fetches its book by id. A unified `Db::list_scope_books`/`count_scope_books` pages
  every tree node (author/genre/series/folder) with the original's per-node ordering (series by sequence).
- **The left-panel trees are cached per tab.** Each tab is a full-scan `GROUP BY` over the whole collection,
  so it runs once per tab visit (and after any data mutation), not on every render frame — which would have
  frozen the UI at scale. Re-visiting a tab returns its cache instantly.

Measured (`examples/startup_bench`, 1M books): open+`State::new` **27 ms**, first render frame ~230 ms
(one-time egui context/font init), Folders tree first build ~2 s (unindexed `GROUP BY`) but re-visit **5 µs**
from cache. The eager full load we no longer do would take ~2.6 s and hold all 1M rows in memory.

*(Known: the per-node tree first-build is a full scan — the original schema has no index on `Folder`/`SeriesID`.
Adding covering indexes would speed it but diverges from the shipped schema; deferred.)*

### D13 — GUI Import = native folder picker (`rfd`) + synchronous import through `State`
The top-bar **Import…** button opens a native folder dialog (`rfd` — the Windows common dialog on the target,
no extra DLLs to bundle) and imports every `.fb2`/`.fbd` under it with the *same* headless `Importer` the CLI
uses. The picker is gated behind the `gui` feature (`#[cfg(feature = "gui")]`) so headless builds/tests never
open a real dialog; the actual work lives in `State::import_dir`, which runs **synchronously** (the rusqlite
connection isn't `Send`) and then rebuilds the per-tab trees + reloads the list total, since new books may add
authors/genres/series/folders. Large folders briefly block the UI while they import; a background/progress path
would need either a second connection or chunked frames (the shared connection can't move to a worker thread) —
deferred as a follow-up.

### D14 — GUI = full structural match of the original (6-tab shell, menu bar, toolbar), on `devel`
The user wanted the Rust GUI to look like the original Delphi app, so on the `devel` branch the window was
restructured to mirror `Program/Forms/frm_main.dfm`: a classic light-gray Win32-style theme (egui draws its own
flat widgets and can't render beveled native controls, so this is the closest achievable approximation), a
TMainMenu-style menu bar (Книга / Коллекция / Группа / Редактирование / Вид / Инструменты / Помощь), an icon
toolbar of the key actions, and the original's six page-control tabs (Авторы / Серии / Жанры / Поиск / Группы /
Список загрузок). `state.tab` is now the main page-control tab (0-5): each category tab shows its own left tree +
a virtualized multi-column book grid (Название / № / Размер / Рейтинг / Добавлено, added-date from
`Books.update_date`); Поиск hosts the multi-field search form + results; Список загрузок shows the download queue.
A bottom info band (the original's TInfoPanel) shows the selected book's details + genres; a TStatusBar sits at the
very bottom. Virtualization is unchanged — only the visible slice is ever materialized, so 1M+ libraries stay fast.
*Known delta:* «Группы» currently shows the folder tree (the original's user-defined groups aren't ported; folders
are the closest existing organization).

### D15 — Grid = original's exact columns (incl. star rating + genre) + real icon toolbar, on `devel`
The book grid now matches the original's column set exactly: **Title** (no header) / **№** / **Размер** /
**Моя оценка** (five vector stars — filled for rated, outline for unrated; orange, white-on-selection) /
**Добавлено** / **Жанр**. Genres cost one batched `GROUP_CONCAT … IN (visible ids)` query per visible window
(cached in `State` keyed by window identity), so the grid stays O(visible rows) at 1M+ scale — never one query
per row. The toolbar now uses the original's real icon art: its bundled 32×32 PNGs (`assets/icons-32/`) decoded
with the pure-Rust `image` crate (png-only, gui-only dep) and cached as `TextureHandle`s per context in
`MyHomeLibApp` — egui's texture manager allocates a new id on every `load_texture`, so re-loading each frame would
leak. The 16 buttons are in the original's order with separators + their original tooltips; the three
`tbsDropDown` buttons draw a small caret. Only two actions are wired (SelectAll, Settings) — the rest mirror the
original's view/collection toggles and show their tooltip until ported. Headless (non-gui) builds render a neutral
placeholder per icon since `image` is gui-only. *Known delta:* egui draws flat widgets, so the toolbar/menu can't
be pixel-identical to beveled native Win32 controls — this is the closest achievable in pure egui.

### D16 — INPX import = faithful port of `TImportInpxThread`, checked row-for-row against the original
`inpx.rs` follows the original record for record: `structure.info` field order (default structure when
absent), `Chr(4)` / `:` / `,` delimiters, `Normalize` (missing title → «Без названия», missing author →
«Неизвестный», missing genre → `0.0`), `DEL >= "1"`, `SeqNumber > 5000 → 0`, extension stored with its dot;
local collections get `Folder = <inp name>.zip` + `InsideNo = line index` unless the structure has
`FOLDER`; online collections get the generated `Letter\Author\LibID Title.fb2.zip` location;
`collection.info` / `version.info` set the notes / URL / script / data-version properties.

**Verified against real data (D20):** the same Flibusta INPX (699,510 records, 217 `.inp`) was imported
here and compared with the collection the original built from it. Details the comparison forced:
dates are stored as plain `yyyy-mm-dd`; empty `Lang` is `''`, not NULL; author name parts are trimmed,
and a name with a missing comma shifts into the *middle* name exactly as the original's cut-twice
parser does; an FB2 genre code the catalog doesn't know is filed under `0.0`.

Deliberate deltas (each one visible in the comparison):
- **Idempotent.** The original inserts every record on every run, so importing an INPX twice doubles
  the collection. Here a record whose (folder, file, position) is already present is skipped.
- **Keywords are imported.** The original's field-list parser only takes names *followed by* `;`, so
  with the default structure (which doesn't end in `;`) the last field, `KEYWORDS`, is silently
  dropped — 188,924 books in the test library have keywords the original never stored.
- **`SERNO` with stray spaces is still a number** (`" 03 "` → 3; the original stores 0). 2 records.
- **A list item without its terminating `:` is still read** (the original drops it).
Known non-match: the original compares names through the Windows collation, which treats soft hyphens
and composed/decomposed letters (`ё`, `й`) as equal, so it reuses one row where this port creates a
second — 3 extra authors and 3 extra series out of 170k / 74k in the test library.

Speed: ids for authors / series / genres come from in-memory maps, one transaction per `.inp`, schema
triggers off (the DAO writes the `Search*` columns itself), 256 MB page cache on the import connection.
Measured on this box: real Flibusta INPX, 699,510 books — **200 s**; synthetic 1M records — 167 s.

### D17 — System database = the original's `user.dbs2`, schema verbatim
`sysdb.rs` ports the registry half of `TSystemData_SQLite`: `Bases` create / register / update / delete
(the delete trigger drops the collection's grouped-book copies) and group add / rename / clear / delete
with the two undeletable built-ins («Избранное», id 1, and «К прочтению»). New collections are stamped
with the original's schema-version GUID and type code so the original accepts them, and
`Db::probe_collection` is the port of `IsValidCollection`. Paths are stored relative to the system
database's folder when under it (the original uses its data folder), absolute otherwise; Windows-style
stored paths are understood on any platform. Not yet ported: grouped-book copies (`Books` /
`BookGroups` writes), per-book user data, user-data import/export.

### D18 — Background jobs: own connection per job, progress page instead of the main window
`jobs.rs` is the `TWorker` + progress-form pair: a thread, a cloneable progress handle (percent, comment,
capped log, error count, cooperative cancel), and a result; a panic in the job becomes an error result.
A job opens its **own** SQLite connection (rusqlite connections aren't `Send`, and the original does the
same), with a 10 s busy timeout on every connection. While a job is active the window shows only the
progress page (Прервать / Закрыть + log tail), so the UI connection issues no queries that could block
on the writer; acknowledging the job rebuilds the trees and the list. Both INPX and folder import run
this way — the folder import no longer freezes the window (supersedes the limitation noted in D13).

### D19 — Left tree virtualized; author tree built in memory
The tree cloned and laid out *every* node on *every* frame — fine for the 3-author fixtures, unusable at
library scale (first paint took ~15 s with 425k authors). It is now rendered with `show_rows` straight
from the cached slice. The author-tree query was also rewritten: counts from an `Author_List` index scan,
names from `Authors`, joined and sorted in Rust (6.5 s → 0.5 s at 425k authors, release build). Author
names are shown in display case (the 4-argument `MHL_FULLNAME` always upper-cases — it is the
`SearchName` form), and the grid's Жанр column shows genre names instead of codes.

### D20 — Fidelity is measured against a collection written by the original
`/home/hermes/inpx` holds a real INPX, collections the original built from it, and book archives. The
original's `.hlc2` opens unchanged (CLI stats and the windowed app: 586,429 visible books, trees, grid).
Importing the same INPX and diffing all 699,510 books field by field — every `Books` column including
the seven `Search*` ones, plus each book's authors (with `SearchName`), series and genre links — leaves
only the deltas listed in D16 and 27 rows of the user's own reading progress. What the diff changed:
- **Genre catalog** is now the original's `genres_fb2` list (317 rows, recovered from that collection's
  `Genres` table), replacing the hand-made 79-row list. Genre links match exactly (1,111,334).
- **`MHL_UPPER` / `Search*`**: NULL gives `''`, never NULL; upper-casing is one character to one
  character like the Windows API (`ß`, `ı`, `ς`, Georgian stay as they are; a leading BOM is dropped).
Not replicated: Windows collation equivalences (see D16).

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

- **Built-in FB2 genre tree** — **resolved in Phase 5.** A standard FB2 tree is embedded
  (`assets/fb2_genres.txt`) and seeded on every new collection (D10); `myhomelib genres <db> --seed` re-applies
  it and `--file <path>` replaces it with a custom list. *(Since D20 the embedded list is the original's.)* Import links each book's raw `<genre>` code into
  `Genre_List`; the catalog is left-joined for aliases by either `GenreCode` or `FB2Code`. *Open: the embedded
  list is a standard FB2 set — swap in the original's exact shipped file via `--file` if byte-parity with an
  existing install matters.*
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
- **Phase 5** — `cargo test` (gui: multi-field search by author/title/lang/ext/local-only, statistics
  breakdowns by language/extension + in-use totals, settings round-trip through `<db>.ini`, render smoke
  opening the stats dialog; genres: file-format parsing, built-in seed on new collections, alias resolution,
  replace-from-file with dangling-link cleanup, idempotent `register_genre_code`). CLI e2e verified:
  `init` (seeds 79 genres) → `import --register-genres` (pulls a new genre into the catalog) → `genres`.
  `cargo build --features gui` compiles clean. 24 tests green total (2 lib + 5 dao + 6 fb2 + 5 gui + 5 genres
  + 1 import/export), no warnings.
- **Phase 6** — `cargo test` (librusec: connection-script parse + macro substitution with `%RESURL%` left
  intact, INPX catalog parse/search over the delimited format, download-queue state machine incl. progress/
  cancel/cancel-all, and the client executing a script against a mock `Transport`; gui: background search poll
  → results, queueing a book resolving its URL onto the catalog base, `[Librusec]` settings round-trip through
  `<db>.ini` with a multi-line script, and a headless render of the Online panel with an in-flight download).
  `cargo build --features gui` compiles clean. 32 tests green total (2 lib + 5 dao + 6 fb2 + 6 gui + 5 genres
  + 1 import/export + 7 librusec), no warnings.
- **Phase 7** — Virtualized the book list (`show_rows` + `LIMIT/OFFSET` window fetch, by-id selection) and
  cached the left-panel trees per tab so startup/interaction cost is independent of library size. New DAO:
  unified paginated `list_scope_books`/`count_scope_books` (all tree nodes, series ordered by sequence) + a
  bulk-insert fast path. `cargo test` green (added a scope-pagination/ordering dao test; GUI tests updated to
  the lazy accessor). **Startup benchmark** (`examples/startup_bench`, 1M books): open+`State::new` 27 ms,
  first render frame ~230 ms (egui init), Folders tree re-visit 5 µs from cache vs ~2 s first build; the eager
  full load we no longer do would take ~2.6 s + hold all rows in memory. `cargo build --features gui` clean,
  no warnings. 33 tests green total.
- **GUI Import (post-Phase 7)** — Wired the top-bar **Import…** button to a native folder picker (`rfd`,
  gated behind the `gui` feature) that imports every `.fb2`/`.fbd` under the chosen folder via the shared
  headless `Importer`; new `State::import_dir` runs it synchronously then rebuilds the per-tab trees + reloads
  the list total (D13). New headless test `import_dir_refreshes_state` covers import → view refresh → dedupe on
  re-import → rebuilt author tree. `cargo test` green (34 total: gui suite now 7), `cargo build --features gui`
  clean, no warnings on either feature set.
- **GUI structural rework (D14, on `devel`)** — Rebuilt the window to match the original's layout: classic
  light-gray theme (G1), multi-column virtualized book grid + bottom info band (G2), menu bar + icon toolbar +
  six-tab page-control shell with per-tab content and a dedicated download-queue tab (G3), and the original's
  book-grid columns with genres in the info band (G4). `state.tab` is now the main tab (0-5) with per-tab helpers
  + a clean-slate reset on switch. The navigation test was updated to the new tab order, and the headless render
  smoke test now walks all six tabs (menu/toolbar/tab-bar/nav/search/downloads paths) without panic. `cargo test`
  green (34 total), `cargo build` and `cargo build --features gui` clean with no warnings on either feature set.
  Visual verification on a real Windows box is still pending (headless here).
- **GUI G5 — original grid columns + real icon toolbar (D15, on `devel`)** — Re-added the original's exact
  book-grid columns: Title / № / Размер / Моя оценка (five vector stars) / Добавлено / Жанр, with genres from a
  single batched query per visible window (`book_genres_batch`, cached in `State`). The toolbar now renders the
  original's real bundled 32×32 PNG icons (decoded via the pure-Rust `image` crate, cached as `TextureHandle`s per
  context; decode is cfg-gated so headless builds use a placeholder). All six tabs + the icon toolbar render
  headlessly without panic. `cargo test` green (34 total) and `cargo build` / `cargo build --features gui` clean
  with no warnings on either feature set; all 16 bundled icons verified valid PNGs. Visual verification on a real
  Windows box is still pending (headless here).
- **INPX import, system DB, background jobs, tree virtualization (D16–D19, on `fable-t1`)** — `cargo test`
  green: 46 tests (8 lib unit, 5 new INPX/system-DB integration, GUI suite now 8 incl. a background import
  driven through the progress page), `cargo build` and `cargo build --features gui` clean, no warnings.
  Real Flibusta INPX (699,510 records) imports in 200 s with 0 errors and matches the original's own
  collection field by field apart from the documented deltas (D20). CLI e2e on a synthetic 1M-record INPX: `collections create … --inpx` → 1,000,000 books in 167 s, re-import
  adds 0. The windowed binary was run against that collection under Xvfb (software GL) and screenshotted:
  tree and grid populated, genre names resolved. Still not run on Windows.
