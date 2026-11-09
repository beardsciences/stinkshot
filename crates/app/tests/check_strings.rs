//! Hard-coded UI string checker (docs/notes/l10n.md). Flags string literals passed directly
//! to egui label/button/heading/hover-text style calls in crates/app/src instead of
//! `tr!("group.key")`, and reports counts per group and file.
//!
//! The full scan is `#[ignore]`d while files are being converted; run it with
//! `scripts/check-strings.ps1` or
//! `cargo test -p stinkshot --test check_strings -- --ignored --nocapture`.
//! Allow a literal with `// l10n-ok` on its line, or globally in scripts/strings-allow.txt.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Calls whose direct string-literal arguments are user-visible text.
const CALLS: &[&str] = &[
    "label",
    "button",
    "heading",
    "small",
    "strong",
    "weak",
    "hyperlink_to",
    "on_hover_text",
    "on_hover_text_at_pointer",
    "on_disabled_hover_text",
    "hint_text",
    "selectable_value",
    "selectable_label",
    "checkbox",
    "radio",
    "radio_value",
    "menu_button",
    "collapsing",
    "selected_text",
    "Button::new",
    "Label::new",
    "RichText::new",
    "MenuItem::new",
    "MenuItem::with_id",
    "with_tooltip",
    "text",
    "layout",
    "layout_no_wrap",
    "layout_job",
    "accent_button",
    "icon_button",
    "setting_row",
    "toggle_row",
    "group_title",
];

#[derive(Debug, PartialEq)]
struct Hit {
    line: usize,
    text: String,
}

/// Scans Rust source and returns flagged literals. Stops at the first `#[cfg(test)]`.
fn scan(src: &str, allow: &[String]) -> Vec<Hit> {
    let src = match src.find("#[cfg(test)]") {
        Some(i) => &src[..i],
        None => src,
    };
    let b = src.as_bytes();
    let lines: Vec<&str> = src.lines().collect();
    let mut hits = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'"' => i = skip_str(b, i).0,
            b'\'' => i = skip_char(b, i),
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b':')
                {
                    i += 1;
                }
                let word = &src[start..i];
                let name = word.rsplit("::").next().unwrap_or(word);
                let matched = CALLS.iter().any(|c| {
                    if c.contains("::") {
                        word == *c || word.ends_with(&format!("::{c}"))
                    } else {
                        name == *c
                    }
                });
                if matched && b.get(i) == Some(&b'(') {
                    for (at, text) in direct_literals(b, i) {
                        let line = src[..at].matches('\n').count() + 1;
                        let line_text = lines.get(line - 1).copied().unwrap_or("");
                        if has_words(&text)
                            && !line_text.contains("l10n-ok")
                            && !allow.contains(&text)
                        {
                            hits.push(Hit { line, text });
                        }
                    }
                }
            }
            _ => i += 1,
        }
    }
    hits
}

/// String literals at paren depth 1 of the call whose `(` is at `open`.
fn direct_literals(b: &[u8], open: usize) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            b'"' => {
                let (end, text) = skip_str(b, i);
                // Skip byte strings and anything inside nested calls/closures.
                let raw_prefix = i > 0 && (b[i - 1] == b'b');
                let in_format = depth == 2 && is_format_arg(b, i);
                if (depth == 1 || in_format) && !raw_prefix {
                    out.push((i, text));
                }
                i = end;
                continue;
            }
            b'\'' => {
                i = skip_char(b, i);
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Two or more letters in a row, ignoring escapes (`\u{b7}`, `\n`) and `{placeholders}`.
fn has_words(text: &str) -> bool {
    let mut run = 0;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if chars.next() == Some('u') {
                    for c in chars.by_ref() {
                        if c == '}' {
                            break;
                        }
                    }
                }
                run = 0;
            }
            '{' => {
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                }
                run = 0;
            }
            c if c.is_alphabetic() => {
                run += 1;
                if run >= 2 {
                    return true;
                }
            }
            _ => run = 0,
        }
    }
    false
}

/// True when the literal at `at` is the first argument of `format!(`.
fn is_format_arg(b: &[u8], at: usize) -> bool {
    let before = String::from_utf8_lossy(&b[..at]);
    let before = before.trim_end();
    before.ends_with("format!(")
}

