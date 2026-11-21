//! Built-in FB2 genre tree + parsing of the MyHomeLib genre-file format.
//!
//! The original loads genres from a UTF-8 text file (`TBookCollection.LoadGenres`,
//! `unit_Database_Abstract.pas`): each line is `<dotted.code> [fb2code;]alias`; blank lines and
//! `#` comments are skipped, and a code with no `.` is ignored. The parent is the code minus its
//! last `.segment`. We embed a standard initial tree (`assets/fb2_genres.txt`) and reuse this same
//! parser for custom files loaded via `myhomelib genres <db> --file <path>`.

use crate::core::Genre;

/// Parse genre-file text into rows, using the exact algorithm of the original `LoadGenres`.
pub fn parse_genre_lines(text: &str) -> Vec<Genre> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end();
        // Skip blanks and comments.
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Code = up to the first space.
        let Some(sp) = line.find(' ') else {
            continue;
        };
        let code = line[..sp].trim();
        if code.is_empty() {
            continue;
        }
        // Parent = code minus its last ".segment" (a code with no dot is ignored, as in the original).
        let Some(dp) = code.rfind('.') else {
            continue;
        };
        let parent = &code[..dp];

        // The remainder: optional "fb2code;" prefix, then the alias.
        let rest = line[sp + 1..].trim_start();
        let (fb2_code, alias) = match rest.find(';') {
            Some(p) => (Some(rest[..p].trim().to_string()), rest[p + 1..].trim().to_string()),
            None => (None, rest.trim().to_string()),
        };

        out.push(Genre {
            genre_code: code.to_string(),
            parent_code: Some(parent.to_string()),
            fb2_code,
            genre_alias: alias,
        });
    }
    out
}

/// The embedded initial FB2 genre tree (standard codes + Russian aliases), in the file format.
pub const BUILTIN_GENRES: &str = include_str!("../assets/fb2_genres.txt");
