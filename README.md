# MyHomeLib (Rust)

Rust rewrite of [MyHomeLib](https://github.com/guliaka/MyHomeLib.git) — a home e-book library
manager (FreePascal/Lazarus original). Caters to FB2/FBD collections and Librusec-based libraries.

**Status: Phase 1 of 7** — data layer only. See [`PORTING_NOTES.md`](./PORTING_NOTES.md) for the
phase plan, decisions, and deferred items.

## Layout
```
src/
  core/     # domain types mirroring the SQLite schema + faithful UDF ports (format_name, upper)
  db/       # rusqlite DAO: schema init, UDF/collation registration, CRUD + search + pagination
  error.rs  # AppError
  lib.rs    # library root (core, db, error)
  main.rs   # Phase 1 smoke CLI (init / books / stats)
schema/     # create_collection_db.sql — copied verbatim from the original
tests/      # integration tests for the data layer
```

## Build & test
Default build compiles SQLite from source (`bundled`) → single self-contained binary (the Windows
target). On a dev machine **without a C compiler**, link against the OS SQLite instead:

```sh
# needs libsqlite3 present; create a dev symlink if only libsqlite3.so.0 exists:
mkdir -p .libs && ln -sf /usr/lib64/libsqlite3.so.0 .libs/libsqlite3.so
RUSTFLAGS="-L native=$PWD/.libs" cargo test --no-default-features
```

> Note: linking any Rust binary requires a C toolchain (`cc`) + glibc dev libs. If the host lacks
> them, `sudo dnf install -y gcc` (or your distro equivalent) first.

## Smoke CLI
```sh
cargo run -- init  demo.db --demo   # create a collection + seed sample data
cargo run -- books demo.db --author Толстой --limit 5
cargo run -- stats demo.db
```
