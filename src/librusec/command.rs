//! Connection-script engine — exact port of the original `TDownloader.ParseCommands`.
//!
//! A connection script is one command per line:
//! ```text
//! CHECK            validate the last response (error-page detection) and save it to disk
//! REDIR            assert a redirect target was captured (else: bad credentials)
//! PAUSE <ms>       sleep for <ms> milliseconds
//! GET  <url>       HTTP GET (with progress); a redirect to a .zip target is captured as %RESURL%
//! POST <url>       HTTP POST of the accumulated form fields (added via ADD)
//! ADD  <name> <v>  append a form field for the next POST
//! ```
//!
//! Macro substitution happens at parse time for `GET`/`POST`/`ADD`: every `%NAME%` in the argument
//! is replaced. The special macro `%RESURL%` is substituted at *execution* time (after a `GET` that
//! redirects), so it survives parsing untouched.

/// One parsed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    Check,
    Redir,
    Pause(u32),
    /// URL template with `%USER%`/`%PASS%`/`%URL%`/book-field macros already substituted;
    /// `%RESURL%` is left for the executor to fill in after a redirect.
    Get(String),
    Post(String),
    Add(String, String),
    /// Unknown or empty line (the original's `Code = -1`): a no-op at execution time.
    Skip,
}

/// Substitute every `%NAME%` macro in `s` using the given `(name, value)` pairs, applied in order
/// (matching the original's iteration over the macro list).
pub fn substitute(s: &str, macros: &[(String, String)]) -> String {
    let mut out = s.to_string();
    for (name, value) in macros {
        if !name.is_empty() {
            out = out.replace(name.as_str(), value.as_str());
        }
    }
    out
}

/// Parse a connection script into commands, substituting the given macros.
pub fn parse_commands(script: &str, macros: &[(String, String)]) -> Vec<Cmd> {
    let mut out = Vec::new();
    for line in script.lines() {
        // Command word = text before the first space; the rest is the argument.
        let (command, rest) = match line.find(' ') {
            Some(i) => (&line[..i], &line[i + 1..]),
            None => (line, ""),
        };
        let cmd = match command {
            "CHECK" => Cmd::Check,
            "REDIR" => Cmd::Redir,
            "PAUSE" => Cmd::Pause(rest.trim().parse::<u32>().unwrap_or(0)),
            "GET" => Cmd::Get(substitute(rest, macros)),
            "POST" => Cmd::Post(substitute(rest, macros)),
            // ADD uses the first two space-delimited tokens (Params[0] = name, Params[1] = value).
            "ADD" => {
                let mut parts = rest.split_whitespace();
                let name = parts.next().unwrap_or("").to_string();
                let value = parts.next().unwrap_or("").to_string();
                Cmd::Add(substitute(&name, macros), substitute(&value, macros))
            }
            _ => Cmd::Skip,
        };
        out.push(cmd);
    }
    out
}
