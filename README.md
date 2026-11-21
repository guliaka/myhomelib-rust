# MyHomeLib (Rust)

Rust rewrite of [MyHomeLib](https://github.com/guliaka/MyHomeLib.git) — a home e-book library
manager (FreePascal/Lazarus original). Caters to FB2/FBD collections and Librusec-based libraries.

**Status: complete — all 7 phases.** Data layer, FB2/FBD engine, headless import/export, the egui GUI,
search/settings/statistics, the Librusec online client (catalog search + download queue), and performance
work for 1M+ book libraries (virtualized book list + cached trees). See [`PORTING_NOTES.md`](./PORTING_NOTES.md)
for the phase plan, decisions, and deferred items.

## Layout
```
src/
  core/     # domain types mirroring the SQLite schema + faithful UDF ports (format_name, upper)
  db/       # rusqlite DAO: schema init, UDF/collation registration, CRUD + search + pagination
  fb2/      # FB2/FBD engine: lossless element-tree model, parser, serializer, ZIP container
  genres.rs # built-in FB2 genre tree + file-format parser (seed / reload / pull-from-FB2)
  import.rs # headless catalog import (scan dir, extract metadata, dedupe by source file)
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
```

## GUI (Phases 4–6)
The windowed app is egui/eframe. The navigation **state** is pure Rust and the **view** is pure egui, so
both are tested headlessly (`cargo test`). Only the native window runner needs winit/GL, behind the
`gui` feature. Phase 5 adds a multi-field search (title / author / lang / ext / local-only), a
statistics dialog (totals + by-language / by-extension breakdowns), and settings persisted to an INI at
`<db>.ini` (reader associations, active tab, window size — matching the original's `TMHLSettings`).

Phase 6 adds the **Online** tab: a Librusec connection editor (user / password / URL / connection
script, persisted under `[Librusec]` in `<db>.ini`), a background catalog search over an INPX feed, and
a download queue with per-item progress + cancel. The engine (connection-script interpreter, INPX
parser, queue) is pure Rust and unit-tested against a mock transport; only the live HTTP edge uses
`reqwest` (`rustls-tls`, no OpenSSL DLLs on Windows).

```sh
cargo run --features gui --bin myhomelib-gui [db-path]   # open the GUI (needs a display)
```

On a headless box the `gui` feature still builds and links; it just can't open a window without
`$DISPLAY`/Wayland. The Windows target is where it ships as part of the single `.exe`.
