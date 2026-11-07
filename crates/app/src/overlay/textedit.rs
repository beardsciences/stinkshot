//! Caret and selection editing for text typed straight onto the image (no text box). Pure
//! string logic on byte offsets that always sit on char boundaries; the overlay maps them to
//! screen positions with `annotate::text_layout`.

use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Caret {
    /// Where the caret is drawn (and where typing goes).
    pub cursor: usize,
    /// The other end of the selection (== cursor when nothing is selected).
    pub anchor: usize,
}

fn prev_boundary(t: &str, i: usize) -> usize {
    t[..i].char_indices().next_back().map_or(0, |(j, _)| j)
}

fn next_boundary(t: &str, i: usize) -> usize {
    t[i..].chars().next().map_or(t.len(), |c| i + c.len_utf8())
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Start of the word before `i` (Ctrl+Left / Ctrl+Backspace): skip spaces, then the word.
pub fn word_left(t: &str, i: usize) -> usize {
    let mut j = i;
    while j > 0 {
        let p = prev_boundary(t, j);
        if is_word(t[p..j].chars().next().unwrap_or(' ')) {
            break;
        }
        j = p;
    }
    while j > 0 {
        let p = prev_boundary(t, j);
        if !is_word(t[p..j].chars().next().unwrap_or(' ')) {
            break;
        }
        j = p;
    }
    j
}

/// End of the word after `i` (Ctrl+Right / Ctrl+Delete).
pub fn word_right(t: &str, i: usize) -> usize {
    let mut j = i;
    while j < t.len() && !is_word(t[j..].chars().next().unwrap_or(' ')) {
        j = next_boundary(t, j);
    }
    while j < t.len() && is_word(t[j..].chars().next().unwrap_or(' ')) {
        j = next_boundary(t, j);
    }
    j
}

/// The word (or run of non-word chars) around `i`, for double-click.
pub fn word_at(t: &str, i: usize) -> Range<usize> {
    let i = i.min(t.len());
    let at = t[i..].chars().next().or_else(|| t[..i].chars().next_back());
    let Some(c) = at else { return i..i };
    let kind = is_word(c);
    let mut a = i;
    while a > 0 {
        let p = prev_boundary(t, a);
        let ch = t[p..a].chars().next().unwrap_or(' ');
        if is_word(ch) != kind || ch == '\n' {
            break;
        }
        a = p;
    }
    let mut b = i;
    while b < t.len() {
        let ch = t[b..].chars().next().unwrap_or(' ');
        if is_word(ch) != kind || ch == '\n' {
            break;
        }
        b = next_boundary(t, b);
    }
    a..b
}

impl Caret {
    pub fn at(i: usize) -> Caret {
        Caret {
            cursor: i,
            anchor: i,
        }
    }

    pub fn has_selection(&self) -> bool {
        self.cursor != self.anchor
    }

    pub fn range(&self) -> Range<usize> {
        self.cursor.min(self.anchor)..self.cursor.max(self.anchor)
    }

    pub fn selected<'a>(&self, t: &'a str) -> &'a str {
        &t[self.range()]
    }

    /// Move the caret to `i`; `extend` keeps the anchor (Shift).
    pub fn move_to(&mut self, i: usize, extend: bool) {
        self.cursor = i;
        if !extend {
            self.anchor = i;
        }
    }

    pub fn select_all(&mut self, t: &str) {
        self.anchor = 0;
        self.cursor = t.len();
    }

    /// Replace the selection with `s` (typing, paste, Enter).
    pub fn insert(&mut self, t: &mut String, s: &str) {
        let r = self.range();
        t.replace_range(r.clone(), s);
        *self = Caret::at(r.start + s.len());
    }

    pub fn backspace(&mut self, t: &mut String, word: bool) {
        if self.has_selection() {
            self.insert(t, "");
            return;
        }
        let from = if word {
            word_left(t, self.cursor)
        } else {
            prev_boundary(t, self.cursor)
        };
        t.replace_range(from..self.cursor, "");
        *self = Caret::at(from);
    }

    pub fn delete(&mut self, t: &mut String, word: bool) {
        if self.has_selection() {
            self.insert(t, "");
            return;
        }
        let to = if word {
            word_right(t, self.cursor)
        } else {
            next_boundary(t, self.cursor)
        };
        t.replace_range(self.cursor..to, "");
    }

    /// Left / Right. Without Shift a selection collapses to its edge first.
    pub fn left(&mut self, t: &str, word: bool, extend: bool) {
        if self.has_selection() && !extend {
            *self = Caret::at(self.range().start);
            return;
        }
        let i = if word {
            word_left(t, self.cursor)
        } else {
            prev_boundary(t, self.cursor)
        };
        self.move_to(i, extend);
    }

    pub fn right(&mut self, t: &str, word: bool, extend: bool) {
        if self.has_selection() && !extend {
            *self = Caret::at(self.range().end);
            return;
        }
        let i = if word {
            word_right(t, self.cursor)
        } else {
            next_boundary(t, self.cursor)
        };
        self.move_to(i, extend);
    }

    /// Keep offsets valid after the text changed underneath (clamped to a char boundary).
    pub fn clamp(&mut self, t: &str) {
        let fix = |i: usize| {
            let mut i = i.min(t.len());
            while !t.is_char_boundary(i) {
                i -= 1;
            }
            i
        };
        self.cursor = fix(self.cursor);
        self.anchor = fix(self.anchor);
    }
}

