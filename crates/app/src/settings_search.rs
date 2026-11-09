//! Settings search (#11). The Settings pages stay plain imperative egui code: while a search
//! runs, Settings draws every page with a filter active, and `widgets::setting_row` asks it
//! whether its row matches (`check_row`). Rows that do not match draw nothing; rows that do
//! draw normally with the matching text highlighted. Non-row content on a page is skipped
//! while `searching()` is true.
//!
//! Matching is on the strings as shown, so it works in whatever language the UI is in. Every
//! word of the query must be found, case-insensitively, in the row's title, its description,
//! its section's name, or the section's synonyms (`settings.search.kw.*`, also translated, so
//! "shortcut" finds Hotkeys).

use std::cell::RefCell;
use std::ops::Range;

/// A parsed query: lowercase words, longest first, without duplicates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    terms: Vec<String>,
}

impl Query {
    pub fn parse(s: &str) -> Query {
        let mut terms: Vec<String> = s.split_whitespace().map(|w| fold(w).text).collect();
        terms.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        terms.dedup();
        Query { terms }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }
}

/// Lowercased text with, for each of its bytes, the byte range of the source char it came
/// from (lowercasing can change byte lengths, so hits are mapped back through this).
struct Folded {
    text: String,
    src: Vec<Range<usize>>,
}

fn fold(s: &str) -> Folded {
    let mut text = String::with_capacity(s.len());
    let mut src = Vec::with_capacity(s.len());
    for (i, c) in s.char_indices() {
        let r = i..i + c.len_utf8();
        for l in c.to_lowercase() {
            text.push(l);
            src.extend(std::iter::repeat_n(r.clone(), l.len_utf8()));
        }
    }
    Folded { text, src }
}

/// Source byte ranges where folded `term` occurs (non-overlapping, in order).
fn hits(f: &Folded, term: &str) -> Vec<Range<usize>> {
    if term.is_empty() {
        return Vec::new();
    }
    f.text
        .match_indices(term)
        .map(|(i, m)| f.src[i].start..f.src[i + m.len() - 1].end)
        .collect()
}

/// Whether a hit at byte `at` of `s` starts a word.
fn word_start(s: &str, at: usize) -> bool {
    s[..at]
        .chars()
        .next_back()
        .is_none_or(|c| !c.is_alphanumeric())
}

