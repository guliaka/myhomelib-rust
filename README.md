# MyHomeLib (Rust)

Rust rewrite of [MyHomeLib](https://github.com/guliaka/MyHomeLib.git) — a home e-book library
manager (FreePascal/Lazarus original). Caters to FB2/FBD collections and Librusec-based libraries.

**Status: Phases 1–3 of 7** — data layer, FB2/FBD engine, and headless import/export. See
[`PORTING_NOTES.md`](./PORTING_NOTES.md) for the phase plan, decisions, and deferred items.

## Layout
```
src/
  core/     # domain types mirroring the SQLite schema + faithful UDF ports (format_name, upper)
  db/       # rusqlite DAO: schema init, UDF/collation registration, CRUD + search + pagination
  fb2/      # FB2/FBD engine: lossless element-tree model, parser, serializer, ZIP container
  import.rs # headless catalog import (scan dir, extract metadata, dedupe by source file)
  export.rs # headless metadata export (rebuild an FB2 from a stored book)
  error.rs  # AppError
  lib.rs    # library root (core, db, fb2, import, export, error)
  main.rs   # CLI (init / books / stats / import / export)
schema/     # create_collection_db.sql — copied verbatim from the original
tests/      # integration tests: data layer, FB2/FBD round-trip, import/export
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

## CLI
```sh
cargo run -- init    demo.db --demo            # create a collection (+ optional sample data)
cargo run -- books   demo.db --author Толстой   # list / search books
cargo run -- stats   demo.db                    # collection counts
cargo run -- import  demo.db /path/to/fb2-dir   # scan dir for .fb2/.fbd, extract + dedupe
cargo run -- export  demo.db <book-id> out.fb2  # rebuild a book's metadata as FB2
```