/// Which laid-out line holds offset `i`: the last line whose range starts at or before it
/// (a wrap break's space belongs to the line before it).
pub fn line_of(lines: &[Range<usize>], i: usize) -> usize {
    lines
        .iter()
        .rposition(|r| r.start <= i)
        .unwrap_or(0)
        .min(lines.len().saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_replaces_the_selection() {
        let mut t = String::from("Hello world");
        let mut c = Caret {
            cursor: 11,
            anchor: 6,
        };
        assert_eq!(c.selected(&t), "world");
        c.insert(&mut t, "there");
        assert_eq!(t, "Hello there");
        assert_eq!(c, Caret::at(11));
        c.insert(&mut t, "\n!");
        assert_eq!(t, "Hello there\n!");
    }

    #[test]
    fn backspace_and_delete_by_char_and_word() {
        let mut t = String::from("one two three");
        let mut c = Caret::at(t.len());
        c.backspace(&mut t, false);
        assert_eq!(t, "one two thre");
        c.backspace(&mut t, true);
        assert_eq!(t, "one two ");
        c.move_to(0, false);
        c.delete(&mut t, true);
        assert_eq!(t, " two ");
        c.delete(&mut t, false);
        assert_eq!(t, "two ");
        // Nothing to delete at the ends.
        let mut c = Caret::at(0);
        c.backspace(&mut t, false);
        assert_eq!(t, "two ");
    }

    #[test]
    fn multibyte_chars_stay_whole() {
        let mut t = String::from("caf\u{e9} \u{1f600}");
        let mut c = Caret::at(t.len());
        c.backspace(&mut t, false);
        assert_eq!(t, "caf\u{e9} ");
        c.left(&t, false, false);
        c.left(&t, false, false);
        assert_eq!(c.cursor, 3);
        c.right(&t, false, true);
        assert_eq!(c.selected(&t), "\u{e9}");
        c.clamp("ca");
        assert!(c.cursor <= 2 && c.anchor <= 2);
    }

    #[test]
    fn arrows_collapse_or_extend_the_selection() {
        let t = "alpha beta";
        let mut c = Caret {
            cursor: 8,
            anchor: 2,
        };
        c.left(t, false, false);
        assert_eq!(c, Caret::at(2));
        let mut c = Caret {
            cursor: 2,
            anchor: 8,
        };
        c.right(t, false, false);
        assert_eq!(c, Caret::at(8));
        let mut c = Caret::at(0);
        c.right(t, true, true);
        assert_eq!(c.selected(t), "alpha");
        c.right(t, true, true);
        assert_eq!(c.selected(t), "alpha beta");
        // Without Shift a selection collapses to its start first.
        c.left(t, true, false);
        assert_eq!(c, Caret::at(0));
        c.select_all(t);
        assert_eq!(c.selected(t), t);
    }

    #[test]
    fn double_click_word() {
        let t = "say hello_there, ok";
        assert_eq!(&t[word_at(t, 6)], "hello_there");
        assert_eq!(&t[word_at(t, 15)], ", ");
        assert_eq!(word_at("", 0), 0..0);
        assert_eq!(&"ab\ncd"[word_at("ab\ncd", 4)], "cd");
    }

    #[test]
    fn line_lookup() {
        let lines = [0..5, 6..9, 10..10];
        assert_eq!(line_of(&lines, 0), 0);
        assert_eq!(line_of(&lines, 5), 0);
        assert_eq!(line_of(&lines, 6), 1);
        assert_eq!(line_of(&lines, 10), 2);
        assert_eq!(line_of(&[], 3), 0);
    }
}
