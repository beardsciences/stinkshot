//! "Copy as Table": rebuild rows and columns from OCR word boxes so tabular text pastes into
//! Excel / Sheets as TSV.
//!
//! 1. Words are grouped into rows by vertical overlap (OCR engines often split one table row
//!    into several "lines" at wide gaps, so their line grouping is not used).
//! 2. Inside a row, a horizontal gap wider than `CELL_GAP` x the row's text height starts a
//!    new cell (normal word spacing is about a third of the height).
//! 3. Columns are the x ranges covered by the cells of most multi-cell rows; gutters are runs
//!    that (almost) no cell covers, so one cell spanning two columns does not merge them.
//! 4. Each cell lands in the first column it overlaps; cells sharing a column join with a
//!    space.
//!
//! Text is tabular when there are at least two columns and at least two rows use two or more
//! of them, and those rows are at least half of all rows.

use crate::geom::PxRect;
use crate::sensitive::TextWord;

/// A gap this many times the text height splits a row into cells.
const CELL_GAP: f32 = 1.1;
/// A gutter must be at least this many times the median text height wide.
const MIN_GUTTER: f32 = 0.6;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    /// Rows top to bottom, each with exactly `columns` cells (empty when absent).
    pub rows: Vec<Vec<String>>,
    pub columns: usize,
}

