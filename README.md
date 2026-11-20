# MyHomeLib (Rust)

Rust rewrite of [MyHomeLib](https://github.com/guliaka/MyHomeLib.git) — a home e-book library
manager (FreePascal/Lazarus original). Caters to FB2/FBD collections and Librusec-based libraries.

**Status: Phases 1–2 of 7** — data layer + FB2/FBD engine. See [`PORTING_NOTES.md`](./PORTING_NOTES.md)
for the phase plan, decisions, and deferred items.

## Layout
```
src/
  core/     # domain types mirroring the SQLite schema + faithful UDF ports (format_name, upper)
  db/       # rusqlite DAO: schema init, UDF/collation registration, CRUD + search + pagination
  fb2/      # FB2/FBD engine: lossless element-tree model, parser, serializer, ZIP container
  error.rs  # AppError
  lib.rs    # library root (core, db, fb2, error)
  main.rs   # smoke CLI (init / books / stats)
schema/     # create_collection_db.sql — copied verbatim from the original
tests/      # integration tests for the data layer
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

## Smoke CLI
```sh
cargo run -- init  demo.db --demo   # create a collection + seed sample data
cargo run -- books demo.db --author Толстой --limit 5
cargo run -- stats demo.db
```
