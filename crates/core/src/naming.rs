//! File naming. Tokens: {yyyy} {MM} {dd} {HH} {mm} {ss} {fff} (millis) {app} (source
//! process name without .exe, sanitized) {title} (window title, sanitized, max 40 chars)
//! {w} {h}. Characters invalid in Windows filenames are replaced with '_'.

use std::path::{Path, PathBuf};

pub struct NameParts<'a> {
    pub now: chrono::DateTime<chrono::Local>,
    pub app: &'a str,
    pub title: &'a str,
    pub width: u32,
    pub height: u32,
}

pub fn expand_pattern(pattern: &str, parts: &NameParts) -> String {
    let _ = (pattern, parts);
    todo!("core::naming::expand_pattern")
}

/// `folder/<expanded>.<ext>`, appending " (2)", " (3)"... if the file exists.
pub fn unique_path(folder: &Path, pattern: &str, parts: &NameParts, ext: &str) -> PathBuf {
    let _ = (folder, pattern, parts, ext);
    todo!("core::naming::unique_path")
}

pub fn sanitize(s: &str) -> String {
    let _ = s;
    todo!("core::naming::sanitize")
}
