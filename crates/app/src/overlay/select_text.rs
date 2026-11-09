//! Select Text tool (W): OCR (`Windows.Media.Ocr` through `Shared::ocr`, on a worker) reads
//! the capture's words once per capture; the tool outlines the word under the pointer, a drag
//! selects words in reading order (OCR line order, then left to right), Ctrl+A selects all,
//! and Ctrl+C or the Copy Text button copies them (words on one line joined by spaces, lines
//! by newlines). Nothing is added to the image. The cache follows the base image: a new
//! region or crop reads again, switching tools keeps it.

use egui::{Align2, Area, RichText, Stroke};
use stinkshot_platform::ocr::{OcrEngine, OcrResult};

use super::paint::ImgToPt;
use super::*;
use crate::theme::Palette;

/// One recognized word, image px of the base.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub rect: RectF,
    /// Index of its OCR line (reading order).
    pub line: usize,
}

pub struct TextSelect {
    base: Arc<Rgba>,
    phase: Phase,
    /// Selected words as (anchor, cursor) indices into the words, either order.
    sel: Option<(usize, usize)>,
    hover: Option<usize>,
    copied_at: Option<Instant>,
}

enum Phase {
    Reading(Receiver<Option<Vec<Word>>>),
    Ready(Vec<Word>),
    /// OCR failed (`Ready` with no words means nothing was found).
    Failed,
    /// No OCR language installed.
    NoEngine,
}

/// How long "Copied" stays on the button.
const COPIED_SECS: Duration = Duration::from_millis(1500);
/// Grace around a word that still counts as over it, image px.
const HIT_PAD: f32 = 2.0;

/// The OCR result as words in reading order. A line without word boxes is one word.
pub fn words_from_ocr(ocr: &OcrResult) -> Vec<Word> {
    let rect = |r: &stinkshot_core::PxRect| {
        RectF::new(r.x as f32, r.y as f32, r.w.max(1) as f32, r.h.max(1) as f32)
    };
    let mut out = Vec::new();
    for (i, l) in ocr.lines.iter().enumerate() {
        if l.words.is_empty() {
            if !l.text.trim().is_empty() {
                out.push(Word {
                    text: l.text.trim().to_string(),
                    rect: rect(&l.rect),
                    line: i,
                });
            }
            continue;
        }
        for w in &l.words {
            if !w.text.trim().is_empty() {
                out.push(Word {
                    text: w.text.trim().to_string(),
                    rect: rect(&w.rect),
                    line: i,
                });
            }
        }
    }
    out
}

/// The word whose box (grown by `pad`) holds `p`.
pub fn word_at(words: &[Word], p: PtF, pad: f32) -> Option<usize> {
    words.iter().position(|w| w.rect.expand(pad).contains(p))
}

fn span_dist(p: f32, lo: f32, hi: f32) -> f32 {
    if p < lo {
        lo - p
    } else if p > hi {
        p - hi
    } else {
        0.0
    }
}

/// The word a selection drag at `p` reaches: on the line nearest `p` vertically (the line
/// under it when there is one), the word nearest horizontally.
pub fn nearest_word(words: &[Word], p: PtF) -> Option<usize> {
    let line = words
        .iter()
        .map(|w| (w.line, span_dist(p.y, w.rect.y, w.rect.y + w.rect.h)))
        .min_by(|a, b| a.1.total_cmp(&b.1))?
        .0;
    words
        .iter()
        .enumerate()
        .filter(|(_, w)| w.line == line)
        .min_by(|(_, a), (_, b)| {
            span_dist(p.x, a.rect.x, a.rect.x + a.rect.w).total_cmp(&span_dist(
                p.x,
                b.rect.x,
                b.rect.x + b.rect.w,
            ))
        })
        .map(|(i, _)| i)
}

/// (first, last) word index of a selection, inclusive.
pub fn ordered(sel: (usize, usize)) -> (usize, usize) {
    (sel.0.min(sel.1), sel.0.max(sel.1))
}

/// The text of words `first..=last`: spaces within a line, a newline between lines.
pub fn selected_text(words: &[Word], sel: (usize, usize)) -> String {
    let (a, b) = ordered(sel);
    let mut out = String::new();
    let mut prev: Option<usize> = None;
    for w in words.iter().take(b + 1).skip(a) {
        match prev {
            Some(l) if l == w.line => out.push(' '),
            Some(_) => out.push('\n'),
            None => {}
        }
        out.push_str(&w.text);
        prev = Some(w.line);
    }
    out
}

