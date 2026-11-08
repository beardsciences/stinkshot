//! Codes and Text panel (Q): reads QR codes and barcodes in the selection
//! (`stinkshot_core::codes`, offline) and, when an OCR language is installed, its text. Each
//! code gets Copy (Copy Link for web links; links are never opened). The text offers Copy
//! Text and, when the words line up in columns (`stinkshot_core::table`), Copy as Table (TSV,
//! pastes into Excel or Sheets as cells). Runs on a worker; results are dropped when the
//! selection changes.

use egui::{Align2, Area, RichText, ScrollArea};
use stinkshot_core::codes::{self, Code};
use stinkshot_core::sensitive::TextWord;
use stinkshot_core::table::{self, Table};
use stinkshot_platform::ocr::OcrEngine;

use super::panels;
use super::*;
use crate::theme::Palette;

pub struct Scan {
    base: Arc<Rgba>,
    phase: ScanPhase,
    /// Which row's Copy was pressed last and when (for "Copied" feedback).
    copied: Option<(CopyTarget, Instant)>,
}

enum ScanPhase {
    Scanning(Receiver<ScanResult>),
    Ready(ScanResult),
}

#[derive(Debug, Default)]
pub struct ScanResult {
    pub codes: Vec<Code>,
    /// None when no OCR engine is installed.
    pub text: Option<String>,
    pub table: Option<Table>,
    /// OCR ran but failed (`text` is then empty).
    pub ocr_failed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyTarget {
    Code(usize),
    Text,
    Table,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanAction {
    Copy(CopyTarget),
    Close,
}

/// What the panel shows this frame.
pub enum ScanView<'a> {
    Scanning,
    Ready {
        result: &'a ScanResult,
        copied: Option<CopyTarget>,
        ocr: bool,
    },
}

/// How long "Copied" stays on a button.
const COPIED_SECS: Duration = Duration::from_millis(1500);
const PANEL_W: f32 = 320.0;

/// Codes, then OCR text and table (worker thread).
fn run(img: &Rgba, engine: Option<&dyn OcrEngine>) -> ScanResult {
    let codes = codes::decode(img);
    let mut ocr_failed = false;
    let (text, table) = match engine {
        None => (None, None),
        Some(e) => match e.recognize(img) {
            Ok(ocr) => {
                let lines: Vec<Vec<TextWord>> = ocr
                    .lines
                    .iter()
                    .map(|l| {
                        if l.words.is_empty() {
                            vec![TextWord {
                                text: l.text.clone(),
                                rect: l.rect,
                            }]
                        } else {
                            l.words
                                .iter()
                                .map(|w| TextWord {
                                    text: w.text.clone(),
                                    rect: w.rect,
                                })
                                .collect()
                        }
                    })
                    .collect();
                let text = ocr.text.trim().to_string();
                (Some(text), table::detect_lines(&lines))
            }
            Err(e) => {
                log::warn!("scan: OCR failed: {e:#}");
                ocr_failed = true;
                (Some(String::new()), None)
            }
        },
    };
    ScanResult {
        codes,
        text,
        table,
        ocr_failed,
    }
}

/// Text that the Copy of `target` puts on the clipboard.
pub fn copy_text(r: &ScanResult, target: CopyTarget) -> Option<String> {
    match target {
        CopyTarget::Code(i) => r.codes.get(i).map(|c| c.text.clone()),
        CopyTarget::Text => r.text.clone().filter(|t| !t.is_empty()),
        CopyTarget::Table => r.table.as_ref().map(|t| t.to_tsv()),
    }
}

/// Button label for a code: links say so (they are copied, never opened).
pub fn code_button_label(c: &Code) -> &'static str {
    if c.is_url() { "Copy Link" } else { "Copy" }
}

impl Overlay {
    /// Q or the toolbar button: start reading, or close the panel when open.
    pub(super) fn toggle_scan(&mut self, ctx: &Context, shared: &Shared) {
        if self.scan.take().is_some() {
            return;
        }
        if !self.annotating() || self.sel.is_none() {
            return;
        }
        self.commit_text();
        self.popup = None;
        self.smart = None;
        self.beautify_open = false;
        let base = self.ensure_base();
        let (tx, rx) = channel();
        let (img, ctx2, engine) = (base.clone(), ctx.clone(), shared.ocr.clone());
        let spawned = std::thread::Builder::new()
            .name("scan-codes".into())
            .spawn(move || {
                let r = run(&img, engine.as_deref());
                let _ = tx.send(r);
                ctx2.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.scan = Some(Scan {
                    base,
                    phase: ScanPhase::Scanning(rx),
                    copied: None,
                })
            }
            Err(e) => {
                log::error!("scan thread: {e}");
                self.flash_notice("Could Not Start Reading");
            }
        }
    }

