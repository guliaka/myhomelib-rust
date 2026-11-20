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
pub fn upper(s: &str) -> String {
    s.to_uppercase()
}

/// Port of `MHL_LOWER` (`SystemLowerString`).
pub fn lower(s: &str) -> String {
    s.to_lowercase()
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
    }
}