impl Table {
    /// Tab-separated values, one row per line (tabs and newlines inside cells become spaces).
    pub fn to_tsv(&self) -> String {
        self.rows
            .iter()
            .map(|r| {
                r.iter()
                    .map(|c| c.replace(['\t', '\n', '\r'], " "))
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

struct Cell {
    x0: i32,
    x1: i32,
    text: String,
}

fn median(mut v: Vec<i32>) -> i32 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[v.len() / 2]
}

/// Group words into rows (top to bottom), each sorted left to right.
fn rows_of(words: &[TextWord]) -> Vec<Vec<&TextWord>> {
    let mut ws: Vec<&TextWord> = words
        .iter()
        .filter(|w| !w.text.trim().is_empty() && w.rect.w > 0 && w.rect.h > 0)
        .collect();
    ws.sort_by_key(|w| (w.rect.y * 2 + w.rect.h, w.rect.x));
    let mut rows: Vec<(PxRect, Vec<&TextWord>)> = Vec::new();
    for w in ws {
        let cy = w.rect.y + w.rect.h / 2;
        // Join the last row whose vertical band holds this word's centre (and vice versa).
        let hit = rows.iter_mut().rev().take(3).find(|(band, _)| {
            let bcy = band.y + band.h / 2;
            cy >= band.y && cy <= band.bottom() && bcy >= w.rect.y && bcy <= w.rect.bottom()
        });
        match hit {
            Some((band, list)) => {
                list.push(w);
                // Keep the band at the row's median extent so a tall word does not swallow
                // the next row.
                let tops = median(list.iter().map(|w| w.rect.y).collect());
                let bottoms = median(list.iter().map(|w| w.rect.bottom()).collect());
                *band = PxRect::new(band.x.min(w.rect.x), tops, 1, (bottoms - tops).max(1));
            }
            None => rows.push((w.rect, vec![w])),
        }
    }
    rows.into_iter()
        .map(|(_, mut list)| {
            list.sort_by_key(|w| w.rect.x);
            list
        })
        .collect()
}

fn cells_of(row: &[&TextWord]) -> Vec<Cell> {
    let h = median(row.iter().map(|w| w.rect.h).collect()).max(1) as f32;
    let mut cells: Vec<Cell> = Vec::new();
    for w in row {
        match cells.last_mut() {
            Some(c) if ((w.rect.x - c.x1) as f32) < CELL_GAP * h => {
                c.x1 = c.x1.max(w.rect.right());
                c.text.push(' ');
                c.text.push_str(w.text.trim());
            }
            _ => cells.push(Cell {
                x0: w.rect.x,
                x1: w.rect.right(),
                text: w.text.trim().to_string(),
            }),
        }
    }
    cells
}

/// Column x ranges from the cells of multi-cell rows.
fn columns_of(rows: &[Vec<Cell>], text_h: i32) -> Vec<(i32, i32)> {
    let multi: Vec<&Vec<Cell>> = rows.iter().filter(|r| r.len() >= 2).collect();
    let (Some(lo), Some(hi)) = (
        multi.iter().flat_map(|r| r.iter().map(|c| c.x0)).min(),
        multi.iter().flat_map(|r| r.iter().map(|c| c.x1)).max(),
    ) else {
        return Vec::new();
    };
    let width = (hi - lo).max(1) as usize;
    let mut cover = vec![0u32; width];
    for r in &multi {
        for c in r.iter() {
            for v in &mut cover[(c.x0 - lo) as usize..((c.x1 - lo) as usize).min(width)] {
                *v += 1;
            }
        }
    }
    // A gutter tolerates the odd spanning cell: at most 10% of the multi-cell rows.
    let limit = (multi.len() as u32) / 10;
    let min_gutter = ((text_h as f32 * MIN_GUTTER).round() as usize).max(2);
    let mut cols = Vec::new();
    let mut start: Option<usize> = None;
    let mut gap = 0usize;
    for (x, &n) in cover.iter().enumerate() {
        if n > limit {
            if start.is_none() {
                start = Some(x);
            } else if gap >= min_gutter
                && let Some(s) = start
            {
                cols.push((lo + s as i32, lo + (x - gap) as i32));
                start = Some(x);
            }
            gap = 0;
        } else if start.is_some() {
            gap += 1;
        }
    }
    if let Some(s) = start {
        cols.push((lo + s as i32, lo + (width - gap) as i32));
    }
    cols
}

/// The column for a cell: the leftmost one it overlaps (a cell spanning columns starts in
/// its first), else the nearest to its centre.
fn column_for(cols: &[(i32, i32)], c: &Cell) -> usize {
    if let Some(i) = cols.iter().position(|(a, b)| c.x0 < *b && c.x1 > *a) {
        return i;
    }
    let cx = (c.x0 + c.x1) / 2;
    cols.iter()
        .enumerate()
        .min_by_key(|(_, (a, b))| {
            if cx < *a {
                a - cx
            } else if cx > *b {
                cx - b
            } else {
                0
            }
        })
        .map_or(0, |(i, _)| i)
}

/// Rebuild a table from OCR words (image px). None when the text is not tabular.
pub fn detect(words: &[TextWord]) -> Option<Table> {
    let rows: Vec<Vec<Cell>> = rows_of(words).iter().map(|r| cells_of(r)).collect();
    if rows.len() < 2 {
        return None;
    }
    let text_h = median(words.iter().map(|w| w.rect.h).collect()).max(1);
    let cols = columns_of(&rows, text_h);
    if cols.len() < 2 {
        return None;
    }
    let mut out = Vec::with_capacity(rows.len());
    let mut multi_col_rows = 0;
    for r in &rows {
        let mut cells = vec![String::new(); cols.len()];
        for c in r {
            let slot = &mut cells[column_for(&cols, c)];
            if !slot.is_empty() {
                slot.push(' ');
            }
            slot.push_str(&c.text);
        }
        if cells.iter().filter(|c| !c.is_empty()).count() >= 2 {
            multi_col_rows += 1;
        }
        out.push(cells);
    }
    if multi_col_rows < 2 || multi_col_rows * 2 < rows.len() {
        return None;
    }
    Some(Table {
        rows: out,
        columns: cols.len(),
    })
}

/// Convenience for OCR results grouped in lines.
pub fn detect_lines(lines: &[Vec<TextWord>]) -> Option<Table> {
    let words: Vec<TextWord> = lines.iter().flatten().cloned().collect();
    detect(&words)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lay out `rows` of cells as words: each cell's words get 8 px per character plus a 5 px
    /// space, cells start at the given column x (or right-aligned to it when negative).
    fn layout(rows: &[&[(i32, &str)]], row_h: i32, y_jitter: &[i32]) -> Vec<TextWord> {
        let mut out = Vec::new();
        for (ri, cells) in rows.iter().enumerate() {
            let y = 10 + ri as i32 * (row_h + 10) + y_jitter.get(ri).copied().unwrap_or(0);
            for (x, text) in cells.iter() {
                let total: i32 = text
                    .split(' ')
                    .map(|w| w.chars().count() as i32 * 8)
                    .sum::<i32>()
                    + 5 * (text.split(' ').count() as i32 - 1);
                let mut cx = if *x < 0 { -x - total } else { *x };
                for w in text.split(' ') {
                    let ww = w.chars().count() as i32 * 8;
                    out.push(TextWord {
                        text: w.to_string(),
                        rect: PxRect::new(cx, y, ww, row_h),
                    });
                    cx += ww + 5;
                }
            }
        }
        out
    }

    #[test]
    fn simple_grid_becomes_tsv() {
        let words = layout(
            &[
                &[(10, "Name"), (200, "Qty"), (320, "Price")],
                &[(10, "Blue widget"), (200, "4"), (320, "3.50")],
                &[(10, "Red gadget"), (200, "12"), (320, "10.00")],
            ],
            16,
            &[],
        );
        let t = detect(&words).expect("tabular");
        assert_eq!(t.columns, 3);
        assert_eq!(
            t.to_tsv(),
            "Name\tQty\tPrice\nBlue widget\t4\t3.50\nRed gadget\t12\t10.00"
        );
    }

    #[test]
    fn right_aligned_numbers_and_missing_cells() {
        // Amount column right-aligned at x=400; the middle row has no Note.
        let words = layout(
            &[
                &[(10, "Item"), (-400, "Amount"), (450, "Note")],
                &[(10, "Rent"), (-400, "1200.00"), (450, "Paid")],
                &[(10, "Power"), (-400, "84.12")],
                &[(10, "Water"), (-400, "9.90"), (450, "Late fee")],
            ],
            16,
            &[],
        );
        let t = detect(&words).expect("tabular");
        assert_eq!(t.columns, 3);
        assert_eq!(t.rows[2], vec!["Power", "84.12", ""]);
        assert_eq!(t.rows[3], vec!["Water", "9.90", "Late fee"]);
    }

    #[test]
    fn rows_survive_jitter_and_shuffled_input() {
        let mut words = layout(
            &[
                &[(10, "a"), (150, "b")],
                &[(10, "c"), (150, "d")],
                &[(10, "e"), (150, "f")],
            ],
            14,
            &[0, 3, -2],
        );
        // Baseline jitter inside a row too.
        words[1].rect.y += 2;
        words.reverse();
        let t = detect(&words).expect("tabular");
        assert_eq!(t.to_tsv(), "a\tb\nc\td\ne\tf");
    }

    #[test]
    fn prose_is_not_a_table() {
        let words = layout(
            &[
                &[(10, "The quick brown fox jumps over")],
                &[(10, "the lazy dog and keeps running")],
                &[(10, "until the end of the page")],
            ],
            16,
            &[],
        );
        assert!(detect(&words).is_none());
        // One row is not a table either.
        let one = layout(&[&[(10, "a"), (200, "b")]], 16, &[]);
        assert!(detect(&one).is_none());
    }

    #[test]
    fn spanning_title_row_does_not_merge_columns() {
        let words = layout(
            &[
                &[(10, "Quarterly results for the whole team")],
                &[(10, "Q1"), (120, "10"), (220, "20")],
                &[(10, "Q2"), (120, "11"), (220, "21")],
                &[(10, "Q3"), (120, "12"), (220, "22")],
            ],
            16,
            &[],
        );
        let t = detect(&words).expect("tabular");
        assert_eq!(t.columns, 3);
        assert_eq!(t.rows[0][0], "Quarterly results for the whole team");
        assert_eq!(t.rows[3], vec!["Q3", "12", "22"]);
    }

    #[test]
    fn cells_with_tabs_are_sanitized() {
        let t = Table {
            rows: vec![vec!["a\tb".into(), "c\nd".into()]],
            columns: 2,
        };
        assert_eq!(t.to_tsv(), "a b\tc d");
    }

    #[test]
    fn detect_lines_flattens() {
        let words = layout(
            &[&[(10, "x"), (200, "y")], &[(10, "z"), (200, "w")]],
            16,
            &[],
        );
        // Split each row into two OCR "lines", as Windows OCR does at wide gaps.
        let lines: Vec<Vec<TextWord>> = words.chunks(1).map(|c| c.to_vec()).collect();
        assert_eq!(detect_lines(&lines).unwrap().to_tsv(), "x\ty\nz\tw");
    }
}
