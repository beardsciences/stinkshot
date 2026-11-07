use super::*;
use crate::fonts::test_fonts;
use crate::geom::PxRect;

const WHITE: [u8; 4] = [255, 255, 255, 255];
const RED: [u8; 4] = [255, 0, 0, 255];

fn white(w: u32, h: u32) -> Rgba {
    Rgba::filled(w, h, WHITE)
}

fn style(color: [u8; 4], width: f32) -> Style {
    Style {
        color,
        width,
        fill: None,
        shadow: false,
    }
}

fn pt(x: f32, y: f32) -> PtF {
    PtF::new(x, y)
}

fn count_changed(a: &Rgba, b: &Rgba, r: PxRect) -> usize {
    let mut n = 0;
    for y in r.y..r.bottom() {
        for x in r.x..r.right() {
            if a.pixel(x as u32, y as u32) != b.pixel(x as u32, y as u32) {
                n += 1;
            }
        }
    }
    n
}

fn is_reddish(p: [u8; 4]) -> bool {
    p[0] > 200 && p[1] < 80 && p[2] < 80
}

fn count_where(img: &Rgba, r: PxRect, f: impl Fn([u8; 4]) -> bool) -> usize {
    let mut n = 0;
    for y in r.y..r.bottom() {
        for x in r.x..r.right() {
            if f(img.pixel(x as u32, y as u32)) {
                n += 1;
            }
        }
    }
    n
}

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("stinkshot-core-ann-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

// ----- model -------------------------------------------------------------------------

#[test]
fn doc_add_remove_renumber() {
    let mut d = Doc::new(100, 100);
    let a = d.add(
        ShapeKind::Step {
            center: pt(10.0, 10.0),
            n: d.next_step_number(),
            radius: 10.0,
        },
        Style::default(),
    );
    let b = d.add(
        ShapeKind::Step {
            center: pt(30.0, 10.0),
            n: d.next_step_number(),
            radius: 10.0,
        },
        Style::default(),
    );
    let c = d.add(
        ShapeKind::Step {
            center: pt(50.0, 10.0),
            n: d.next_step_number(),
            radius: 10.0,
        },
        Style::default(),
    );
    assert_eq!((a, b, c), (1, 2, 3));
    assert_eq!(d.next_step_number(), 4);
    d.remove(b);
    d.renumber_steps();
    let ns: Vec<u32> = d
        .shapes
        .iter()
        .filter_map(|s| {
            if let ShapeKind::Step { n, .. } = s.kind {
                Some(n)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(ns, vec![1, 2]);
    assert!(d.get(b).is_none());
    assert!(d.get_mut(c).is_some());
}

#[test]
fn doc_json_round_trip_all_kinds() {
    let mut d = Doc::new(300, 200);
    let r = RectF::new(1.0, 2.0, 30.0, 40.0);
    let kinds = vec![
        ShapeKind::Arrow {
            from: pt(1.0, 2.0),
            to: pt(3.0, 4.0),
        },
        ShapeKind::Line {
            from: pt(1.0, 2.0),
            to: pt(3.0, 4.0),
        },
        ShapeKind::Rect {
            rect: r,
            corner_radius: 6.0,
        },
        ShapeKind::Ellipse { rect: r },
        ShapeKind::Pen {
            points: vec![pt(0.0, 0.0), pt(1.0, 5.0)],
        },
        ShapeKind::Highlighter {
            points: vec![pt(0.0, 0.0), pt(10.0, 5.0)],
        },
        ShapeKind::Text {
            pos: pt(5.0, 5.0),
            text: "hi\nthere".into(),
            size: 18.0,
            background: Some([1, 2, 3, 255]),
        },
        ShapeKind::Step {
            center: pt(5.0, 5.0),
            n: 3,
            radius: 12.0,
        },
        ShapeKind::Blur {
            rect: r,
            radius: 9.0,
        },
        ShapeKind::Pixelate { rect: r, block: 10 },
        ShapeKind::Redact { rect: r },
        ShapeKind::Magnifier {
            source: r,
            dest: r,
            circular: true,
        },
        ShapeKind::Callout {
            rect: r,
            tail: pt(50.0, 60.0),
            text: "x".into(),
            size: 16.0,
        },
        ShapeKind::Spotlight { rect: r, dim: 0.6 },
    ];
    for k in kinds {
        d.add(
            k,
            Style {
                fill: Some([9, 9, 9, 200]),
                ..Style::default()
            },
        );
    }
    let back = Doc::from_json(&d.to_json().unwrap()).unwrap();
    assert_eq!(back, d);
}

// ----- hit testing ---------------------------------------------------------------------

#[test]
fn hit_line_is_stroke_aware() {
    let mut d = Doc::new(200, 200);
    let id = d.add(
        ShapeKind::Line {
            from: pt(10.0, 10.0),
            to: pt(110.0, 10.0),
        },
        style(RED, 6.0),
    );
    assert_eq!(d.hit_test(pt(60.0, 10.0), 0.0), Some(id));
    assert_eq!(d.hit_test(pt(60.0, 12.5), 0.0), Some(id));
    assert_eq!(d.hit_test(pt(60.0, 16.0), 0.0), None);
    assert_eq!(d.hit_test(pt(60.0, 16.0), 4.0), Some(id));
    assert_eq!(d.hit_test(pt(140.0, 10.0), 4.0), None);
}

#[test]
fn hit_rect_outline_vs_filled() {
    let mut d = Doc::new(200, 200);
    let r = RectF::new(20.0, 20.0, 100.0, 60.0);
    let outline = d.add(
        ShapeKind::Rect {
            rect: r,
            corner_radius: 0.0,
        },
        style(RED, 4.0),
    );
    assert_eq!(d.hit_test(pt(70.0, 20.0), 0.0), Some(outline));
    assert_eq!(
        d.hit_test(pt(70.0, 50.0), 0.0),
        None,
        "inside an unfilled rect is not a hit"
    );
    d.remove(outline);
    let filled = d.add(
        ShapeKind::Rect {
            rect: r,
            corner_radius: 0.0,
        },
        Style {
            fill: Some([0, 0, 255, 255]),
            ..style(RED, 4.0)
        },
    );
    assert_eq!(d.hit_test(pt(70.0, 50.0), 0.0), Some(filled));
    assert_eq!(d.hit_test(pt(70.0, 100.0), 0.0), None);
}

#[test]
fn hit_ellipse_outline_and_fill() {
    let mut d = Doc::new(200, 200);
    let r = RectF::new(20.0, 20.0, 100.0, 60.0);
    let id = d.add(ShapeKind::Ellipse { rect: r }, style(RED, 4.0));
    assert_eq!(d.hit_test(pt(20.0, 50.0), 0.0), Some(id)); // left extreme
    assert_eq!(d.hit_test(pt(70.0, 20.0), 0.0), Some(id)); // top extreme
    assert_eq!(d.hit_test(pt(70.0, 50.0), 0.0), None); // center
    // Bounding-box corner is outside the ellipse.
    assert_eq!(d.hit_test(pt(22.0, 22.0), 0.0), None);
    d.get_mut(id).unwrap().style.fill = Some([1, 1, 1, 255]);
    assert_eq!(d.hit_test(pt(70.0, 50.0), 0.0), Some(id));
    assert_eq!(d.hit_test(pt(22.0, 22.0), 0.0), None);
}

#[test]
fn hit_pen_and_highlighter() {
    let mut d = Doc::new(200, 200);
    let pen = d.add(
        ShapeKind::Pen {
            points: vec![pt(10.0, 10.0), pt(50.0, 40.0), pt(90.0, 10.0)],
        },
        style(RED, 4.0),
    );
    assert_eq!(d.hit_test(pt(30.0, 25.0), 0.0), Some(pen));
    assert_eq!(d.hit_test(pt(50.0, 10.0), 0.0), None);
    let hl = d.add(
        ShapeKind::Highlighter {
            points: vec![pt(10.0, 100.0), pt(100.0, 100.0)],
        },
        style(RED, 20.0),
    );
    assert_eq!(d.hit_test(pt(50.0, 108.0), 0.0), Some(hl));
    assert_eq!(d.hit_test(pt(50.0, 115.0), 0.0), None);
}

#[test]
fn hit_arrow_head_counts() {
    let mut d = Doc::new(200, 200);
    let id = d.add(
        ShapeKind::Arrow {
            from: pt(10.0, 100.0),
            to: pt(150.0, 100.0),
        },
        style(RED, 4.0),
    );
    assert_eq!(d.hit_test(pt(80.0, 100.0), 0.0), Some(id));
    // Beside the head, further out than the shaft would reach.
    assert_eq!(d.hit_test(pt(140.0, 106.0), 0.0), Some(id));
    assert_eq!(d.hit_test(pt(140.0, 130.0), 0.0), None);
}

#[test]
fn hit_topmost_wins_and_misses_return_none() {
    let mut d = Doc::new(200, 200);
    let r = RectF::new(10.0, 10.0, 100.0, 100.0);
    let _below = d.add(ShapeKind::Redact { rect: r }, style(RED, 1.0));
    let above = d.add(
        ShapeKind::Blur {
            rect: RectF::new(50.0, 50.0, 100.0, 100.0),
            radius: 8.0,
        },
        style(RED, 1.0),
    );
    assert_eq!(d.hit_test(pt(70.0, 70.0), 0.0), Some(above));
    assert_eq!(d.hit_test(pt(20.0, 20.0), 0.0), Some(1));
    assert_eq!(d.hit_test(pt(190.0, 190.0), 0.0), None);
}

#[test]
fn hit_step_text_magnifier_callout_spotlight() {
    let mut d = Doc::new(400, 300);
    let step = d.add(
        ShapeKind::Step {
            center: pt(30.0, 30.0),
            n: 1,
            radius: 12.0,
        },
        Style::default(),
    );
    assert_eq!(d.hit_test(pt(38.0, 30.0), 0.0), Some(step));
    assert_eq!(d.hit_test(pt(46.0, 30.0), 0.0), None);
    assert_eq!(d.hit_test(pt(46.0, 30.0), 6.0), Some(step));

    let text = d.add(
        ShapeKind::Text {
            pos: pt(100.0, 100.0),
            text: "Hello".into(),
            size: 20.0,
            background: None,
        },
        Style::default(),
    );
    assert_eq!(d.hit_test(pt(110.0, 110.0), 0.0), Some(text));
    assert_eq!(d.hit_test(pt(110.0, 200.0), 0.0), None);

    let mag = d.add(
        ShapeKind::Magnifier {
            source: RectF::new(0.0, 0.0, 20.0, 20.0),
            dest: RectF::new(200.0, 200.0, 80.0, 80.0),
            circular: false,
        },
        Style::default(),
    );
    assert_eq!(d.hit_test(pt(240.0, 240.0), 0.0), Some(mag));
    assert_eq!(d.hit_test(pt(10.0, 10.0), 0.0), None);

    let callout = d.add(
        ShapeKind::Callout {
            rect: RectF::new(250.0, 20.0, 100.0, 40.0),
            tail: pt(230.0, 120.0),
            text: "hi".into(),
            size: 14.0,
        },
        Style::default(),
    );
    assert_eq!(d.hit_test(pt(300.0, 40.0), 0.0), Some(callout));
    assert_eq!(
        d.hit_test(pt(240.0, 90.0), 0.0),
        Some(callout),
        "tail region"
    );

    // A spotlight only hits at its border, never in its clear interior.
    let mut d2 = Doc::new(200, 200);
    let inner = d2.add(
        ShapeKind::Redact {
            rect: RectF::new(60.0, 60.0, 30.0, 30.0),
        },
        Style::default(),
    );
    let spot = d2.add(
        ShapeKind::Spotlight {
            rect: RectF::new(40.0, 40.0, 100.0, 100.0),
            dim: 0.5,
        },
        Style::default(),
    );
    assert_eq!(d2.hit_test(pt(70.0, 70.0), 0.0), Some(inner));
    assert_eq!(d2.hit_test(pt(41.0, 90.0), 0.0), Some(spot));
}

// ----- bounds / translate ----------------------------------------------------------------

#[test]
fn bounds_include_stroke_and_head() {
    let f = test_fonts();
    let mut d = Doc::new(300, 300);
    let line = d.add(
        ShapeKind::Line {
            from: pt(10.0, 20.0),
            to: pt(60.0, 20.0),
        },
        style(RED, 10.0),
    );
    let b = shape_bounds(d.get(line).unwrap(), &f);
    assert_eq!((b.x, b.y, b.w, b.h), (5.0, 15.0, 60.0, 10.0));

    let rect = d.add(
        ShapeKind::Rect {
            rect: RectF::new(10.0, 10.0, 50.0, 40.0),
            corner_radius: 4.0,
        },
        style(RED, 4.0),
    );
    let b = shape_bounds(d.get(rect).unwrap(), &f);
    assert_eq!((b.x, b.y, b.w, b.h), (8.0, 8.0, 54.0, 44.0));

    let arrow = d.add(
        ShapeKind::Arrow {
            from: pt(10.0, 100.0),
            to: pt(110.0, 100.0),
        },
        style(RED, 4.0),
    );
    let b = shape_bounds(d.get(arrow).unwrap(), &f);
    assert!(b.h > 4.0 + 8.0, "head wider than shaft: {b:?}");
    assert!(b.x <= 8.0 && b.x + b.w >= 112.0);

    let pen = d.add(
        ShapeKind::Pen {
            points: vec![pt(5.0, 5.0), pt(25.0, 45.0)],
        },
        style(RED, 2.0),
    );
    let b = shape_bounds(d.get(pen).unwrap(), &f);
    assert_eq!((b.x, b.y, b.w, b.h), (4.0, 4.0, 22.0, 42.0));

    let step = d.add(
        ShapeKind::Step {
            center: pt(50.0, 50.0),
            n: 1,
            radius: 15.0,
        },
        Style::default(),
    );
    let b = shape_bounds(d.get(step).unwrap(), &f);
    assert_eq!((b.x, b.y, b.w, b.h), (35.0, 35.0, 30.0, 30.0));

    let text = d.add(
        ShapeKind::Text {
            pos: pt(20.0, 30.0),
            text: "Hello".into(),
            size: 20.0,
            background: None,
        },
        Style::default(),
    );
    let (tw, th) = f.measure("Hello", 20.0, false);
    let b = shape_bounds(d.get(text).unwrap(), &f);
    assert_eq!((b.x, b.y), (20.0, 30.0));
    assert!((b.w - tw).abs() < 1e-3 && (b.h - th).abs() < 1e-3);
    d.get_mut(text).unwrap().kind = ShapeKind::Text {
        pos: pt(20.0, 30.0),
        text: "Hello".into(),
        size: 20.0,
        background: Some(WHITE),
    };
    let b2 = shape_bounds(d.get(text).unwrap(), &f);
    assert!(b2.w > b.w && b2.x < b.x);

    let callout = d.add(
        ShapeKind::Callout {
            rect: RectF::new(100.0, 100.0, 80.0, 30.0),
            tail: pt(90.0, 200.0),
            text: "x".into(),
            size: 12.0,
        },
        style(RED, 2.0),
    );
    let b = shape_bounds(d.get(callout).unwrap(), &f);
    assert!(b.y + b.h >= 200.0 && b.x <= 90.0);
}

#[test]
fn translate_moves_everything_but_magnifier_source() {
    let mut k = ShapeKind::Callout {
        rect: RectF::new(1.0, 2.0, 3.0, 4.0),
        tail: pt(5.0, 6.0),
        text: String::new(),
        size: 10.0,
    };
    translate_shape(&mut k, 10.0, -1.0);
    assert_eq!(
        k,
        ShapeKind::Callout {
            rect: RectF::new(11.0, 1.0, 3.0, 4.0),
            tail: pt(15.0, 5.0),
            text: String::new(),
            size: 10.0
        }
    );

    let mut k = ShapeKind::Pen {
        points: vec![pt(0.0, 0.0), pt(1.0, 1.0)],
    };
    translate_shape(&mut k, 2.0, 3.0);
    assert_eq!(
        k,
        ShapeKind::Pen {
            points: vec![pt(2.0, 3.0), pt(3.0, 4.0)]
        }
    );

    let mut k = ShapeKind::Magnifier {
        source: RectF::new(0.0, 0.0, 10.0, 10.0),
        dest: RectF::new(50.0, 50.0, 40.0, 40.0),
        circular: true,
    };
    translate_shape(&mut k, 5.0, 5.0);
    assert_eq!(
        k,
        ShapeKind::Magnifier {
            source: RectF::new(0.0, 0.0, 10.0, 10.0),
            dest: RectF::new(55.0, 55.0, 40.0, 40.0),
            circular: true
        }
    );

    let mut k = ShapeKind::Arrow {
        from: pt(1.0, 1.0),
        to: pt(2.0, 2.0),
    };
    translate_shape(&mut k, -1.0, -1.0);
    assert_eq!(
        k,
        ShapeKind::Arrow {
            from: pt(0.0, 0.0),
            to: pt(1.0, 1.0)
        }
    );
}

#[test]
fn translate_keeps_hit_test_consistent() {
    let mut d = Doc::new(200, 200);
    let id = d.add(
        ShapeKind::Ellipse {
            rect: RectF::new(10.0, 10.0, 40.0, 40.0),
        },
        Style {
            fill: Some(RED),
            ..style(RED, 2.0)
        },
    );
    translate_shape(&mut d.get_mut(id).unwrap().kind, 100.0, 100.0);
    assert_eq!(d.hit_test(pt(30.0, 30.0), 0.0), None);
    assert_eq!(d.hit_test(pt(130.0, 130.0), 0.0), Some(id));
}

// ----- rendering ---------------------------------------------------------------------------

#[test]
fn render_empty_doc_is_identity_and_never_mutates_base() {
    let f = test_fonts();
    let base = white(50, 40);
    let out = render(&base, &Doc::new(50, 40), &f);
    assert_eq!(out, base);
}

#[test]
fn render_keeps_transparency_of_base() {
    let f = test_fonts();
    let mut base = Rgba::new(40, 40);
    base.set_pixel(5, 5, [10, 20, 30, 128]);
    let out = render(&base, &Doc::new(40, 40), &f);
    assert_eq!(out.pixel(5, 5), [10, 20, 30, 128]);
    assert_eq!(out.pixel(6, 6)[3], 0);
}

#[test]
fn render_line_draws_only_near_the_line_with_aa() {
    let f = test_fonts();
    let base = white(100, 60);
    let mut d = Doc::new(100, 60);
    d.add(
        ShapeKind::Line {
            from: pt(10.0, 30.0),
            to: pt(90.0, 30.0),
        },
        style(RED, 6.0),
    );
    let out = render(&base, &d, &f);
    assert!(is_reddish(out.pixel(50, 30)));
    assert!(is_reddish(out.pixel(50, 28)));
    assert_eq!(out.pixel(50, 20), WHITE);
    assert_eq!(out.pixel(50, 40), WHITE);
    // Half-pixel offset line produces partial coverage somewhere on its edge.
    let mut d2 = Doc::new(100, 60);
    d2.add(
        ShapeKind::Line {
            from: pt(10.0, 30.3),
            to: pt(90.0, 30.3),
        },
        style([0, 0, 0, 255], 3.0),
    );
    let out2 = render(&base, &d2, &f);
    let partial = (0..60)
        .filter(|&y| {
            let v = out2.pixel(50, y)[0];
            v > 10 && v < 245
        })
        .count();
    assert!(partial >= 1, "expected anti-aliased edge pixels");
    // Base unchanged.
    assert_eq!(base, white(100, 60));
}

#[test]
fn render_arrow_has_head_wider_than_shaft() {
    let f = test_fonts();
    let base = white(200, 100);
    let mut d = Doc::new(200, 100);
    d.add(
        ShapeKind::Arrow {
            from: pt(20.0, 50.0),
            to: pt(170.0, 50.0),
        },
        style(RED, 4.0),
    );
    let out = render(&base, &d, &f);
    let thickness = |x: u32| (0..100).filter(|&y| out.pixel(x, y) != WHITE).count();
    let shaft = thickness(60);
    let head = thickness(160);
    assert!((3..=7).contains(&shaft), "shaft {shaft}");
    assert!(head > shaft + 6, "head {head} vs shaft {shaft}");
    // Nothing drawn past the tip.
    assert_eq!(out.pixel(176, 50), WHITE);
    assert!(is_reddish(out.pixel(168, 50)));
}

#[test]
fn render_arrow_head_scales_with_width() {
    let f = test_fonts();
    let base = white(300, 200);
    let head_height = |w: f32| {
        let mut d = Doc::new(300, 200);
        d.add(
            ShapeKind::Arrow {
                from: pt(20.0, 100.0),
                to: pt(250.0, 100.0),
            },
            style(RED, w),
        );
        let out = render(&base, &d, &f);
        (180..250)
            .map(|x| (0..200).filter(|&y| out.pixel(x, y) != WHITE).count())
            .max()
            .unwrap_or(0)
    };
    assert!(head_height(12.0) > head_height(3.0) + 8);
}

#[test]
fn render_degenerate_shapes_do_not_panic() {
    let f = test_fonts();
    let base = white(60, 60);
    let mut d = Doc::new(60, 60);
    let s = style(RED, 4.0);
    d.add(
        ShapeKind::Arrow {
            from: pt(10.0, 10.0),
            to: pt(10.0, 10.0),
        },
        s.clone(),
    );
    d.add(
        ShapeKind::Line {
            from: pt(10.0, 10.0),
            to: pt(10.0, 10.0),
        },
        s.clone(),
    );
    d.add(
        ShapeKind::Rect {
            rect: RectF::new(5.0, 5.0, 0.0, 0.0),
            corner_radius: 3.0,
        },
        s.clone(),
    );
    d.add(
        ShapeKind::Ellipse {
            rect: RectF::new(5.0, 5.0, 0.0, 10.0),
        },
        s.clone(),
    );
    d.add(ShapeKind::Pen { points: vec![] }, s.clone());
    d.add(
        ShapeKind::Pen {
            points: vec![pt(20.0, 20.0)],
        },
        s.clone(),
    );
    d.add(
        ShapeKind::Highlighter {
            points: vec![pt(f32::NAN, 1.0)],
        },
        s.clone(),
    );
    d.add(
        ShapeKind::Text {
            pos: pt(1.0, 1.0),
            text: String::new(),
            size: 10.0,
            background: Some(RED),
        },
        s.clone(),
    );
    d.add(
        ShapeKind::Spotlight {
            rect: RectF::new(0.0, 0.0, 0.0, 0.0),
            dim: 0.5,
        },
        s.clone(),
    );
    d.add(
        ShapeKind::Magnifier {
            source: RectF::new(0.0, 0.0, 0.0, 0.0),
            dest: RectF::new(0.0, 0.0, 0.0, 0.0),
            circular: true,
        },
        s.clone(),
    );
    d.add(
        ShapeKind::Callout {
            rect: RectF::new(0.0, 0.0, 0.0, 0.0),
            tail: pt(1.0, 1.0),
            text: "x".into(),
            size: 10.0,
        },
        s.clone(),
    );
    d.add(
        ShapeKind::Blur {
            rect: RectF::new(-100.0, -100.0, 10.0, 10.0),
            radius: 3.0,
        },
        s.clone(),
    );
    d.add(
        ShapeKind::Redact {
            rect: RectF::new(500.0, 500.0, 10.0, 10.0),
        },
        s.clone(),
    );
    let out = render(&base, &d, &f);
    assert_eq!((out.width, out.height), (60, 60));
    // A zero-size base is returned as is.
    let empty = Rgba::new(0, 0);
    assert_eq!(render(&empty, &d, &f), empty);
}

#[test]
fn render_rect_outline_vs_fill_and_rounded_corner() {
    let f = test_fonts();
    let base = white(120, 90);
    let r = RectF::new(20.0, 20.0, 80.0, 50.0);
    let mut d = Doc::new(120, 90);
    d.add(
        ShapeKind::Rect {
            rect: r,
            corner_radius: 0.0,
        },
        style(RED, 4.0),
    );
    let out = render(&base, &d, &f);
    assert!(is_reddish(out.pixel(60, 20)));
    assert_eq!(out.pixel(60, 45), WHITE, "outline leaves the interior");
    assert_eq!(out.pixel(5, 5), WHITE);

    let mut d = Doc::new(120, 90);
    d.add(
        ShapeKind::Rect {
            rect: r,
            corner_radius: 16.0,
        },
        Style {
            fill: Some([0, 0, 255, 255]),
            ..style(RED, 4.0)
        },
    );
    let out = render(&base, &d, &f);
    assert_eq!(out.pixel(60, 45), [0, 0, 255, 255]);
    // The sharp corner pixel is not covered when the corner is rounded.
    assert_eq!(out.pixel(19, 19), WHITE);
    assert_eq!(out.pixel(21, 21), WHITE);
}

#[test]
fn render_ellipse_draws_extremes_not_corners() {
    let f = test_fonts();
    let base = white(120, 90);
    let mut d = Doc::new(120, 90);
    d.add(
        ShapeKind::Ellipse {
            rect: RectF::new(20.0, 20.0, 80.0, 50.0),
        },
        Style {
            fill: Some([0, 200, 0, 255]),
            ..style(RED, 3.0)
        },
    );
    let out = render(&base, &d, &f);
    assert_eq!(out.pixel(60, 45), [0, 200, 0, 255]);
    assert!(is_reddish(out.pixel(60, 20)));
    assert!(is_reddish(out.pixel(20, 45)));
    assert_eq!(out.pixel(22, 22), WHITE);
}

#[test]
fn render_pen_follows_points_and_single_point_is_a_dot() {
    let f = test_fonts();
    let base = white(100, 100);
    let mut d = Doc::new(100, 100);
    d.add(
        ShapeKind::Pen {
            points: vec![
                pt(10.0, 80.0),
                pt(30.0, 20.0),
                pt(50.0, 80.0),
                pt(70.0, 20.0),
            ],
        },
        style(RED, 4.0),
    );
    let out = render(&base, &d, &f);
    assert!(count_changed(&out, &base, PxRect::new(0, 0, 100, 100)) > 100);
    assert_eq!(out.pixel(90, 90), WHITE);
    assert!(is_reddish(out.pixel(40, 50)) || out.pixel(40, 50) != WHITE);

    let mut d = Doc::new(100, 100);
    d.add(
        ShapeKind::Pen {
            points: vec![pt(50.0, 50.0)],
        },
        style(RED, 10.0),
    );
    let out = render(&base, &d, &f);
    assert!(is_reddish(out.pixel(50, 50)));
    assert_eq!(out.pixel(50, 60), WHITE);
}

#[test]
fn render_highlighter_is_translucent() {
    let f = test_fonts();
    let mut base = white(100, 60);
    // Dark text-like bar under the highlighter must remain visible through it.
    for x in 20..80 {
        for y in 28..32 {
            base.set_pixel(x, y, [0, 0, 0, 255]);
        }
    }
    let mut d = Doc::new(100, 60);
    d.add(
        ShapeKind::Highlighter {
            points: vec![pt(10.0, 30.0), pt(90.0, 30.0)],
        },
        style([255, 235, 0, 255], 16.0),
    );
    let out = render(&base, &d, &f);
    let on_white = out.pixel(50, 24);
    assert!(
        on_white[2] < 200 && on_white[0] > 200,
        "tinted: {on_white:?}"
    );
    assert_ne!(on_white, [255, 235, 0, 255], "not opaque");
    assert!(on_white[1] > 220);
    let on_black = out.pixel(50, 29);
    assert!(on_black[0] < 150, "text still readable: {on_black:?}");
    assert_eq!(out.pixel(50, 45), WHITE);
    assert_eq!(out.pixel(50, 10), WHITE);
}

#[test]
fn render_text_draws_inside_its_bounds_only() {
    let f = test_fonts();
    let base = white(200, 80);
    let mut d = Doc::new(200, 80);
    let id = d.add(
        ShapeKind::Text {
            pos: pt(20.0, 15.0),
            text: "Hello".into(),
            size: 28.0,
            background: None,
        },
        style([0, 0, 0, 255], 4.0),
    );
    let out = render(&base, &d, &f);
    let b = shape_bounds(d.get(id).unwrap(), &f);
    let inside = b.to_px();
    let dark = count_where(&out, inside, |p| p[0] < 100);
    assert!(dark > 60, "text should leave dark pixels, got {dark}");
    // Nothing far outside the box (allowing a little for overhang).
    let outside = PxRect::new(0, 0, 200, 10);
    assert_eq!(count_changed(&out, &base, outside), 0);
    let right = PxRect::new(inside.right() + 6, 0, 200 - inside.right() - 6, 80);
    assert_eq!(count_changed(&out, &base, right), 0);
}

#[test]
fn render_text_multiline_and_shadow() {
    let f = test_fonts();
    let base = white(160, 120);
    let mk = |shadow: bool| {
        let mut d = Doc::new(160, 120);
        d.add(
            ShapeKind::Text {
                pos: pt(10.0, 10.0),
                text: "AB\nCD".into(),
                size: 30.0,
                background: None,
            },
            Style {
                shadow,
                ..style([0, 0, 0, 255], 4.0)
            },
        );
        render(&base, &d, &f)
    };
    let plain = mk(false);
    let shadowed = mk(true);
    // Second line is drawn lower: ink exists in rows of line 2 (y ~ 47..85).
    assert!(count_where(&plain, PxRect::new(0, 50, 160, 40), |p| p[0] < 100) > 30);
    // Shadow adds non-white pixels that the plain version lacks, but the same dark core.
    assert!(count_changed(&plain, &shadowed, PxRect::new(0, 0, 160, 120)) > 20);
}

#[test]
fn render_text_background_pill() {
    let f = test_fonts();
    let base = white(200, 80);
    let mut d = Doc::new(200, 80);
    let id = d.add(
        ShapeKind::Text {
            pos: pt(30.0, 20.0),
            text: "Hi".into(),
            size: 24.0,
            background: Some([20, 20, 120, 255]),
        },
        style([255, 255, 255, 255], 4.0),
    );
    let out = render(&base, &d, &f);
    let b = shape_bounds(d.get(id).unwrap(), &f);
    // Pill padding region is background-colored, and beyond the pill is white.
    assert_eq!(
        out.pixel(b.x as u32 + 3, (b.y + b.h / 2.0) as u32),
        [20, 20, 120, 255]
    );
    assert_eq!(out.pixel(b.x as u32 - 3, (b.y + b.h / 2.0) as u32), WHITE);
    assert_eq!(out.pixel(5, 5), WHITE);
}

#[test]
fn render_step_circle_with_number() {
    let f = test_fonts();
    let base = white(100, 100);
    let mut d = Doc::new(100, 100);
    d.add(
        ShapeKind::Step {
            center: pt(50.0, 50.0),
            n: 7,
            radius: 20.0,
        },
        style([30, 60, 200, 255], 4.0),
    );
    let out = render(&base, &d, &f);
    // Disc color near the edge (inside the white ring), outside untouched.
    assert_eq!(out.pixel(50, 50 - 15)[2], 200);
    assert_eq!(out.pixel(50, 50 + 25), WHITE);
    assert_eq!(out.pixel(50 - 25, 50), WHITE);
    // Number is drawn in a contrasting (white) color near the center.
    let ink = count_where(&out, PxRect::new(38, 38, 24, 24), |p| {
        p[0] > 200 && p[1] > 200
    });
    assert!(ink > 15, "digit pixels: {ink}");
    // Digit ink is roughly centered.
    let mut sx = 0.0;
    let mut sy = 0.0;
    let mut n = 0.0;
    for y in 38..62 {
        for x in 38..62 {
            let p = out.pixel(x, y);
            if p[0] > 200 && p[1] > 200 {
                sx += x as f32;
                sy += y as f32;
                n += 1.0;
            }
        }
    }
    assert!(
        (sx / n - 49.5).abs() < 4.0 && (sy / n - 49.5).abs() < 4.0,
        "centroid {} {}",
        sx / n,
        sy / n
    );
}

#[test]
fn render_step_number_text_adapts_to_light_color() {
    let f = test_fonts();
    let base = Rgba::filled(100, 100, [0, 0, 0, 255]);
    let mut d = Doc::new(100, 100);
    d.add(
        ShapeKind::Step {
            center: pt(50.0, 50.0),
            n: 12,
            radius: 20.0,
        },
        style([255, 230, 90, 255], 4.0),
    );
    let out = render(&base, &d, &f);
    let dark_ink = count_where(&out, PxRect::new(36, 36, 28, 28), |p| {
        p[0] < 90 && p[1] < 90
    });
    assert!(dark_ink > 20, "{dark_ink}");
}

#[test]
fn render_callout_has_bubble_tail_and_text() {
    let f = test_fonts();
    let base = white(240, 160);
    let mut d = Doc::new(240, 160);
    d.add(
        ShapeKind::Callout {
            rect: RectF::new(60.0, 20.0, 120.0, 50.0),
            tail: pt(90.0, 120.0),
            text: "Look here".into(),
            size: 16.0,
        },
        style([20, 20, 120, 255], 4.0),
    );
    let out = render(&base, &d, &f);
    // Bubble body.
    assert_eq!(
        out.pixel(65, 25)[2],
        120,
        "body color near the corner interior"
    );
    // Tail tip region is colored; far away is not.
    assert!(
        out.pixel(95, 108)[0] < 100,
        "tail: {:?}",
        out.pixel(95, 108)
    );
    assert_eq!(out.pixel(200, 110), WHITE);
    assert_eq!(out.pixel(20, 100), WHITE);
    // Light text on the dark bubble.
    let light = count_where(&out, PxRect::new(65, 28, 110, 40), |p| {
        p[0] > 200 && p[1] > 200
    });
    assert!(light > 40, "text pixels: {light}");
}

#[test]
fn render_callout_tail_sides() {
    let f = test_fonts();
    let base = white(300, 300);
    let r = RectF::new(100.0, 100.0, 100.0, 60.0);
    for (tail, probe) in [
        (pt(150.0, 40.0), (150u32, 80u32)), // above
        (pt(150.0, 260.0), (150, 200)),     // below
        (pt(40.0, 130.0), (80, 130)),       // left
        (pt(260.0, 130.0), (220, 130)),     // right
    ] {
        let mut d = Doc::new(300, 300);
        d.add(
            ShapeKind::Callout {
                rect: r,
                tail,
                text: String::new(),
                size: 14.0,
            },
            style([220, 0, 0, 255], 2.0),
        );
        let out = render(&base, &d, &f);
        assert!(
            is_reddish(out.pixel(probe.0, probe.1)),
            "tail {tail:?} probe {probe:?} -> {:?}",
            out.pixel(probe.0, probe.1)
        );
    }
}

#[test]
fn render_callout_wraps_long_text_inside_rect() {
    let f = test_fonts();
    let base = white(260, 200);
    let mut d = Doc::new(260, 200);
    d.add(
        ShapeKind::Callout {
            rect: RectF::new(20.0, 20.0, 140.0, 100.0),
            tail: pt(30.0, 150.0),
            text:
                "This is a fairly long sentence that must wrap onto several lines inside the bubble"
                    .into(),
            size: 14.0,
        },
        style([0, 0, 0, 255], 2.0),
    );
    let out = render(&base, &d, &f);
    // Ink in several different bands of the bubble, nothing to the right of it.
    let mut runs = 0;
    let mut prev = false;
    for y in 22..118 {
        let row = count_where(&out, PxRect::new(40, y, 100, 1), |p| p[0] > 200) > 0;
        if row && !prev {
            runs += 1;
        }
        prev = row;
    }
    assert!(runs >= 3, "wrapped lines: {runs}");
    assert_eq!(count_changed(&out, &base, PxRect::new(170, 0, 90, 200)), 0);
}

#[test]
fn render_spotlight_dims_outside_only_and_unions() {
    let f = test_fonts();
    let base = white(200, 100);
    let mut d = Doc::new(200, 100);
    d.add(
        ShapeKind::Spotlight {
            rect: RectF::new(20.0, 20.0, 50.0, 50.0),
            dim: 0.6,
        },
        style(RED, 1.0),
    );
    let out = render(&base, &d, &f);
    assert_eq!(out.pixel(40, 40), WHITE, "inside stays clear");
    let outside = out.pixel(150, 50);
    assert!(
        (outside[0] as i32 - 102).abs() <= 2,
        "dimmed to ~40%: {outside:?}"
    );
    assert_eq!(outside[3], 255);

    d.add(
        ShapeKind::Spotlight {
            rect: RectF::new(120.0, 20.0, 50.0, 50.0),
            dim: 0.6,
        },
        style(RED, 1.0),
    );
    let out = render(&base, &d, &f);
    assert_eq!(out.pixel(40, 40), WHITE);
    assert_eq!(out.pixel(140, 40), WHITE, "both clear areas stay clear");
    assert!(out.pixel(95, 40)[0] < 140, "gap between them is dimmed");
}

#[test]
fn spotlight_dimming_is_under_vector_shapes() {
    let f = test_fonts();
    let base = white(120, 80);
    let mut d = Doc::new(120, 80);
    d.add(
        ShapeKind::Spotlight {
            rect: RectF::new(10.0, 10.0, 30.0, 30.0),
            dim: 0.8,
        },
        style(RED, 1.0),
    );
    d.add(
        ShapeKind::Rect {
            rect: RectF::new(60.0, 10.0, 40.0, 40.0),
            corner_radius: 0.0,
        },
        Style {
            fill: Some([255, 0, 0, 255]),
            ..style(RED, 2.0)
        },
    );
    let out = render(&base, &d, &f);
    assert_eq!(
        out.pixel(80, 30),
        RED,
        "annotation outside the spotlight is not dimmed"
    );
}

#[test]
fn render_redact_blur_pixelate_change_pixels_in_rect_only() {
    let f = test_fonts();
    let mut base = white(120, 80);
    for y in 0..80u32 {
        for x in 0..120u32 {
            if (x / 2 + y / 3) % 2 == 0 {
                base.set_pixel(x, y, [0, 0, 0, 255]);
            }
        }
    }
    let area = PxRect::new(10, 10, 50, 40);
    let rect = RectF::new(10.0, 10.0, 50.0, 40.0);
    for kind in [
        ShapeKind::Redact { rect },
        ShapeKind::Blur { rect, radius: 8.0 },
        ShapeKind::Pixelate { rect, block: 10 },
    ] {
        let mut d = Doc::new(120, 80);
        d.add(kind.clone(), style([40, 40, 40, 255], 4.0));
        let out = render(&base, &d, &f);
        assert!(
            count_changed(&out, &base, area) > 500,
            "{kind:?} should change the area"
        );
        assert_eq!(
            count_changed(&out, &base, PxRect::new(70, 0, 50, 80)),
            0,
            "{kind:?} leaked outside"
        );
        assert_eq!(count_changed(&out, &base, PxRect::new(0, 55, 120, 25)), 0);
    }
    // Redact is an exact opaque fill, even with a translucent style color.
    let mut d = Doc::new(120, 80);
    d.add(ShapeKind::Redact { rect }, style([10, 20, 30, 40], 4.0));
    let out = render(&base, &d, &f);
    assert_eq!(out.pixel(30, 30), [10, 20, 30, 255]);
}

#[test]
fn magnifier_zooms_and_does_not_reveal_redacted_pixels() {
    let f = test_fonts();
    let mut base = white(200, 120);
    // Secret: a red patch.
    for y in 20..40 {
        for x in 20..40 {
            base.set_pixel(x, y, RED);
        }
    }
    let dest = RectF::new(110.0, 40.0, 80.0, 60.0);
    let source = RectF::new(15.0, 15.0, 30.0, 30.0);

    // Without redaction the magnifier shows the red patch enlarged.
    let mut d = Doc::new(200, 120);
    d.add(
        ShapeKind::Magnifier {
            source,
            dest,
            circular: false,
        },
        style(RED, 4.0),
    );
    let out = render(&base, &d, &f);
    let zoomed_red = count_where(&out, PxRect::new(112, 42, 76, 56), is_reddish);
    assert!(
        zoomed_red > 800,
        "zoomed patch should be large: {zoomed_red}"
    );
    // Source stays intact in the output.
    assert!(is_reddish(out.pixel(30, 30)));

    // With a redaction box over the patch (drawn LATER in z-order than... even if earlier),
    // the magnified copy must not contain any secret pixels.
    for redact_first in [true, false] {
        let mut d = Doc::new(200, 120);
        let redact = ShapeKind::Redact {
            rect: RectF::new(15.0, 15.0, 30.0, 30.0),
        };
        let mag = ShapeKind::Magnifier {
            source,
            dest,
            circular: false,
        };
        if redact_first {
            d.add(redact, style([0, 0, 0, 255], 4.0));
            d.add(mag, style(RED, 4.0));
        } else {
            d.add(mag, style(RED, 4.0));
            d.add(redact, style([0, 0, 0, 255], 4.0));
        }
        let out = render(&base, &d, &f);
        assert_eq!(
            count_where(&out, PxRect::new(20, 20, 20, 20), is_reddish),
            0,
            "source redacted"
        );
        // The ring is white but the interior is black; no red from the base survives.
        assert_eq!(
            count_where(&out, PxRect::new(114, 44, 72, 52), is_reddish),
            0,
            "magnifier leaked secret (redact_first={redact_first})"
        );
        let black = count_where(&out, PxRect::new(114, 44, 72, 52), |p| {
            p[0] < 20 && p[1] < 20 && p[2] < 20
        });
        assert!(black > 1500, "magnifier shows the black box: {black}");
    }

    // Blur and pixelate are respected too.
    for kind in [
        ShapeKind::Blur {
            rect: RectF::new(15.0, 15.0, 30.0, 30.0),
            radius: 14.0,
        },
        ShapeKind::Pixelate {
            rect: RectF::new(15.0, 15.0, 30.0, 30.0),
            block: 30,
        },
    ] {
        let mut d = Doc::new(200, 120);
        d.add(kind, style(RED, 4.0));
        d.add(
            ShapeKind::Magnifier {
                source,
                dest,
                circular: false,
            },
            style(RED, 4.0),
        );
        let flat = render(&base, &d, &f);
        // Magnified region equals a scaled copy of the redacted flat region: its darkest-red
        // pure pixels (255,0,0) are rare compared to an unredacted copy.
        let pure = count_where(&flat, PxRect::new(114, 44, 72, 52), |p| {
            p[0] == 255 && p[1] == 0 && p[2] == 0
        });
        assert!(pure < zoomed_red / 4, "pure secret pixels leaked: {pure}");
    }
}

#[test]
fn magnifier_circular_clips_to_circle_and_has_white_ring() {
    let f = test_fonts();
    let base = Rgba::filled(160, 160, [0, 0, 255, 255]);
    let mut d = Doc::new(160, 160);
    d.add(
        ShapeKind::Magnifier {
            source: RectF::new(10.0, 10.0, 20.0, 20.0),
            dest: RectF::new(60.0, 60.0, 80.0, 80.0),
            circular: true,
        },
        style([255, 0, 0, 255], 4.0),
    );
    let out = render(&base, &d, &f);
    // Center of the inset is the magnified base (blue), corner of dest box is untouched.
    assert_eq!(out.pixel(100, 100), [0, 0, 255, 255]);
    assert_eq!(out.pixel(62, 62), [0, 0, 255, 255]);
    // Ring: white pixels at the left extreme of the circle.
    let ring = count_where(&out, PxRect::new(56, 96, 10, 8), |p| {
        p[0] > 230 && p[1] > 230 && p[2] > 230
    });
    assert!(ring > 8, "ring pixels {ring}");
}

#[test]
fn magnifier_works_at_image_edges_and_with_out_of_bounds_source() {
    let f = test_fonts();
    let base = white(100, 100);
    let mut d = Doc::new(100, 100);
    d.add(
        ShapeKind::Magnifier {
            source: RectF::new(-20.0, -20.0, 40.0, 40.0),
            dest: RectF::new(50.0, 50.0, 60.0, 60.0),
            circular: false,
        },
        Style::default(),
    );
    let out = render(&base, &d, &f);
    assert_eq!(out.pixel(80, 80)[3], 255);
}

#[test]
fn z_order_later_shapes_cover_earlier() {
    let f = test_fonts();
    let base = white(80, 60);
    let r = RectF::new(10.0, 10.0, 50.0, 40.0);
    let mut d = Doc::new(80, 60);
    d.add(
        ShapeKind::Rect {
            rect: r,
            corner_radius: 0.0,
        },
        Style {
            fill: Some([255, 0, 0, 255]),
            ..style(RED, 1.0)
        },
    );
    d.add(
        ShapeKind::Rect {
            rect: r,
            corner_radius: 0.0,
        },
        Style {
            fill: Some([0, 255, 0, 255]),
            ..style([0, 255, 0, 255], 1.0)
        },
    );
    assert_eq!(render(&base, &d, &f).pixel(30, 30), [0, 255, 0, 255]);
}

#[test]
fn redact_covers_earlier_vector_shapes() {
    let f = test_fonts();
    let base = white(80, 60);
    let mut d = Doc::new(80, 60);
    d.add(
        ShapeKind::Rect {
            rect: RectF::new(10.0, 10.0, 50.0, 40.0),
            corner_radius: 0.0,
        },
        Style {
            fill: Some([255, 0, 0, 255]),
            ..style(RED, 1.0)
        },
    );
    d.add(
        ShapeKind::Redact {
            rect: RectF::new(20.0, 20.0, 20.0, 20.0),
        },
        style([0, 0, 0, 255], 1.0),
    );
    let out = render(&base, &d, &f);
    assert_eq!(out.pixel(30, 30), [0, 0, 0, 255]);
    assert_eq!(out.pixel(15, 15), [255, 0, 0, 255]);
}

#[test]
fn shadow_adds_pixels_below_the_stroke() {
    let f = test_fonts();
    let base = white(100, 60);
    let mk = |shadow: bool| {
        let mut d = Doc::new(100, 60);
        d.add(
            ShapeKind::Line {
                from: pt(10.0, 30.0),
                to: pt(90.0, 30.0),
            },
            Style {
                shadow,
                ..style(RED, 6.0)
            },
        );
        render(&base, &d, &f)
    };
    let (a, b) = (mk(false), mk(true));
    assert_eq!(a.pixel(50, 34), WHITE);
    assert!(
        b.pixel(50, 34)[1] < 250,
        "shadow visible: {:?}",
        b.pixel(50, 34)
    );
    assert!(is_reddish(b.pixel(50, 30)));
}

// ----- render_subset -------------------------------------------------------------------------

#[test]
fn subset_renders_only_requested_shapes_on_transparent_canvas() {
    let f = test_fonts();
    let base = white(100, 60);
    let mut d = Doc::new(100, 60);
    let a = d.add(
        ShapeKind::Line {
            from: pt(10.0, 15.0),
            to: pt(90.0, 15.0),
        },
        style(RED, 6.0),
    );
    let b = d.add(
        ShapeKind::Line {
            from: pt(10.0, 45.0),
            to: pt(90.0, 45.0),
        },
        style([0, 0, 255, 255], 6.0),
    );
    let out = render_subset(&base, &d, &[b], &f);
    assert_eq!(out.pixel(50, 15)[3], 0, "other shape not drawn");
    assert_eq!(out.pixel(50, 5)[3], 0, "canvas transparent");
    let p = out.pixel(50, 45);
    assert!(p[2] > 200 && p[3] == 255, "{p:?}");
    // Unknown ids are ignored; empty list is fully transparent.
    let none = render_subset(&base, &d, &[999], &f);
    assert!(none.data.iter().all(|&v| v == 0));
    let both = render_subset(&base, &d, &[a, b], &f);
    assert!(both.pixel(50, 15)[3] == 255 && both.pixel(50, 45)[3] == 255);
}

#[test]
fn subset_destructive_shapes_show_effect_from_base() {
    let f = test_fonts();
    let mut base = white(80, 60);
    for y in 0..60u32 {
        for x in 0..80u32 {
            if x % 2 == 0 {
                base.set_pixel(x, y, [0, 0, 0, 255]);
            }
        }
    }
    let mut d = Doc::new(80, 60);
    let red = d.add(
        ShapeKind::Redact {
            rect: RectF::new(10.0, 10.0, 20.0, 20.0),
        },
        style([5, 6, 7, 255], 1.0),
    );
    let blur = d.add(
        ShapeKind::Blur {
            rect: RectF::new(40.0, 10.0, 30.0, 30.0),
            radius: 8.0,
        },
        style(RED, 1.0),
    );
    let out = render_subset(&base, &d, &[red, blur], &f);
    assert_eq!(out.pixel(15, 15), [5, 6, 7, 255]);
    let p = out.pixel(55, 25);
    assert_eq!(p[3], 255);
    assert!(p[0] > 60 && p[0] < 200, "blurred mid-gray: {p:?}");
    assert_eq!(out.pixel(5, 5)[3], 0);
}

// ----- sidecars -------------------------------------------------------------------------------

#[test]
fn sidecar_round_trip_and_layout() {
    let dir = tmpdir("sidecar");
    let flat = dir.join("Shot.png");
    let mut base = white(30, 20);
    base.set_pixel(3, 4, [9, 8, 7, 255]);
    let mut doc = Doc::new(30, 20);
    doc.add(
        ShapeKind::Redact {
            rect: RectF::new(1.0, 1.0, 5.0, 5.0),
        },
        Style::default(),
    );
    doc.add(
        ShapeKind::Text {
            pos: pt(2.0, 2.0),
            text: "ünï\ncode".into(),
            size: 12.0,
            background: None,
        },
        Style::default(),
    );

    assert!(!has_editable(&flat));
    assert!(load_editable(&flat).unwrap().is_none());

    save_editable(&flat, &base, &doc).unwrap();
    assert!(has_editable(&flat));
    assert!(
        dir.join(".stinkshot")
            .join("edit")
            .join("Shot.png.json")
            .is_file()
    );
    assert!(
        dir.join(".stinkshot")
            .join("edit")
            .join("Shot.png.base.png")
            .is_file()
    );

    let (b2, d2) = load_editable(&flat).unwrap().unwrap();
    assert_eq!(b2, base);
    assert_eq!(d2, doc);

    // Overwrite with a changed doc.
    doc.add(
        ShapeKind::Line {
            from: pt(0.0, 0.0),
            to: pt(5.0, 5.0),
        },
        Style::default(),
    );
    save_editable(&flat, &base, &doc).unwrap();
    assert_eq!(load_editable(&flat).unwrap().unwrap().1.shapes.len(), 3);

    delete_editable(&flat).unwrap();
    assert!(!has_editable(&flat));
    assert!(load_editable(&flat).unwrap().is_none());
    // Deleting again is fine.
    delete_editable(&flat).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sidecars_are_per_file_and_tolerate_partial_state() {
    let dir = tmpdir("sidecar2");
    let a = dir.join("a.png");
    let b = dir.join("b.jpg");
    let base = white(10, 10);
    let doc = Doc::new(10, 10);
    save_editable(&a, &base, &doc).unwrap();
    assert!(has_editable(&a));
    assert!(!has_editable(&b));

    // Missing base -> treated as no sidecar.
    std::fs::remove_file(dir.join(".stinkshot").join("edit").join("a.png.base.png")).unwrap();
    assert!(!has_editable(&a));
    assert!(load_editable(&a).unwrap().is_none());

    // Corrupt json -> error, not panic.
    save_editable(&a, &base, &doc).unwrap();
    std::fs::write(
        dir.join(".stinkshot").join("edit").join("a.png.json"),
        "{ nope",
    )
    .unwrap();
    assert!(load_editable(&a).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn same_stem_different_extension_do_not_share_a_sidecar() {
    let dir = tmpdir("sidecar_ext");
    let png = dir.join("Foo.png");
    let webp = dir.join("Foo.webp");
    let base = white(10, 10);
    let mut d1 = Doc::new(10, 10);
    d1.add(
        ShapeKind::Line {
            from: pt(0.0, 0.0),
            to: pt(5.0, 5.0),
        },
        Style::default(),
    );
    let d2 = Doc::new(10, 10);
    save_editable(&png, &base, &d1).unwrap();
    save_editable(&webp, &base, &d2).unwrap();
    assert_eq!(load_editable(&png).unwrap().unwrap().1.shapes.len(), 1);
    assert_eq!(load_editable(&webp).unwrap().unwrap().1.shapes.len(), 0);
    delete_editable(&webp).unwrap();
    assert!(has_editable(&png));
    assert!(!has_editable(&webp));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn legacy_stem_keyed_sidecar_still_loads() {
    let dir = tmpdir("sidecar_legacy");
    let flat = dir.join("Old.png");
    std::fs::write(&flat, b"x").unwrap();
    let base = white(6, 6);
    let doc = Doc::new(6, 6);
    let edit = dir.join(".stinkshot").join("edit");
    std::fs::create_dir_all(&edit).unwrap();
    std::fs::write(edit.join("Old.json"), doc.to_json().unwrap()).unwrap();
    std::fs::write(edit.join("Old.base.png"), base.to_png().unwrap()).unwrap();
    assert!(has_editable(&flat));
    assert!(load_editable(&flat).unwrap().is_some());
    assert_eq!(editable_files(&flat).len(), 2);
    delete_editable(&flat).unwrap();
    assert!(!has_editable(&flat));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sidecar_flatten_matches_original_render() {
    let f = test_fonts();
    let dir = tmpdir("sidecar3");
    let flat = dir.join("x.png");
    let base = white(60, 40);
    let mut doc = Doc::new(60, 40);
    doc.add(
        ShapeKind::Rect {
            rect: RectF::new(5.0, 5.0, 30.0, 20.0),
            corner_radius: 4.0,
        },
        style(RED, 3.0),
    );
    let first = render(&base, &doc, &f);
    save_editable(&flat, &base, &doc).unwrap();
    let (b2, d2) = load_editable(&flat).unwrap().unwrap();
    assert_eq!(render(&b2, &d2, &f), first);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Visual smoke test: `STINKSHOT_SHOWCASE_OUT=<file.png> cargo test -p stinkshot-core showcase -- --ignored`.
#[test]
#[ignore]
fn showcase() {
    let Ok(out_path) = std::env::var("STINKSHOT_SHOWCASE_OUT") else {
        return;
    };
    let f = test_fonts();
    let mut base = Rgba::filled(640, 400, [245, 246, 250, 255]);
    for y in 0..400u32 {
        for x in 0..640u32 {
            if ((x / 8) + (y / 8)) % 2 == 0 {
                base.set_pixel(x, y, [225, 230, 240, 255]);
            }
        }
    }
    let mut d = Doc::new(640, 400);
    let orange = Style::default();
    d.add(
        ShapeKind::Arrow {
            from: pt(40.0, 60.0),
            to: pt(200.0, 120.0),
        },
        orange.clone(),
    );
    d.add(
        ShapeKind::Arrow {
            from: pt(40.0, 160.0),
            to: pt(220.0, 160.0),
        },
        Style {
            width: 9.0,
            ..orange.clone()
        },
    );
    d.add(
        ShapeKind::Line {
            from: pt(40.0, 190.0),
            to: pt(220.0, 200.0),
        },
        orange.clone(),
    );
    d.add(
        ShapeKind::Rect {
            rect: RectF::new(250.0, 30.0, 120.0, 70.0),
            corner_radius: 12.0,
        },
        orange.clone(),
    );
    d.add(
        ShapeKind::Ellipse {
            rect: RectF::new(400.0, 30.0, 110.0, 70.0),
        },
        Style {
            fill: Some([76, 194, 255, 90]),
            ..orange.clone()
        },
    );
    d.add(
        ShapeKind::Pen {
            points: (0..40)
                .map(|i| {
                    pt(
                        260.0 + i as f32 * 6.0,
                        140.0 + (i as f32 * 0.5).sin() * 20.0,
                    )
                })
                .collect(),
        },
        orange.clone(),
    );
    d.add(
        ShapeKind::Highlighter {
            points: vec![pt(250.0, 200.0), pt(440.0, 200.0)],
        },
        Style {
            color: [255, 230, 0, 255],
            width: 22.0,
            ..orange.clone()
        },
    );
    d.add(
        ShapeKind::Text {
            pos: pt(255.0, 190.0),
            text: "Highlighted text".into(),
            size: 22.0,
            background: None,
        },
        Style {
            color: [20, 20, 20, 255],
            ..orange.clone()
        },
    );
    d.add(
        ShapeKind::Text {
            pos: pt(40.0, 230.0),
            text: "Label with pill".into(),
            size: 22.0,
            background: Some([17, 17, 17, 230]),
        },
        Style {
            color: [255, 255, 255, 255],
            ..orange.clone()
        },
    );
    for i in 0..3 {
        d.add(
            ShapeKind::Step {
                center: pt(60.0 + i as f32 * 50.0, 310.0),
                n: i + 1,
                radius: 18.0,
            },
            orange.clone(),
        );
    }
    d.add(
        ShapeKind::Callout {
            rect: RectF::new(250.0, 250.0, 170.0, 60.0),
            tail: pt(230.0, 340.0),
            text: "Click this button".into(),
            size: 18.0,
        },
        Style {
            color: [20, 20, 40, 255],
            ..orange.clone()
        },
    );
    d.add(
        ShapeKind::Magnifier {
            source: RectF::new(40.0, 40.0, 40.0, 30.0),
            dest: RectF::new(500.0, 140.0, 110.0, 110.0),
            circular: true,
        },
        orange.clone(),
    );
    d.add(
        ShapeKind::Magnifier {
            source: RectF::new(250.0, 30.0, 40.0, 30.0),
            dest: RectF::new(480.0, 280.0, 130.0, 90.0),
            circular: false,
        },
        orange.clone(),
    );
    d.add(
        ShapeKind::Redact {
            rect: RectF::new(440.0, 250.0, 30.0, 20.0),
        },
        Style {
            color: [0, 0, 0, 255],
            ..orange.clone()
        },
    );
    d.add(
        ShapeKind::Spotlight {
            rect: RectF::new(20.0, 20.0, 600.0, 250.0),
            dim: 0.35,
        },
        orange,
    );
    let out = render(&base, &d, &f);
    out.save(
        std::path::Path::new(&out_path),
        crate::image::ImageFormat::Png,
        90,
    )
    .unwrap();
}
