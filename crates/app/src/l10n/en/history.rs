//! English strings for history_ui.rs. Keys start with `history.`.

pub fn get(key: &str) -> Option<&'static str> {
    Some(match key {
        "history.search.hint" => "Search Captures",
        "history.search.clear" => "Clear Search",
        "history.delete.title" => "Delete This Capture?",
        // Placeholder example, used by the l10n tests until a converter needs it.
        "history.empty.count" => "{n} captures",
        _ => return None,
    })
}

/// Every key above, for the key test.
#[cfg(test)]
pub const KEYS: &[&str] = &[
    "history.search.hint",
    "history.search.clear",
    "history.delete.title",
    "history.empty.count",
];
