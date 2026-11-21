//! User settings, persisted as an INI file next to the collection DB — `<db>.ini` — matching the
//! original's `TMHLSettings`, which writes to `DBFileName + '.ini'`. A minimal ordered INI reader/writer
//! keeps this dependency-free and stays forward-compatible with the original's richer file: unknown
//! sections/keys are preserved on load and re-emitted verbatim on save.

use std::fs;
use std::path::{Path, PathBuf};

/// A minimal ordered INI document: an ordered list of sections, each an ordered list of key/value pairs.
#[derive(Debug, Clone, Default)]
pub struct Ini {
    pub sections: Vec<(String, Vec<(String, String)>)>,
}

impl Ini {
    /// Value for `key` in `section`, if present.
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.sections
            .iter()
            .find(|(s, _)| s == section)
            .and_then(|(_, kv)| kv.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()))
    }

    /// Set (or insert) `key` in `section`, preserving existing order; a new section/key is appended.
    pub fn set(&mut self, section: &str, key: &str, value: &str) {
        match self.sections.iter_mut().find(|(s, _)| s == section) {
            Some((_, kv)) => match kv.iter_mut().find(|(k, _)| k == key) {
                Some((_, v)) => *v = value.to_string(),
                None => kv.push((key.to_string(), value.to_string())),
            },
            None => self
                .sections
                .push((section.to_string(), vec![(key.to_string(), value.to_string())])),
        }
    }

    /// Remove `key` from `section` (no-op if absent).
    pub fn remove(&mut self, section: &str, key: &str) {
        if let Some((_, kv)) = self.sections.iter_mut().find(|(s, _)| s == section) {
            kv.retain(|(k, _)| k != key);
        }
    }

    /// Write to `path` (UTF-8), creating parent directories.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let mut out = String::new();
        for (section, kv) in &self.sections {
            out.push('[');
            out.push_str(section);
            out.push_str("]\n");
            for (k, v) in kv {
                out.push_str(k);
                out.push('=');
                out.push_str(v);
                out.push('\n');
            }
            out.push('\n');
        }
        fs::write(path, out)
    }

    /// Read from `path`; a missing file yields an empty document. Skips blanks and `;`/`#` comments.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let mut ini = Ini::default();
        if !path.exists() {
            return Ok(ini);
        }
        for raw in fs::read_to_string(path)?.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if let Some(inner) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                ini.sections.push((inner.to_string(), Vec::new()));
            } else if let Some(eq) = line.find('=') {
                let key = line[..eq].trim().to_string();
                let val = line[eq + 1..].trim().to_string();
                match ini.sections.last_mut() {
                    Some((_, kv)) => kv.push((key, val)),
                    None => {
                        // A key before any section header — keep it under a synthetic section.
                        ini.sections.push(("_".to_string(), vec![(key, val)]));
                    }
                }
            }
        }
        Ok(ini)
    }
}

/// Typed settings over an [`Ini`], stored at `<db>.ini`.
#[derive(Debug, Clone)]
pub struct Settings {
    pub ini: Ini,
}

impl Default for Settings {
    fn default() -> Self {
        Self { ini: Ini::default() }
    }
}

const GENERAL: &str = "General";
const READERS: &str = "Readers"; // matches the original's READERS_SECTION
const DIRS: &str = "Dirs";

impl Settings {
    /// The INI path for a collection DB — `<db-path>.ini` (the original appends ".ini" to the db name).
    pub fn path_for(db_path: &Path) -> PathBuf {
        let mut p = db_path.as_os_str().to_owned();
        p.push(".ini");
        PathBuf::from(p)
    }

    /// Load settings for `db_path` (defaults if the file is absent).
    pub fn load(db_path: &Path) -> Self {
        Self {
            ini: Ini::load(&Self::path_for(db_path)).unwrap_or_default(),
        }
    }

    /// Persist to `<db>.ini`.
    pub fn save(&self, db_path: &Path) -> Result<(), crate::error::AppError> {
        self.ini
            .save(&Self::path_for(db_path))
            .map_err(|e| crate::error::AppError::Other(format!("save settings: {e}")))
    }

    // -- General -----------------------------------------------------------
    pub fn window_size(&self) -> [f32; 2] {
        let w = self
            .ini
            .get(GENERAL, "window_width")
            .and_then(|s| s.parse::<f32>().ok())
            .unwrap_or(1024.0);
        let h = self
            .ini
            .get(GENERAL, "window_height")
            .and_then(|s| s.parse::<f32>().ok())
            .unwrap_or(700.0);
        [w, h]
    }

    pub fn set_window_size(&mut self, size: [f32; 2]) {
        self.ini.set(GENERAL, "window_width", &format!("{:.0}", size[0]));
        self.ini.set(GENERAL, "window_height", &format!("{:.0}", size[1]));
    }

    pub fn active_tab(&self) -> usize {
        self.ini.get(GENERAL, "active_tab").and_then(|s| s.parse::<usize>().ok()).unwrap_or(0)
    }

    pub fn set_active_tab(&mut self, tab: usize) {
        self.ini.set(GENERAL, "active_tab", &tab.to_string());
    }

    // -- Reader associations (extension → open command) --------------------
    pub fn reader_for(&self, ext: &str) -> Option<String> {
        self.ini.get(READERS, ext).map(|s| s.to_string())
    }

    pub fn set_reader(&mut self, ext: &str, command: &str) {
        if command.trim().is_empty() {
            self.ini.remove(READERS, ext);
        } else {
            self.ini.set(READERS, ext, command);
        }
    }

    pub fn readers(&self) -> Vec<(String, String)> {
        self.ini
            .sections
            .iter()
            .find(|(s, _)| s == READERS)
            .map(|(_, kv)| kv.iter().cloned().collect())
            .unwrap_or_default()
    }

    // -- Last directories --------------------------------------------------
    pub fn last_import_dir(&self) -> Option<String> {
        self.ini.get(DIRS, "import").map(|s| s.to_string())
    }

    pub fn set_last_import_dir(&mut self, dir: &str) {
        self.ini.set(DIRS, "import", dir);
    }
}