fn read(img: &Rgba, engine: &dyn OcrEngine) -> Option<Vec<Word>> {
    match engine.recognize(img) {
        Ok(r) => Some(words_from_ocr(&r)),
        Err(e) => {
            log::warn!("select text: OCR failed: {e:#}");
            None
        }
    }
}

impl TextSelect {
    fn words(&self) -> &[Word] {
        match &self.phase {
            Phase::Ready(w) => w,
            _ => &[],
        }
    }
}

impl Overlay {
    /// The Select Text tool is active on a selection.
    pub(super) fn select_text_active(&self) -> bool {
        self.tool == Tool::SelectText && self.annotating() && self.sel.is_some()
    }

    /// Each frame: start reading the capture when the tool needs words for a new base, and
    /// pick up the worker's result.
    pub(super) fn poll_select_text(&mut self, ctx: &Context, shared: &Shared) {
        if !self.select_text_active() || self.edit.is_some() {
            return;
        }
        let base = self.ensure_base();
        if !self
            .text_select
            .as_ref()
            .is_some_and(|t| Arc::ptr_eq(&t.base, &base))
        {
            let phase = match shared.ocr.clone() {
                None => {
                    self.flash_notice(tr!("overlay.select_text.no_ocr"));
                    Phase::NoEngine
                }
                Some(engine) => {
                    let (tx, rx) = channel();
                    let (img, ctx2) = (base.clone(), ctx.clone());
                    let spawned = std::thread::Builder::new()
                        .name("select-text-ocr".into())
                        .spawn(move || {
                            let _ = tx.send(read(&img, engine.as_ref()));
                            ctx2.request_repaint();
                        });
                    match spawned {
                        Ok(_) => Phase::Reading(rx),
                        Err(e) => {
                            log::error!("select text thread: {e}");
                            self.flash_notice(tr!("overlay.select_text.failed"));
                            Phase::Failed
                        }
                    }
                }
            };
            self.text_select = Some(TextSelect {
                base,
                phase,
                sel: None,
                hover: None,
                copied_at: None,
            });
        }
        let mut notice = None;
        if let Some(t) = self.text_select.as_mut()
            && let Phase::Reading(rx) = &t.phase
        {
            match rx.try_recv() {
                Ok(Some(words)) => {
                    if words.is_empty() {
                        notice = Some(tr!("overlay.select_text.none"));
                    }
                    t.phase = Phase::Ready(words);
                }
                Ok(None) | Err(TryRecvError::Disconnected) => {
                    t.phase = Phase::Failed;
                    notice = Some(tr!("overlay.select_text.failed"));
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(n) = notice {
            self.flash_notice(n);
        }
    }

    /// OCR is still running for the current capture (hint bar shows it).
    pub(super) fn select_text_reading(&self) -> bool {
        self.text_select
            .as_ref()
            .is_some_and(|t| matches!(t.phase, Phase::Reading(_)))
    }

    /// Hover: remember the word under the pointer (outlined) and say whether there is one.
    pub(super) fn select_text_hover(&mut self, p: (f32, f32)) -> bool {
        let img = self.to_image(p);
        let Some(t) = self.text_select.as_mut() else {
            return false;
        };
        t.hover = word_at(t.words(), img, HIT_PAD);
        t.hover.is_some()
    }

    /// Press: start a word selection at the word under the pointer (Shift extends the current
    /// one); a press away from every word clears it.
    pub(super) fn select_text_press(&mut self, p: (f32, f32), shift: bool) {
        let img = self.to_image(p);
        let Some(t) = self.text_select.as_mut() else {
            return;
        };
        let words = t.words();
        let at = word_at(words, img, HIT_PAD);
        t.sel = match (at, t.sel, shift) {
            (_, Some((anchor, _)), true) => nearest_word(words, img).map(|i| (anchor, i)),
            (Some(i), _, _) => Some((i, i)),
            (None, _, _) => None,
        };
        t.copied_at = None;
        if t.sel.is_some() {
            self.drag = Drag::SelectWords;
        } else {
            self.drag = Drag::Ignore;
        }
    }

    /// Drag: the selection runs from its anchor to the word nearest the pointer.
    pub(super) fn select_text_drag(&mut self, p: (f32, f32)) {
        let img = self.to_image(p);
        if let Some(t) = self.text_select.as_mut()
            && let Some((anchor, _)) = t.sel
            && let Some(i) = nearest_word(t.words(), img)
        {
            t.sel = Some((anchor, i));
        }
    }

    fn copy_selected_text(&mut self) {
        let Some(t) = self.text_select.as_mut() else {
            return;
        };
        let Some(sel) = t.sel else { return };
        let text = selected_text(t.words(), sel);
        match clipboard::set_text(&text) {
            Ok(()) => t.copied_at = Some(Instant::now()),
            Err(e) => {
                log::warn!("select text copy: {e:#}");
                self.flash_notice(tr!("overlay.notice.clipboard_busy"));
            }
        }
    }

    /// Keys of the Select Text tool: Ctrl+A selects every word, Ctrl+C copies the selected
    /// words (without a selection it copies the image as usual), Esc clears the selection.
    /// True when the key was used.
    pub(super) fn select_text_keys(&mut self, ctx: &Context, k: &Keys) -> bool {
        if !self.select_text_active() || !matches!(self.drag, Drag::None | Drag::Ignore) {
            return false;
        }
        let Some(t) = self.text_select.as_mut() else {
            return false;
        };
        if ctx.input(|i| keys::SELECT_ALL.pressed(i)) {
            let n = t.words().len();
            if n > 0 {
                t.sel = Some((0, n - 1));
                t.copied_at = None;
            }
            return true;
        }
        if t.sel.is_none() {
            return false;
        }
        if k.esc {
            t.sel = None;
            return true;
        }
        if k.copy_event && !k.enter {
            self.copy_selected_text();
            return true;
        }
        false
    }

    /// Word boxes: every word faintly, the hovered one outlined, the selected ones filled.
    pub(super) fn paint_text_select(&self, painter: &egui::Painter, m: ImgToPt, accent: Color32) {
        if !self.select_text_active() {
            return;
        }
        let Some(t) = &self.text_select else { return };
        let words = t.words();
        let faint = Stroke::new(1.0, Color32::from_white_alpha(70));
        for w in words {
            painter.rect_stroke(
                m.r(w.rect).expand(1.0),
                2.0,
                faint,
                egui::epaint::StrokeKind::Outside,
            );
        }
        if let Some(sel) = t.sel {
            let (a, b) = ordered(sel);
            let fill = Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 90);
            for w in words.iter().take(b + 1).skip(a) {
                painter.rect_filled(m.r(w.rect).expand(1.0), 2.0, fill);
            }
        }
        if let Some(w) = t.hover.and_then(|i| words.get(i)) {
            painter.rect_stroke(
                m.r(w.rect).expand(1.5),
                2.0,
                Stroke::new(1.5, accent),
                egui::epaint::StrokeKind::Outside,
            );
        }
    }

    /// The Copy Text button under the last selected word (above it near the screen bottom).
    pub(super) fn select_text_bar(&mut self, ctx: &Context, ppp: f32, pal: &Palette) {
        if !self.select_text_active() || matches!(self.drag, Drag::SelectWords) {
            return;
        }
        let Some(m) = self.img_to_pt(ppp) else { return };
        let Some(t) = &self.text_select else { return };
        let Some(sel) = t.sel else { return };
        let Some(last) = t.words().get(ordered(sel).1) else {
            return;
        };
        let r = m.r(last.rect);
        let screen = ctx.content_rect();
        let (pos, pivot) = if r.bottom() + 44.0 < screen.bottom() {
            (Pos2::new(r.right(), r.bottom() + 6.0), Align2::RIGHT_TOP)
        } else {
            (Pos2::new(r.right(), r.top() - 6.0), Align2::RIGHT_BOTTOM)
        };
        let copied = t.copied_at.is_some_and(|at| at.elapsed() < COPIED_SECS);
        if copied {
            ctx.request_repaint_after(COPIED_SECS);
        }
        let label = if copied {
            tr!("overlay.select_text.copied")
        } else {
            tr!("overlay.select_text.copy")
        };
        let mut clicked = false;
        Area::new(egui::Id::unique("stinkshot-ov-select-text"))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .pivot(pivot)
            .show(ctx, |ui| {
                super::panels::panel_frame(pal).show(ui, |ui| {
                    let b = egui::Button::new(RichText::new(label).size(13.0).color(pal.text));
                    clicked = ui.add(b).on_hover_text(keys::COPY.label).clicked();
                });
            });
        if clicked {
            self.copy_selected_text();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stinkshot_core::PxRect;
    use stinkshot_platform::ocr::{OcrLine, OcrWord};

    fn w(text: &str, x: f32, y: f32, line: usize) -> Word {
        Word {
            text: text.into(),
            rect: RectF::new(x, y, 40.0, 16.0),
            line,
        }
    }

    /// Two lines: "Hello big world" at y 10 and "second line" at y 40.
    fn sample() -> Vec<Word> {
        vec![
            w("Hello", 10.0, 10.0, 0),
            w("big", 60.0, 10.0, 0),
            w("world", 110.0, 10.0, 0),
            w("second", 10.0, 40.0, 1),
            w("line", 60.0, 40.0, 1),
        ]
    }

    #[test]
    fn words_keep_ocr_reading_order() {
        let px = |x, y| PxRect::new(x, y, 30, 12);
        let ocr = OcrResult {
            text: "a b\nwhole line\nc".into(),
            lines: vec![
                OcrLine {
                    text: "a b".into(),
                    rect: px(0, 0),
                    words: vec![
                        OcrWord {
                            text: "a".into(),
                            rect: px(0, 0),
                        },
                        OcrWord {
                            text: " ".into(),
                            rect: px(5, 0),
                        },
                        OcrWord {
                            text: "b".into(),
                            rect: px(40, 0),
                        },
                    ],
                },
                OcrLine {
                    text: " whole line ".into(),
                    rect: px(0, 20),
                    words: vec![],
                },
                OcrLine {
                    text: "c".into(),
                    rect: px(0, 40),
                    words: vec![OcrWord {
                        text: "c".into(),
                        rect: px(0, 40),
                    }],
                },
            ],
        };
        let words = words_from_ocr(&ocr);
        let texts: Vec<(&str, usize)> = words.iter().map(|w| (w.text.as_str(), w.line)).collect();
        assert_eq!(texts, [("a", 0), ("b", 0), ("whole line", 1), ("c", 2)]);
        assert_eq!(words[1].rect, RectF::new(40.0, 0.0, 30.0, 12.0));
    }

    #[test]
    fn selection_joins_words_in_reading_order() {
        let ws = sample();
        assert_eq!(selected_text(&ws, (0, 0)), "Hello");
        assert_eq!(selected_text(&ws, (1, 2)), "big world");
        // Backwards drags read the same, lines break with a newline.
        assert_eq!(selected_text(&ws, (4, 1)), "big world\nsecond line");
        assert_eq!(
            selected_text(&ws, (0, ws.len() - 1)),
            "Hello big world\nsecond line"
        );
        assert_eq!(ordered((4, 1)), (1, 4));
    }

    #[test]
    fn hover_hits_words_and_drag_reaches_the_nearest() {
        let ws = sample();
        assert_eq!(word_at(&ws, PtF::new(70.0, 15.0), 2.0), Some(1));
        assert_eq!(word_at(&ws, PtF::new(55.0, 15.0), 2.0), None);
        assert_eq!(word_at(&ws, PtF::new(55.0, 15.0), 6.0), Some(0));
        // Between words: the nearer one on the line under the pointer.
        assert_eq!(nearest_word(&ws, PtF::new(53.0, 15.0)), Some(0));
        assert_eq!(nearest_word(&ws, PtF::new(57.0, 15.0)), Some(1));
        // Past the end of a line: its last word; below everything: the last line.
        assert_eq!(nearest_word(&ws, PtF::new(400.0, 12.0)), Some(2));
        assert_eq!(nearest_word(&ws, PtF::new(70.0, 300.0)), Some(4));
        // Above the first line, left of it: the first word.
        assert_eq!(nearest_word(&ws, PtF::new(0.0, 0.0)), Some(0));
        // Between lines: the nearer line.
        assert_eq!(nearest_word(&ws, PtF::new(70.0, 37.0)), Some(4));
        assert_eq!(nearest_word(&[], PtF::new(0.0, 0.0)), None);
    }
}