/// Returns the index after the closing quote and the literal's raw contents.
fn skip_str(b: &[u8], start: usize) -> (usize, String) {
    let mut i = start + 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => {
                return (
                    i + 1,
                    String::from_utf8_lossy(&b[start + 1..i]).into_owned(),
                );
            }
            _ => i += 1,
        }
    }
    (b.len(), String::new())
}

/// Skips a char literal like 'x' or '\n'; a lifetime `'a` just advances one byte.
fn skip_char(b: &[u8], start: usize) -> usize {
    if b.get(start + 1) == Some(&b'\\') {
        let mut i = start + 2;
        while i < b.len() && b[i] != b'\'' {
            i += 1;
        }
        return i + 1;
    }
    // 'x' (possibly multi-byte UTF-8) closes within 5 bytes; otherwise it is a lifetime.
    for k in 2..=5 {
        if b.get(start + k) == Some(&b'\'') {
            return start + k + 1;
        }
    }
    start + 1
}

fn group_of(rel: &str) -> &'static str {
    if rel == "settings_ui.rs" || rel.starts_with("settings_ui/") {
        "settings"
    } else if rel == "history_ui.rs" || rel.starts_with("history_ui/") {
        "history"
    } else if rel.starts_with("overlay/") {
        "overlay"
    } else {
        "shell"
    }
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn load_allow() -> Vec<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/strings-allow.txt");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect()
}

#[test]
#[ignore = "fails until every group is converted; run scripts/check-strings.ps1"]
fn user_strings_use_lookup() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let skip_dir = src.join("l10n");
    let allow = load_allow();
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    files.sort();
    let mut by_group: BTreeMap<&str, Vec<(String, Vec<Hit>)>> = BTreeMap::new();
    let mut total = 0;
    for f in files.iter().filter(|f| !f.starts_with(&skip_dir)) {
        let rel = f
            .strip_prefix(&src)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let hits = scan(&std::fs::read_to_string(f).unwrap(), &allow);
        if !hits.is_empty() {
            total += hits.len();
            by_group
                .entry(group_of(&rel))
                .or_default()
                .push((rel, hits));
        }
    }
    let verbose = std::env::var("STRINGS_VERBOSE").is_ok_and(|v| v == "1");
    for (group, files) in &by_group {
        let n: usize = files.iter().map(|(_, h)| h.len()).sum();
        println!("{group}: {n}");
        for (rel, hits) in files {
            println!("  {:>4}  {rel}", hits.len());
            if verbose {
                for h in hits {
                    println!("        {rel}:{}  \"{}\"", h.line, h.text);
                }
            }
        }
    }
    println!("total: {total}");
    assert_eq!(total, 0, "hard-coded UI strings found (see counts above)");
}

#[test]
fn scanner_flags_and_skips() {
    let src = r#"
fn ui() {
    ui.label("Hello World");
    ui.label(tr!("shell.hello"));
    widgets::setting_row(
        ui,
        "Start with Windows",
        "Launch to the tray.",
        |ui| { ui.button("Inner"); },
    );
    let id = ui.scope_id().with("hist_retention");
    ui.label("PNG");
    ui.label("Skip me"); // l10n-ok
    r.on_hover_text(format!("{n} items"));
    painter.text(pos, Align2::CENTER_CENTER, "Drawn", font, color);
    ui.label(foo(format!("Nested {x}")));
    ui.label("123");
    ui.label("\u{b7}");
    ui.label("{} \u{d7} {}");
    ui.label("B");
    let c = '"';
    ui.checkbox(&mut on, "Show Loupe");
}
#[cfg(test)]
mod tests { fn t() { ui.label("Test Only"); } }
"#;
    let allow = vec!["PNG".to_string()];
    let texts: Vec<String> = scan(src, &allow).into_iter().map(|h| h.text).collect();
    assert_eq!(
        texts,
        [
            "Hello World",
            "Start with Windows",
            "Launch to the tray.",
            "Inner",
            "{n} items",
            "Drawn",
            "Show Loupe"
        ]
    );
}