/// Sorted, merged ranges.
fn merge(mut v: Vec<Range<usize>>) -> Vec<Range<usize>> {
    v.sort_by_key(|r| r.start);
    let mut out: Vec<Range<usize>> = Vec::with_capacity(v.len());
    for r in v {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

/// Every place any query word occurs in `s`, for highlighting.
pub fn highlights(q: &Query, s: &str) -> Vec<Range<usize>> {
    let f = fold(s);
    merge(q.terms.iter().flat_map(|t| hits(&f, t)).collect())
}

/// A matching row: how well it matched and what to highlight.
#[derive(Clone, Debug, PartialEq)]
pub struct RowMatch {
    pub score: u32,
    pub title: Vec<Range<usize>>,
    pub desc: Vec<Range<usize>>,
}

/// Score of one word against a row; None when it is found nowhere.
fn term_score(
    term: &str,
    title: (&Folded, &str),
    desc: (&Folded, &str),
    section: &Folded,
    keywords: &Folded,
) -> Option<u32> {
    let t = hits(title.0, term);
    if t.first().is_some_and(|r| r.start == 0) {
        return Some(6);
    }
    if t.iter().any(|r| word_start(title.1, r.start)) {
        return Some(5);
    }
    if !t.is_empty() {
        return Some(3);
    }
    let d = hits(desc.0, term);
    if d.iter().any(|r| word_start(desc.1, r.start)) {
        return Some(2);
    }
    if !d.is_empty() {
        return Some(1);
    }
    // Section name, or a synonym that starts with the word ("short" finds "shortcut").
    let in_section = section.text.contains(term)
        || keywords
            .text
            .split(|c: char| c.is_whitespace() || c == ',')
            .any(|k| !k.is_empty() && k.starts_with(term));
    in_section.then_some(1)
}

/// Whether a row matches `q`, and how well. Every word must match somewhere.
pub fn match_row(
    q: &Query,
    title: &str,
    desc: &str,
    section: &str,
    keywords: &str,
) -> Option<RowMatch> {
    if q.is_empty() {
        return None;
    }
    let ft = fold(title);
    let fd = fold(desc);
    let fs = fold(section);
    let fk = fold(keywords);
    let mut score = 0;
    for term in &q.terms {
        score += term_score(term, (&ft, title), (&fd, desc), &fs, &fk)?;
    }
    let all = |f: &Folded| merge(q.terms.iter().flat_map(|t| hits(f, t)).collect());
    Some(RowMatch {
        score,
        title: all(&ft),
        desc: all(&fd),
    })
}

/// Sections ordered best first by their best row score (ties keep page order); sections
/// without a score keep their place after the scored ones.
pub fn order<T: Copy + PartialEq>(sections: &[T], scores: &[(T, u32)]) -> Vec<T> {
    let score = |s: &T| scores.iter().find(|(k, _)| k == s).map_or(0, |(_, v)| *v);
    let mut v: Vec<(usize, T)> = sections.iter().copied().enumerate().collect();
    v.sort_by(|(ia, a), (ib, b)| score(b).cmp(&score(a)).then(ia.cmp(ib)));
    v.into_iter().map(|(_, s)| s).collect()
}

// ---------------------------------------------------------------------------------------
// The filter `setting_row` consults while Settings draws search results.
// ---------------------------------------------------------------------------------------

struct Active {
    query: Query,
    section: String,
    keywords: String,
    rows: usize,
    best: u32,
}

thread_local! {
    static ACTIVE: RefCell<Option<Active>> = const { RefCell::new(None) };
}

/// While alive, `setting_row` filters against `query` for the named section.
pub struct Scope(());

impl Scope {
    /// Rows shown and the best score since `begin`.
    pub fn finish(self) -> (usize, u32) {
        ACTIVE.with(|a| a.borrow().as_ref().map_or((0, 0), |a| (a.rows, a.best)))
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with(|a| *a.borrow_mut() = None);
    }
}

pub fn begin(query: &Query, section: &str, keywords: &str) -> Scope {
    ACTIVE.with(|a| {
        *a.borrow_mut() = Some(Active {
            query: query.clone(),
            section: section.to_string(),
            keywords: keywords.to_string(),
            rows: 0,
            best: 0,
        })
    });
    Scope(())
}

/// True while search results are being drawn: pages skip everything that is not a row.
pub fn searching() -> bool {
    ACTIVE.with(|a| a.borrow().is_some())
}

/// What a row should do this frame.
pub enum RowCheck {
    /// No search: draw as usual.
    Normal,
    /// Search running, the row does not match: draw nothing.
    Hidden,
    /// Search running, the row matches: draw with these highlights.
    Hit(RowMatch),
}

pub fn check_row(title: &str, desc: &str) -> RowCheck {
    ACTIVE.with(|a| {
        let mut a = a.borrow_mut();
        let Some(a) = a.as_mut() else {
            return RowCheck::Normal;
        };
        match match_row(&a.query, title, desc, &a.section, &a.keywords) {
            Some(m) => {
                a.rows += 1;
                a.best = a.best.max(m.score);
                RowCheck::Hit(m)
            }
            None => RowCheck::Hidden,
        }
    })
}

/// `text` as a layout job with `ranges` in `mark` colors (text, background).
pub fn highlighted(
    text: &str,
    ranges: &[Range<usize>],
    font: egui::FontId,
    color: egui::Color32,
    mark: (egui::Color32, egui::Color32),
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let plain = egui::TextFormat::simple(font.clone(), color);
    let marked = egui::TextFormat {
        background: mark.1,
        ..egui::TextFormat::simple(font, mark.0)
    };
    let mut at = 0;
    for r in ranges {
        if r.start < at || r.end > text.len() || r.start >= r.end {
            continue;
        }
        if r.start > at {
            job.append(&text[at..r.start], 0.0, plain.clone());
        }
        job.append(&text[r.clone()], 0.0, marked.clone());
        at = r.end;
    }
    if at < text.len() {
        job.append(&text[at..], 0.0, plain);
    }
    job
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)] // highlight lists of one range
mod tests {
    use super::*;

    fn m(q: &str, title: &str, desc: &str) -> Option<RowMatch> {
        match_row(&Query::parse(q), title, desc, "Capture", "screenshot")
    }

    #[test]
    fn parses_words() {
        let q = Query::parse("  Cursor  MOUSE cursor ");
        assert_eq!(q.terms, vec!["cursor".to_string(), "mouse".to_string()]);
        assert!(Query::parse("   ").is_empty());
    }

    #[test]
    fn matches_title_description_and_section_case_insensitively() {
        let r = m("CURSOR", "Include Mouse Cursor", "Draw the pointer.").unwrap();
        assert_eq!(r.title, vec![14..20]);
        assert!(r.desc.is_empty());
        let r = m("pointer", "Include Mouse Cursor", "Draw the pointer.").unwrap();
        assert_eq!(r.desc, vec![9..16]);
        // Section name and section synonyms match every row of the section.
        assert!(m("capture", "Show Loupe", "Magnifier.").is_some());
        assert!(m("screen", "Show Loupe", "Magnifier.").is_some());
        assert!(m("zebra", "Show Loupe", "Magnifier.").is_none());
    }

    #[test]
    fn every_word_must_match() {
        assert!(m("mouse cursor", "Include Mouse Cursor", "").is_some());
        assert!(m("mouse zebra", "Include Mouse Cursor", "").is_none());
        // Words may match in different places.
        let r = m("cursor draw", "Include Mouse Cursor", "Draw the pointer.").unwrap();
        assert_eq!(r.title, vec![14..20]);
        assert_eq!(r.desc, vec![0..4]);
    }

    #[test]
    fn ranks_title_start_over_word_over_inside_over_description_over_section() {
        let s = |t: &str, d: &str| m("loupe", t, d).map(|r| r.score);
        let start = s("Loupe Size", "").unwrap();
        let word = s("Show Loupe", "").unwrap();
        let inside = s("Showloupe", "").unwrap();
        let desc = s("Magnifier", "The loupe follows the pointer.").unwrap();
        let desc_inside = s("Magnifier", "Theloupe.").unwrap();
        assert!(start > word && word > inside && inside > desc && desc > desc_inside);
        let section = m("capture", "Magnifier", "").unwrap().score;
        assert!(desc_inside >= section);
        // More matching words score higher.
        let two = m("mouse cursor", "Include Mouse Cursor", "").unwrap().score;
        let one = m("cursor", "Include Mouse Cursor", "").unwrap().score;
        assert!(two > one);
    }

    #[test]
    fn synonyms_match_by_prefix() {
        let q = Query::parse("shortcut");
        assert!(match_row(&q, "Region", "", "Hotkeys", "shortcuts keyboard").is_some());
        let q = Query::parse("keyb");
        assert!(match_row(&q, "Region", "", "Hotkeys", "shortcuts, keyboard").is_some());
        let q = Query::parse("board");
        assert!(match_row(&q, "Region", "", "Hotkeys", "shortcuts keyboard").is_none());
    }

    #[test]
    fn non_ascii_text_highlights_on_char_boundaries() {
        // Pseudo-localized text and a capital whose lowercase is longer in bytes (U+0130).
        let q = Query::parse("ŧĥé");
        let title = "[Ŧĥéɱé ~~~]";
        let r = match_row(&q, title, "", "", "").unwrap();
        assert_eq!(&title[r.title[0].clone()], "Ŧĥé");
        let q = Query::parse("i\u{307}x");
        let title = "A \u{130}X b";
        let r = match_row(&q, title, "", "", "").unwrap();
        assert_eq!(&title[r.title[0].clone()], "\u{130}X");
        // German capital sharp S and friends do not panic.
        let _ = highlights(&Query::parse("ss"), "STRASSE \u{1E9E}");
    }

    #[test]
    fn highlights_merge_overlaps() {
        let q = Query::parse("abc bcd");
        assert_eq!(highlights(&q, "xabcdx"), vec![1..5]);
        assert!(highlights(&Query::default(), "abc").is_empty());
    }

    #[test]
    fn sections_order_by_best_score() {
        let all = [1, 2, 3, 4];
        assert_eq!(order(&all, &[(3, 6), (1, 2), (4, 2)]), vec![3, 1, 4, 2]);
        assert_eq!(order(&all, &[]), vec![1, 2, 3, 4]);
    }

    #[test]
    fn filter_scope_counts_rows_and_clears() {
        assert!(matches!(check_row("Theme", ""), RowCheck::Normal));
        let s = begin(&Query::parse("theme"), "General", "");
        assert!(searching());
        assert!(matches!(
            check_row("Theme", "Dark or light."),
            RowCheck::Hit(_)
        ));
        assert!(matches!(check_row("Language", ""), RowCheck::Hidden));
        assert_eq!(s.finish(), (1, 6));
        assert!(!searching());
        assert!(matches!(check_row("Theme", ""), RowCheck::Normal));
    }

    #[test]
    fn highlighted_job_splits_text() {
        let job = highlighted(
            "Show Loupe",
            &[5..10],
            egui::FontId::proportional(12.0),
            egui::Color32::GRAY,
            (egui::Color32::WHITE, egui::Color32::RED),
        );
        assert_eq!(job.text, "Show Loupe");
        assert_eq!(job.sections.len(), 2);
        assert_eq!(job.sections[1].format.background, egui::Color32::RED);
    }
}