    pub(super) fn poll_scan(&mut self) {
        let Some(s) = self.scan.as_mut() else { return };
        if !self.base.as_ref().is_some_and(|b| Arc::ptr_eq(b, &s.base)) {
            self.scan = None;
            return;
        }
        if let ScanPhase::Scanning(rx) = &s.phase {
            match rx.try_recv() {
                Ok(r) => s.phase = ScanPhase::Ready(r),
                Err(TryRecvError::Disconnected) => {
                    self.scan = None;
                    self.flash_notice("Could Not Read This Image");
                }
                Err(TryRecvError::Empty) => {}
            }
        }
    }

    pub(super) fn scan_view(&self, ocr: bool) -> Option<ScanView<'_>> {
        let s = self.scan.as_ref()?;
        Some(match &s.phase {
            ScanPhase::Scanning(_) => ScanView::Scanning,
            ScanPhase::Ready(result) => ScanView::Ready {
                result,
                copied: s
                    .copied
                    .filter(|(_, at)| at.elapsed() < COPIED_SECS)
                    .map(|(t, _)| t),
                ocr,
            },
        })
    }

    pub(super) fn apply_scan_action(&mut self, a: ScanAction) {
        match a {
            ScanAction::Close => self.scan = None,
            ScanAction::Copy(target) => {
                let Some(s) = self.scan.as_mut() else { return };
                let ScanPhase::Ready(r) = &s.phase else {
                    return;
                };
                let Some(text) = copy_text(r, target) else {
                    return;
                };
                match clipboard::set_text(&text) {
                    Ok(()) => s.copied = Some((target, Instant::now())),
                    Err(e) => {
                        log::warn!("scan copy: {e:#}");
                        self.flash_notice("Clipboard Busy, Try Again");
                    }
                }
            }
        }
    }

    /// Outline the codes found on the canvas.
    pub(super) fn paint_scan_codes(&self, painter: &egui::Painter, m: ImgToPt, accent: Color32) {
        let Some(Scan {
            phase: ScanPhase::Ready(r),
            ..
        }) = &self.scan
        else {
            return;
        };
        for c in &r.codes {
            if c.rect.w <= 0 || c.rect.h <= 0 {
                continue;
            }
            let rr = m.r(RectF::new(
                c.rect.x as f32,
                c.rect.y as f32,
                c.rect.w as f32,
                c.rect.h as f32,
            ));
            painter.rect_stroke(
                rr.expand(3.0),
                3.0,
                egui::Stroke::new(2.0, accent),
                egui::epaint::StrokeKind::Outside,
            );
        }
    }
}

/// Size estimate for placing the panel before it is drawn, egui points.
pub fn panel_height(view: &ScanView) -> f32 {
    match view {
        ScanView::Scanning => 44.0,
        ScanView::Ready { result, .. } => {
            90.0 + 52.0 * result.codes.len().min(4) as f32
                + if result.text.is_some() { 110.0 } else { 30.0 }
        }
    }
}

fn first_lines(text: &str, n: usize) -> String {
    let mut lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let more = lines.len() > n;
    lines.truncate(n);
    let mut s = lines.join("\n");
    if more {
        s.push('\u{2026}');
    }
    s
}

pub fn panel(
    ctx: &Context,
    pos: Pos2,
    pivot: Align2,
    view: &ScanView,
    pal: &Palette,
    accent: Color32,
    out: &mut Vec<ScanAction>,
) {
    Area::new(egui::Id::unique("stinkshot-ov-scan"))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .pivot(pivot)
        .show(ctx, |ui| {
            panels::panel_frame(pal).show(ui, |ui| match view {
                ScanView::Scanning => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(16.0).color(accent));
                        ui.label(
                            RichText::new("Reading Codes and Text\u{2026}")
                                .size(13.0)
                                .color(pal.text),
                        );
                    });
                }
                ScanView::Ready {
                    result,
                    copied,
                    ocr,
                } => ready(ui, result, *copied, *ocr, pal, out),
            });
        });
}

