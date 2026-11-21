# MyHomeLib (Rust)

Rust rewrite of [MyHomeLib](https://github.com/guliaka/MyHomeLib.git) — a home e-book library
manager (FreePascal/Lazarus original). Caters to FB2/FBD collections and Librusec-based libraries.

**Status: in progress — roughly a third of the original's functionality.** Working: the data layer
(opens existing collections unchanged), the FB2/FBD engine, folder import, **INPX import** (builds a
1M-book collection from an index in ~3 min), the **system database** (collection registry + groups), the
egui window (six tabs, virtualized grid *and* tree, background imports with a progress page), search,
statistics, and the Librusec script engine. Not yet ported: opening books in a reader, multi-collection UI
and the new-collection wizard, ratings/groups/reviews, editing dialogs, export to device, most view
toggles. See [`PORTING_NOTES.md`](./PORTING_NOTES.md) — "Remaining work" — for the list and estimates.

## Layout
```
src/
  core/     # domain types mirroring the SQLite schema + faithful UDF ports (format_name, upper)
  db/       # rusqlite DAO: schema init, UDF/collation registration, CRUD + search + pagination
  fb2/      # FB2/FBD engine: lossless element-tree model, parser, serializer, ZIP container
  genres.rs # built-in FB2 genre tree + file-format parser (seed / reload / pull-from-FB2)
  import.rs # headless catalog import (scan dir, extract metadata, dedupe by source file)
  inpx.rs   # INPX index -> local collection (port of TImportInpxThread); idempotent, cancellable
  sysdb.rs  # system database `user.dbs2`: collection registry (Bases) + book groups
  jobs.rs   # background jobs: progress, log, cooperative cancel (GUI/CLI/tests)
  export.rs # headless metadata export (rebuild an FB2 from a stored book)
  settings.rs# user settings: minimal ordered INI store at `<db>.ini` (readers, tab, window size, Librusec)
  librusec/ # Librusec online client: connection-script engine, INPX catalog parser, download queue
  gui/      # egui GUI: state (pure Rust) + view (egui) + eframe window runner (`gui` feature)
  error.rs  # AppError
  lib.rs    # library root (core, db, fb2, import, export, librusec, gui, error)
  main.rs   # CLI (init / books / stats / import / genres / export)
  bin/gui.rs# native GUI binary `myhomelib-gui` (behind the `gui` feature)
schema/     # create_collection_db.sql — copied verbatim from the original
assets/     # fb2_genres.txt — embedded initial genre list (standard FB2 codes + Russian aliases)
tests/      # integration tests: data layer, FB2/FBD round-trip, import/export, headless GUI, Librusec engine
```

## Build & test
The default build compiles SQLite from source (`bundled`) → a single self-contained binary (the
Windows target). With a C toolchain on the machine it just works:

```sh
cargo build
cargo test
```

> Linking any Rust binary needs a C compiler (`cc`) + glibc dev libs — e.g. `sudo dnf install -y gcc`
> on Fedora/Rocky. To link the system SQLite instead of bundling it, drop the default feature:
> `cargo test --no-default-features`.

## Performance (1M+ book libraries)
The book list is **virtualized** (`show_rows` + a `LIMIT/OFFSET` fetch of only the visible slice) and the
left-panel trees are **cached per tab**, so cold start is a single `COUNT(*)` — independent of library size.
Measure it at scale:

```sh
cargo run --release --example startup_bench 1000000   # seed 1M books, time open + first frame
```

## CLI
```sh
cargo run -- init    demo.db --demo            # create a collection (+ optional sample data)
cargo run -- books   demo.db --author Толстой   # list / search books
cargo run -- stats   demo.db                    # collection counts
cargo run -- import  demo.db /path/to/fb2-dir [--register-genres]   # scan dir, extract + dedupe
cargo run -- genres  demo.db [--seed | --file list.txt]             # list / re-seed / replace genre catalog
cargo run -- export  demo.db <book-id> out.fb2  # rebuild a book's metadata as FB2
cargo run -- import-inpx lib.hlc2 librusec.inpx [--root /books]     # build a collection from an INPX index
cargo run -- collections user.dbs2 [list]                           # collection registry
cargo run -- collections user.dbs2 create "Name" lib.hlc2 /books [--inpx librusec.inpx]
cargo run -- collections user.dbs2 register "Name" lib.hlc2 /books
cargo run -- collections user.dbs2 delete <id> [--remove-file]
```

## GUI (Phases 4–6)
The windowed app is egui/eframe. The navigation **state** is pure Rust and the **view** is pure egui, so
both are tested headlessly (`cargo test`). Only the native window runner needs winit/GL, behind the
`gui` feature. The window mirrors the original Delphi app's layout (D14): a classic light-gray Win32-style
theme, a menu bar (Книга / Коллекция / Группа / Редактирование / Вид / Инструменты / Помощь), an icon toolbar
of the key actions, and six page-control tabs — **Авторы / Серии / Жанры** (each a left tree + virtualized
multi-column book grid: Название / № / Размер / Рейтинг / Добавлено), **Поиск** (the multi-field search form +
results), **Группы** (folder-based grouping), and **Список загрузок** (the download queue). A bottom info band
shows the selected book's details + genres, with a status bar beneath.

Features: multi-field search (title / author / lang / ext / local-only), a statistics dialog (totals + by-language
/ by-extension breakdowns), settings persisted to an INI at `<db>.ini` (readers, active tab, window size — matching
the original's `TMHLSettings`), and **Import…** which opens a native folder picker (`rfd`) and imports every
`.fb2`/`.fbd` under it with the same headless engine as the CLI. The **Online** panel (Librusec) adds a connection
editor, a background catalog search over an INPX feed, and a download queue with per-item progress + cancel; its
engine is pure Rust and unit-tested against a mock transport, only the live HTTP edge uses `reqwest`
(`rustls-tls`, no OpenSSL DLLs on Windows). egui approximates — but can't pixel-replicate — the original's beveled
native controls.

```sh
cargo run --features gui --bin myhomelib-gui [db-path]   # open the GUI (needs a display)
```

On a headless box the `gui` feature still builds and links; it just can't open a window without
`$DISPLAY`/Wayland. The Windows target is where it ships as part of the single `.exe`.
