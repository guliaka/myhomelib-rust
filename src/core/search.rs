//! Faithful ports of the original's SQLite UDFs that build the `Search*` columns.
//!
//! Source of truth (original FreePascal):
//!   * `unit_FB2Utils.FormatName`            -> [`format_name`]
//!   * `MHL_FULLNAME(L,F,M)`                 -> format_name (preserved case)
//!   * `MHL_FULLNAME(L,F,M,1)`               -> [`upper`](format_name(...))  [the Author.SearchName path]
//!   * `MHL_UPPER(x)` / `MHL_LOWER(x)`       -> [`upper`] / [`lower`] (Unicode)
//!
//! These are also registered as real SQLite UDFs in [`crate::db::connection`] so that existing
//! user `.db` files — whose schema already contains triggers calling these functions — keep
//! working unchanged.

/// Port of `unit_FB2Utils.FormatName` with the UDF's fixed arguments
/// (`nickName = ''`, `onlyInitials = false`).
///
/// Original logic, verbatim semantics:
/// ```pascal
/// Result := LastName;
/// if FirstName <> ''  then Result := Result + ' ' + FirstName;
/// if MiddleName <> '' then Result := Result + ' ' + MiddleName;
/// ```
/// `None` is treated as an empty string (the original's `text16` wrapper yields '' for NULL).
pub fn format_name(
    last_name: &str,
    first_name: Option<&str>,
    middle_name: Option<&str>,
) -> String {
    let mut out = last_name.to_string();
    if let Some(f) = first_name {
        if !f.is_empty() {
            out.push(' ');
            out.push_str(f);
        }
    }
    if let Some(m) = middle_name {
        if !m.is_empty() {
            out.push(' ');
            out.push_str(m);
        }
    }
    out
}

/// Port of `MHL_UPPER` (`SystemUpperString`): Unicode uppercase. Handles Cyrillic, unlike the
/// built-in SQLite `UPPER`/`NOCASE` which are ASCII-only.
///
/// The original upper-cases through the Windows API, which maps one character to one character and
/// differs from Unicode's full mapping in a few places: `ß` stays `ß` (not `SS`), ligatures stay
/// ligatures, dotless `ı`, final sigma `ς` and Georgian letters are left alone. A byte-order mark at the very start of
/// the text is dropped (SQLite consumes it when handing the UDF UTF-16 text). All of this was
/// verified against the `Search*` columns of a 700k-book collection written by the original —
/// matching them keeps searches on such a collection consistent whichever program wrote the row.
pub fn upper(s: &str) -> String {
    map_case(s, |c| {
        // Georgian: Unicode upper-cases Mkhedruli to Mtavruli; Windows has no such mapping.
        if matches!(c, 'ı' | 'ς' | '\u{10d0}'..='\u{10ff}') {
            return c;
        }
        let mut it = c.to_uppercase();
        match (it.next(), it.next()) {
            (Some(u), None) => u,
            _ => c, // multi-character expansion: Windows leaves the character alone
        }
    })
}

/// Port of `MHL_LOWER` (`SystemLowerString`); one-to-one like [`upper`].
pub fn lower(s: &str) -> String {
    map_case(s, |c| {
        let mut it = c.to_lowercase();
        match (it.next(), it.next()) {
            (Some(l), None) => l,
            _ => c,
        }
    })
}

fn map_case(s: &str, f: impl Fn(char) -> char) -> String {
    s.strip_prefix('\u{feff}').unwrap_or(s).chars().map(f).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_name_joins_non_empty_parts() {
        assert_eq!(format_name("Толстой", Some("Лев"), Some("Николаевич")), "Толстой Лев Николаевич");
        assert_eq!(format_name("Doe", None, None), "Doe");
        assert_eq!(format_name("", Some("John"), None), " John"); // leading-space quirk preserved from original
    }

    #[test]
    fn upper_is_unicode_aware() {
        assert_eq!(upper("война и мир"), "ВОЙНА И МИР");
        // One-to-one, like the Windows API the original uses.
        assert_eq!(upper("Die großen Vier"), "DIE GROßEN VIER");
        assert_eq!(upper("\u{feff}The Things"), "THE THINGS");
        assert_eq!(upper("Дэвис \u{feff}Анджела"), "ДЭВИС \u{feff}АНДЖЕЛА");
        assert_eq!(upper("Çalıkuşu της"), "ÇALıKUŞU ΤΗς");
        assert_eq!(upper("ალი და ნინო"), "ალი და ნინო");
        assert_eq!(lower("ВОЙНА"), "война");
    }
}