fn copy_button(
    ui: &mut egui::Ui,
    label: &str,
    target: CopyTarget,
    copied: Option<CopyTarget>,
    out: &mut Vec<ScanAction>,
) {
    let text = if copied == Some(target) {
        "Copied"
    } else {
        label
    };
    if theme::widgets::button(ui, text).clicked() {
        out.push(ScanAction::Copy(target));
    }
}

fn ready(
    ui: &mut egui::Ui,
    r: &ScanResult,
    copied: Option<CopyTarget>,
    ocr: bool,
    pal: &Palette,
    out: &mut Vec<ScanAction>,
) {
    ui.set_width(PANEL_W);
    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
    if panels::header(ui, "Codes and Text", pal) {
        out.push(ScanAction::Close);
    }
    if r.codes.is_empty() {
        panels::caption(ui, "No codes found.", pal);
    } else {
        ScrollArea::vertical()
            .id_salt("scan-codes")
            .max_height(52.0 * 4.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (i, c) in r.codes.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(PANEL_W - 96.0);
                            ui.label(RichText::new(c.format).size(11.5).color(pal.text_tertiary));
                            let shown = first_lines(&c.text, 2);
                            ui.add(
                                egui::Label::new(RichText::new(shown).size(13.0).color(pal.text))
                                    .truncate(),
                            )
                            .on_hover_text(&c.text);
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            copy_button(ui, code_button_label(c), CopyTarget::Code(i), copied, out);
                        });
                    });
                }
            });
    }
    ui.separator();
    match &r.text {
        None if !ocr => panels::caption(ui, "Text needs an OCR language in Windows Settings.", pal),
        None => {}
        Some(t) if t.is_empty() && r.ocr_failed => {
            panels::caption(ui, "Could not read text. Try a larger selection.", pal)
        }
        Some(t) if t.is_empty() => panels::caption(ui, "No text found.", pal),
        Some(t) => {
            ui.add(
                egui::Label::new(
                    RichText::new(first_lines(t, 3))
                        .size(12.5)
                        .color(pal.text_secondary),
                )
                .wrap(),
            );
            ui.horizontal(|ui| {
                copy_button(ui, "Copy Text", CopyTarget::Text, copied, out);
                let table_tip = match &r.table {
                    Some(tb) => format!(
                        "{} rows \u{d7} {} columns, tab-separated",
                        tb.rows.len(),
                        tb.columns
                    ),
                    None => "No columns found".to_string(),
                };
                let resp = ui
                    .add_enabled_ui(r.table.is_some(), |ui| {
                        let label = if copied == Some(CopyTarget::Table) {
                            "Copied"
                        } else {
                            "Copy as Table"
                        };
                        theme::widgets::button(ui, label)
                    })
                    .inner;
                if resp
                    .on_hover_text(&table_tip)
                    .on_disabled_hover_text(&table_tip)
                    .clicked()
                {
                    out.push(ScanAction::Copy(CopyTarget::Table));
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(text: &str) -> Code {
        Code {
            format: "QR Code",
            text: text.into(),
            rect: PxRect::default(),
        }
    }

    #[test]
    fn copy_targets() {
        let r = ScanResult {
            codes: vec![code("https://example.com"), code("plain")],
            text: Some("a b\nc d".into()),
            table: Some(Table {
                rows: vec![vec!["a".into(), "b".into()], vec!["c".into(), "d".into()]],
                columns: 2,
            }),
            ocr_failed: false,
        };
        assert_eq!(
            copy_text(&r, CopyTarget::Code(0)).as_deref(),
            Some("https://example.com")
        );
        assert_eq!(copy_text(&r, CopyTarget::Code(5)), None);
        assert_eq!(copy_text(&r, CopyTarget::Text).as_deref(), Some("a b\nc d"));
        assert_eq!(
            copy_text(&r, CopyTarget::Table).as_deref(),
            Some("a\tb\nc\td")
        );
        let empty = ScanResult {
            text: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(copy_text(&empty, CopyTarget::Text), None);
        assert_eq!(copy_text(&empty, CopyTarget::Table), None);
    }

    #[test]
    fn link_codes_are_copy_only() {
        assert_eq!(code_button_label(&code("https://x.y")), "Copy Link");
        assert_eq!(code_button_label(&code("WIFI:S:x;;")), "Copy");
    }

    #[test]
    fn previews_are_trimmed() {
        assert_eq!(first_lines("a\n\nb\nc\nd", 2), "a\nb\u{2026}");
        assert_eq!(first_lines("a", 3), "a");
    }
}
