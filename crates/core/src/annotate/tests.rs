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
            ctrl: None,
            double: false,
            head: ArrowHead::Notched,
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
        ShapeKind::HighlightBox { rect: r },
        ShapeKind::Text {
            pos: pt(5.0, 5.0),
            text: "hi\nthere".into(),
            size: 18.0,
            background: Some([1, 2, 3, 255]),
            bold: false,
            width: None,
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
        cursor_shape(pt(4.0, 6.0), arrow_cursor()).0,
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
            ctrl: None,
            double: false,
            head: ArrowHead::Notched,
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
            bold: false,
            width: None,
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
            ctrl: None,
            double: false,
            head: ArrowHead::Notched,
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
            bold: false,
            width: None,
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
        bold: false,
        width: None,
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
        ctrl: None,
        double: false,
        head: ArrowHead::Notched,
    };
    translate_shape(&mut k, -1.0, -1.0);
    assert_eq!(
        k,
        ShapeKind::Arrow {
            from: pt(0.0, 0.0),
            to: pt(1.0, 1.0),
            ctrl: None,
            double: false,
            head: ArrowHead::Notched,
        }
    );
    // A curved arrow's control point moves with it.
    let mut k = ShapeKind::Arrow {
        from: pt(1.0, 1.0),
        to: pt(9.0, 1.0),
        ctrl: Some(pt(5.0, 8.0)),
        double: true,
        head: ArrowHead::Open,
    };
    translate_shape(&mut k, 2.0, 3.0);
    assert!(matches!(k, ShapeKind::Arrow { ctrl: Some(c), .. } if c == pt(7.0, 11.0)));
}

// ----- additive fields: curved / double / head style arrows, bold text --------------------

#[test]
fn old_sidecar_json_loads_with_defaults() {
    // A doc written before arrows could curve and text could be bold.
    let json = r#"{
        "version": 1, "base_width": 100, "base_height": 80, "next_id": 3,
        "shapes": [
            {"id": 1, "kind": {"type": "arrow", "from": {"x": 1.0, "y": 2.0},
                "to": {"x": 50.0, "y": 60.0}},
             "style": {"color": [255, 0, 0, 255], "width": 4.0, "fill": null, "shadow": true}},
            {"id": 2, "kind": {"type": "text", "pos": {"x": 5.0, "y": 5.0}, "text": "Hi",
                "size": 20.0, "background": null},
             "style": {"color": [255, 0, 0, 255], "width": 4.0, "fill": null, "shadow": true}}
        ]
    }"#;
    let d = Doc::from_json(json).unwrap();
    assert_eq!(
        d.shapes[0].kind,
        ShapeKind::Arrow {
            from: pt(1.0, 2.0),
            to: pt(50.0, 60.0),
            ctrl: None,
            double: false,
            head: ArrowHead::Notched,
        }
    );
    assert!(matches!(
        d.shapes[1].kind,
        ShapeKind::Text { bold: false, .. }
    ));
    // Defaults are not written back, so straight arrows stay readable by older builds.
    let out = d.to_json().unwrap();
    assert!(!out.contains("ctrl") && !out.contains("double") && !out.contains("bold"));
    assert!(!out.contains("\"head\""));
}

#[test]
fn new_arrow_and_text_fields_round_trip() {
    let mut d = Doc::new(200, 200);
    d.add(
        ShapeKind::Arrow {
            from: pt(10.0, 10.0),
            to: pt(150.0, 40.0),
            ctrl: Some(pt(80.0, 120.0)),
            double: true,
            head: ArrowHead::Triangle,
        },
        style(RED, 4.0),
    );
    d.add(
        ShapeKind::Text {
            pos: pt(1.0, 1.0),
            text: "Bold".into(),
            size: 22.0,
            background: Some([0, 0, 0, 255]),
            bold: true,
            width: None,
        },
        style(RED, 4.0),
    );
    let back = Doc::from_json(&d.to_json().unwrap()).unwrap();
    assert_eq!(back, d);
}

#[test]
fn curve_helpers() {
    let (a, b) = (pt(0.0, 0.0), pt(100.0, 0.0));
    // The control point that passes through (50, 40) at t = 0.5.
    let c = ctrl_through(a, b, pt(50.0, 40.0));
    assert_eq!(c, pt(50.0, 80.0));
    assert_eq!(arrow_mid(a, b, Some(c)), pt(50.0, 40.0));
    assert_eq!(arrow_mid(a, b, None), pt(50.0, 0.0));
    let q = quad_points(a, c, b, 4);
    assert_eq!(q.len(), 5);
    assert_eq!(q[0], a);
    assert_eq!(q[4], b);
    assert_eq!(q[2], pt(50.0, 40.0));
    // Moving an end keeps the bend: doubling the chord doubles the bulge.
    let c2 = follow_ctrl(a, b, a, pt(200.0, 0.0), c);
    assert!((c2.x - 100.0).abs() < 1e-3 && (c2.y - 160.0).abs() < 1e-3);
    // Rotating the chord 90 degrees rotates the control point with it.
    let c3 = follow_ctrl(a, b, a, pt(0.0, 100.0), c);
    assert!((c3.x + 80.0).abs() < 1e-3 && (c3.y - 50.0).abs() < 1e-3);
}

#[test]
fn curved_arrow_head_follows_the_end_tangent() {
    // Curve bulging down: the end tangent (to - ctrl) points up and to the right.
    let parts = arrow_parts(
        pt(0.0, 0.0),
        pt(200.0, 0.0),
        Some(pt(100.0, 200.0)),
        4.0,
        false,
        ArrowHead::Notched,
    )
    .unwrap();
    let g = parts.heads[0];
    let (dx, dy) = (g.tip.x - g.notch.x, g.tip.y - g.notch.y);
    let l = (dx * dx + dy * dy).sqrt();
    // Tangent direction (100, -200) normalised.
    assert!((dx / l - 0.447).abs() < 0.01 && (dy / l + 0.894).abs() < 0.01);
    // The shaft ends at the notch, not at the tip.
    let end = *parts.shaft.last().unwrap();
    assert!(((end.x - g.notch.x).powi(2) + (end.y - g.notch.y).powi(2)).sqrt() < 1.0);
    // Double heads: one at each end, the shaft trimmed at both.
    let parts = arrow_parts(
        pt(0.0, 0.0),
        pt(200.0, 0.0),
        None,
        4.0,
        true,
        ArrowHead::Triangle,
    )
    .unwrap();
    assert_eq!(parts.heads.len(), 2);
    assert_eq!(parts.heads[1].tip, pt(0.0, 0.0));
    assert!(parts.shaft[0].x > 5.0 && parts.shaft.last().unwrap().x < 195.0);
}

#[test]
fn curved_arrow_hits_and_bounds_follow_the_curve() {
    let mut d = Doc::new(300, 300);
    let id = d.add(
        ShapeKind::Arrow {
            from: pt(20.0, 20.0),
            to: pt(220.0, 20.0),
            ctrl: Some(pt(120.0, 220.0)),
            double: false,
            head: ArrowHead::Notched,
        },
        style(RED, 4.0),
    );
    // The curve passes through (120, 120); the chord at y = 20 is empty.
    assert_eq!(d.hit_test(pt(120.0, 120.0), 2.0), Some(id));
    assert_eq!(d.hit_test(pt(120.0, 20.0), 2.0), None);
    let b = shape_bounds(d.get(id).unwrap(), &test_fonts());
    assert!(b.y + b.h >= 120.0 && b.y + b.h < 130.0, "{b:?}");
}

#[test]
fn head_styles_and_double_heads_render() {
    let f = test_fonts();
    for head in ArrowHead::ALL {
        let mut d = Doc::new(240, 120);
        d.add(
            ShapeKind::Arrow {
                from: pt(20.0, 60.0),
                to: pt(220.0, 60.0),
                ctrl: None,
                double: true,
                head,
            },
            style(RED, 4.0),
        );
        let out = render(&white(240, 120), &d, &f);
        // Both ends get a head wider than the 4 px shaft.
        for x in [34, 206] {
            let tall = (0..120).filter(|y| is_reddish(out.pixel(x, *y))).count();
            assert!(tall >= 8, "{head:?} at x={x}: {tall}");
        }
        let mid = (0..120).filter(|y| is_reddish(out.pixel(120, *y))).count();
        assert!(mid <= 6, "{head:?} shaft: {mid}");
    }
}

#[test]
fn bold_text_is_wider_and_dark_text_gets_a_light_halo() {
    let f = test_fonts();
    let text = |bold: bool, color: [u8; 4]| Shape {
        id: 1,
        kind: ShapeKind::Text {
            pos: pt(10.0, 10.0),
            text: "Wide Words".into(),
            size: 30.0,
            background: None,
            bold,
            width: None,
        },
        style: Style {
            shadow: true,
            ..style(color, 4.0)
        },
    };
    let r = shape_bounds(&text(false, RED), &f);
    let b = shape_bounds(&text(true, RED), &f);
    assert!(b.w > r.w, "{} vs {}", b.w, r.w);
    // Black text on black: the halo makes some pixels light.
    let mut d = Doc::new(300, 80);
    d.shapes.push(text(false, [0, 0, 0, 255]));
    let out = render(&Rgba::filled(300, 80, [0, 0, 0, 255]), &d, &f);
    let light = count_where(&out, PxRect::new(0, 0, 300, 80), |p| p[0] > 60);
    assert!(light > 50, "{light}");
}

#[test]
fn text_layout_matches_lines_and_wraps_callouts() {
    let f = test_fonts();
    let (size, bold, lines) = text_layout(
        &f,
        &ShapeKind::Text {
            pos: pt(5.0, 7.0),
            text: "ab\ncde".into(),
            size: 20.0,
            background: None,
            bold: true,
            width: None,
        },
    )
    .unwrap();
    assert_eq!((size, bold), (20.0, true));
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].range, 0..2);
    assert_eq!(lines[1].range, 3..6);
    assert_eq!(lines[1].origin, pt(5.0, 7.0 + 25.0));
    let kind = ShapeKind::Callout {
        rect: RectF::new(0.0, 0.0, 120.0, 200.0),
        tail: pt(0.0, 260.0),
        text: "one two three four five six seven".into(),
        size: 16.0,
    };
    let (_, _, lines) = text_layout(&f, &kind).unwrap();
    assert!(lines.len() > 1);
    let ShapeKind::Callout { text, .. } = &kind else {
        unreachable!()
    };
    // Lines cover the words in order; breaks consume exactly one space.
    let joined: Vec<&str> = lines.iter().map(|l| &text[l.range.clone()]).collect();
    assert_eq!(joined.join(" "), *text);
    // Every line fits inside the padded bubble.
    for l in &lines {
        assert!(f.line_width(&text[l.range.clone()], 16.0, false) <= 120.0 - 2.0 * 16.0 * 0.7);
        assert!(l.origin.x > 0.0);
    }
}

fn boxed_text(text: &str, width: Option<f32>, bold: bool) -> ShapeKind {
    ShapeKind::Text {
        pos: pt(10.0, 12.0),
        text: text.into(),
        size: 20.0,
        background: None,
        bold,
        width,
    }
}

#[test]
fn wrap_with_breaks_at_spaces_and_paragraphs() {
    // 10 px per char: "aa bb" = 50 px.
    let w = |s: &str| s.chars().count() as f32 * 10.0;
    let t = "aa bb cc\ndd";
    let r = wrap_with(t, 50.0, w);
    let lines: Vec<&str> = r.iter().map(|r| &t[r.clone()]).collect();
    assert_eq!(lines, ["aa bb", "cc", "dd"]);
    // A word wider than the box keeps its own line; empty paragraphs stay.
    let t = "abcdefgh x\n\ny";
    let lines: Vec<&str> = wrap_with(t, 30.0, w).into_iter().map(|r| &t[r]).collect();
    assert_eq!(lines, ["abcdefgh", "x", "", "y"]);
    // No width: only '\n' breaks.
    let t = "aa bb cc\ndd";
    assert_eq!(text_line_ranges(t, None, w), vec![0..8, 9..11]);
    assert_eq!(shown_text(t, Some(50.0), w), "aa bb\ncc\ndd");
}

#[test]
fn fixed_width_text_wraps_in_layout_bounds_and_render() {
    let f = test_fonts();
    let text = "one two three four five six seven eight";
    let auto = boxed_text(text, None, false);
    let boxed = boxed_text(text, Some(120.0), false);
    let (_, _, auto_lines) = text_layout(&f, &auto).unwrap();
    let (_, _, lines) = text_layout(&f, &boxed).unwrap();
    assert_eq!(auto_lines.len(), 1);
    assert!(lines.len() >= 3, "{}", lines.len());
    // Lines cover the words in order, each fits the box, and they stack by line height.
    let joined: Vec<&str> = lines.iter().map(|l| &text[l.range.clone()]).collect();
    assert_eq!(joined.join(" "), text);
    for (i, l) in lines.iter().enumerate() {
        assert!(f.line_width(&text[l.range.clone()], 20.0, false) <= 120.0);
        assert_eq!(
            l.origin,
            pt(10.0, 12.0 + i as f32 * 20.0 * crate::fonts::LINE_HEIGHT)
        );
    }
    // Bounds: the box width (not the text's), one line height per wrapped line.
    let s = |kind| Shape {
        id: 1,
        kind,
        style: style(RED, 2.0),
    };
    let b = shape_bounds(&s(boxed.clone()), &f);
    assert_eq!((b.x, b.y, b.w), (10.0, 12.0, 120.0));
    assert!((b.h - lines.len() as f32 * 20.0 * crate::fonts::LINE_HEIGHT).abs() < 0.01);
    assert!(shape_hit(&s(boxed.clone()), pt(125.0, b.y + 5.0), 2.0));
    // The renderer draws exactly the wrapped lines: same pixels as the text pre-broken.
    let pre = boxed_text(&joined.join("\n"), None, false);
    let mut d1 = Doc::new(200, 200);
    d1.shapes.push(s(boxed));
    let mut d2 = Doc::new(200, 200);
    d2.shapes.push(s(pre));
    let base = white(200, 200);
    assert_eq!(render(&base, &d1, &f).data, render(&base, &d2, &f).data);
    // Bold measures wider, so it may wrap sooner, never later.
    let (_, _, bold_lines) = text_layout(&f, &boxed_text(text, Some(120.0), true)).unwrap();
    assert!(bold_lines.len() >= lines.len());
}

#[test]
fn text_width_json_is_optional() {
    let mut d = Doc::new(100, 80);
    d.add(boxed_text("hi there", Some(64.0), false), style(RED, 2.0));
    d.add(boxed_text("auto", None, false), style(RED, 2.0));
    let json = d.to_json().unwrap();
    assert_eq!(
        json.matches("\"width\": 64").count() + json.matches("\"width\":64").count(),
        1,
        "{json}"
    );
    assert_eq!(Doc::from_json(&json).unwrap(), d);
    // A text shape from before fixed boxes loads as auto width.
    let old = r#"{
        "version": 1, "base_width": 100, "base_height": 80, "next_id": 2,
        "shapes": [
            {"id": 1, "kind": {"type": "text", "pos": {"x": 1.0, "y": 2.0}, "text": "old",
                "size": 18.0, "background": null},
             "style": {"color": [255, 0, 0, 255], "width": 2.0, "fill": null, "shadow": false}}
        ]
    }"#;
    let back = Doc::from_json(old).unwrap();
    assert!(matches!(
        back.shapes[0].kind,
        ShapeKind::Text {
            width: None,
            bold: false,
            ..
        }
    ));
}

#[test]
fn region_render_matches_full_render() {
    let f = test_fonts();
    let mut base = Rgba::new(320, 240);
    for (i, p) in base.data.chunks_exact_mut(4).enumerate() {
        let (x, y) = ((i % 320) as u8, (i / 320) as u8);
        p.copy_from_slice(&[x, y, x ^ y, 255]);
    }
    let mut d = Doc::new(320, 240);
    d.add(
        ShapeKind::Blur {
            rect: RectF::new(20.0, 20.0, 80.0, 50.0),
            radius: 6.0,
        },
        style(RED, 4.0),
    );
    d.add(
        ShapeKind::Arrow {
            from: pt(30.0, 200.0),
            to: pt(290.0, 40.0),
            ctrl: Some(pt(250.0, 230.0)),
            double: true,
            head: ArrowHead::Notched,
        },
        style(RED, 5.0),
    );
    d.add(
        ShapeKind::Text {
            pos: pt(120.0, 100.0),
            text: "Region\nrender".into(),
            size: 24.0,
            background: None,
            bold: true,
            width: None,
        },
        style([30, 200, 255, 255], 4.0),
    );
    d.add(
        ShapeKind::Magnifier {
            source: RectF::new(30.0, 30.0, 40.0, 30.0),
            dest: RectF::new(200.0, 150.0, 90.0, 70.0),
            circular: true,
        },
        style(RED, 4.0),
    );
    d.add(
        ShapeKind::Spotlight {
            rect: RectF::new(100.0, 80.0, 120.0, 90.0),
            dim: 0.5,
        },
        style(RED, 4.0),
    );
    d.add(
        ShapeKind::Redact {
            rect: RectF::new(250.0, 10.0, 40.0, 20.0),
        },
        style([0, 0, 0, 255], 4.0),
    );
    let full = render(&base, &d, &f);
    let work = bake_destructive(&base, &d);
    for r in [
        PxRect::new(0, 0, 320, 240),
        PxRect::new(110, 90, 60, 50),
        PxRect::new(190, 140, 120, 90),
        PxRect::new(-20, -20, 70, 70),
        PxRect::new(300, 200, 100, 100),
    ] {
        let part = render_region(&work, &d, &f, r);
        let clip = r.intersect(&base.bounds()).unwrap();
        let want = full.crop(clip);
        assert_eq!((part.width, part.height), (want.width, want.height));
        let worst = part
            .data
            .iter()
            .zip(&want.data)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(worst <= 2, "{r:?}: max channel difference {worst}");
    }
}

#[test]
fn shape_patch_matches_the_full_render() {
    let f = test_fonts();
    let shape = Shape {
        id: 1,
        kind: ShapeKind::Text {
            pos: pt(40.3, 30.0),
            text: "Patch".into(),
            size: 28.0,
            background: Some([20, 20, 20, 255]),
            bold: false,
            width: None,
        },
        style: style([255, 255, 255, 255], 4.0),
    };
    let (r, patch) = render_shape_patch(&shape, &f).unwrap();
    assert!(r.x <= 30 && r.y <= 25 && r.right() >= 120);
    // Opaque pill pixels in the patch equal the full render over white.
    let mut d = Doc::new(300, 120);
    d.shapes.push(shape);
    let full = render(&white(300, 120), &d, &f);
    let mut same = 0;
    for y in 0..patch.height {
        for x in 0..patch.width {
            let p = patch.pixel(x, y);
            if p[3] == 255 {
                let q = full.pixel(x + r.x as u32, y + r.y as u32);
                assert!(p.iter().zip(q.iter()).all(|(a, b)| a.abs_diff(*b) <= 1));
                same += 1;
            }
        }
    }
    assert!(same > 200);
    // Destructive shapes have no patch.
    let blur = Shape {
        id: 2,
        kind: ShapeKind::Blur {
            rect: RectF::new(0.0, 0.0, 10.0, 10.0),
            radius: 4.0,
        },
        style: style(RED, 4.0),
    };
    assert!(render_shape_patch(&blur, &f).is_none());
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
            ctrl: None,
            double: false,
            head: ArrowHead::Notched,
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
                ctrl: None,
                double: false,
                head: ArrowHead::Notched,
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
            ctrl: None,
            double: false,
            head: ArrowHead::Notched,
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
            bold: false,
            width: None,
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
fn render_highlight_box_fills_exactly_its_rect_like_a_stroke() {
    let f = test_fonts();
    let mut base = white(100, 60);
    for x in 20..80 {
        for y in 28..32 {
            base.set_pixel(x, y, [0, 0, 0, 255]);
        }
    }
    let yellow = style([255, 235, 0, 0x99], 16.0);
    let mut d = Doc::new(100, 60);
    d.add(
        ShapeKind::HighlightBox {
            rect: RectF::new(10.0, 20.0, 80.0, 20.0),
        },
        yellow.clone(),
    );
    let out = render(&base, &d, &f);
    // Same tint as a freehand stroke of the same style, text still readable, no outline.
    let mut stroke = Doc::new(100, 60);
    stroke.add(
        ShapeKind::Highlighter {
            points: vec![pt(10.0, 30.0), pt(90.0, 30.0)],
        },
        yellow,
    );
    let ref_out = render(&base, &stroke, &f);
    assert_eq!(out.pixel(50, 24), ref_out.pixel(50, 24));
    assert_ne!(out.pixel(50, 24), WHITE);
    assert!(out.pixel(50, 29)[0] < 150, "text readable");
    assert_eq!(
        out.pixel(10, 20),
        out.pixel(50, 24),
        "flat fill to the corner"
    );
    assert_eq!(out.pixel(89, 39), out.pixel(50, 24));
    assert_eq!(out.pixel(5, 30), WHITE);
    assert_eq!(out.pixel(50, 45), WHITE);
    assert_eq!(out.pixel(50, 15), WHITE);
    // The region render (editor) and the full render agree.
    let work = bake_destructive(&base, &d);
    let region = render_region(&work, &d, &f, PxRect::new(0, 0, 100, 60));
    assert_eq!(region.data, out.data);
}

#[test]
fn highlight_box_hits_bounds_and_moves() {
    let f = test_fonts();
    let mut d = Doc::new(200, 200);
    let id = d.add(
        ShapeKind::HighlightBox {
            rect: RectF::new(20.0, 30.0, 100.0, 20.0),
        },
        style(RED, 16.0),
    );
    assert_eq!(d.hit_test(pt(70.0, 40.0), 0.0), Some(id));
    assert_eq!(d.hit_test(pt(70.0, 55.0), 0.0), None);
    assert_eq!(
        shape_bounds(d.get(id).unwrap(), &f),
        RectF::new(20.0, 30.0, 100.0, 20.0)
    );
    translate_shape(&mut d.get_mut(id).unwrap().kind, 5.0, -10.0);
    assert_eq!(
        d.get(id).unwrap().kind,
        ShapeKind::HighlightBox {
            rect: RectF::new(25.0, 20.0, 100.0, 20.0)
        }
    );
}

#[test]
fn highlighter_box_rect_covers_the_painted_stroke() {
    let kind = ShapeKind::Highlighter {
        points: vec![pt(10.0, 30.0), pt(50.0, 34.0), pt(90.0, 28.0)],
    };
    assert_eq!(
        highlighter_box_rect(&kind, 16.0),
        Some(RectF::new(2.0, 20.0, 96.0, 22.0))
    );
    // Same as the stroke's own bounds, so the box covers what the stroke painted.
    let f = test_fonts();
    let shape = Shape {
        id: 1,
        kind: kind.clone(),
        style: style(RED, 16.0),
    };
    assert_eq!(
        highlighter_box_rect(&kind, 16.0),
        Some(shape_bounds(&shape, &f))
    );
    assert_eq!(
        highlighter_box_rect(&ShapeKind::Highlighter { points: vec![] }, 16.0),
        None
    );
    let pen = ShapeKind::Pen {
        points: vec![pt(0.0, 0.0), pt(5.0, 5.0)],
    };
    assert_eq!(highlighter_box_rect(&pen, 16.0), None);
    // A single dot becomes a marker-sized square; non-finite points are ignored.
    let dot = ShapeKind::Highlighter {
        points: vec![pt(10.0, 10.0), pt(f32::NAN, 3.0)],
    };
    assert_eq!(
        highlighter_box_rect(&dot, 8.0),
        Some(RectF::new(6.0, 6.0, 8.0, 8.0))
    );
}

#[test]
fn highlight_box_json_and_older_sidecars() {
    let mut d = Doc::new(100, 80);
    d.add(
        ShapeKind::HighlightBox {
            rect: RectF::new(1.0, 2.0, 30.0, 10.0),
        },
        style([255, 235, 0, 0x99], 16.0),
    );
    let json = d.to_json().unwrap();
    assert!(json.contains("\"highlight_box\""), "{json}");
    assert_eq!(Doc::from_json(&json).unwrap(), d);
    // A sidecar from before box highlights (freehand only) still loads unchanged.
    let old = r#"{
        "version": 1, "base_width": 100, "base_height": 80, "next_id": 2,
        "shapes": [
            {"id": 1, "kind": {"type": "highlighter",
                "points": [{"x": 1.0, "y": 2.0}, {"x": 50.0, "y": 2.0}]},
             "style": {"color": [255, 235, 0, 153], "width": 16.0, "fill": null, "shadow": false}}
        ]
    }"#;
    let back = Doc::from_json(old).unwrap();
    assert_eq!(
        back.shapes[0].kind,
        ShapeKind::Highlighter {
            points: vec![pt(1.0, 2.0), pt(50.0, 2.0)]
        }
    );
}

#[test]
fn highlight_box_survives_save_and_reopen() {
    let dir = tmpdir("hlbox");
    let flat = dir.join("Shot.png");
    let base = white(64, 48);
    let mut d = Doc::new(64, 48);
    d.add(
        ShapeKind::HighlightBox {
            rect: RectF::new(4.0, 6.0, 40.0, 12.0),
        },
        style([255, 235, 0, 0x99], 16.0),
    );
    save_editable(&flat, &base, &d).unwrap();
    let (b2, d2) = load_editable(&flat).unwrap().unwrap();
    assert_eq!(d2, d);
    assert_eq!(b2.data, base.data);
    let _ = std::fs::remove_dir_all(&dir);
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
            bold: false,
            width: None,
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
    let mk = |shadow: bool, color: [u8; 4]| {
        let mut d = Doc::new(160, 120);
        d.add(
            ShapeKind::Text {
                pos: pt(10.0, 10.0),
                text: "AB\nCD".into(),
                size: 30.0,
                background: None,
                bold: false,
                width: None,
            },
            Style {
                shadow,
                ..style(color, 4.0)
            },
        );
        render(&base, &d, &f)
    };
    let plain = mk(false, [0, 0, 0, 255]);
    // Second line is drawn lower: ink exists in rows of line 2 (y ~ 47..85).
    assert!(count_where(&plain, PxRect::new(0, 50, 160, 40), |p| p[0] < 100) > 30);
    // Shadow adds dark pixels under light text that the plain version lacks.
    let light = [255, 200, 0, 255];
    let (plain, shadowed) = (mk(false, light), mk(true, light));
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
            bold: false,
            width: None,
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
            bold: false,
            width: None,
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
    // The stored base has the Redact baked in: the marked pixel under it is gone.
    assert_eq!(b2, bake_destructive(&base, &doc));
    assert_ne!(b2.pixel(3, 4), [9, 8, 7, 255]);
    assert_eq!(b2.pixel(20, 15), base.pixel(20, 15));
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
            ctrl: None,
            double: false,
            head: ArrowHead::Notched,
        },
        orange.clone(),
    );
    d.add(
        ShapeKind::Arrow {
            from: pt(40.0, 160.0),
            to: pt(220.0, 160.0),
            ctrl: None,
            double: false,
            head: ArrowHead::Notched,
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
            bold: false,
            width: None,
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
            bold: false,
            width: None,
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

#[test]
fn sidecar_base_has_redactions_baked_in() {
    let dir = tmpdir("sidecar-baked");
    let flat = dir.join("Secret.png");
    // A "secret" in distinctive pixels, plus a clean area outside every redaction.
    let mut base = white(60, 40);
    for y in 10..20 {
        for x in 5..55 {
            base.set_pixel(x, y, [200, 30, 30, 255]);
        }
    }
    let mut doc = Doc::new(60, 40);
    doc.add(
        ShapeKind::Redact {
            rect: RectF::new(5.0, 10.0, 20.0, 10.0),
        },
        style([0, 0, 0, 255], 1.0),
    );
    doc.add(
        ShapeKind::Pixelate {
            rect: RectF::new(25.0, 5.0, 30.0, 20.0),
            block: 10,
        },
        style([0, 0, 0, 255], 1.0),
    );
    doc.add(
        ShapeKind::Line {
            from: pt(0.0, 35.0),
            to: pt(10.0, 35.0),
        },
        Style::default(),
    );
    save_editable(&flat, &base, &doc).unwrap();
    let (stored, d2) = load_editable(&flat).unwrap().unwrap();
    // The shapes stay editable.
    assert_eq!(d2, doc);
    // Inside the Redact box: no source pixel survives.
    for y in 10..20 {
        for x in 5..25 {
            assert_eq!(stored.pixel(x, y), [0, 0, 0, 255], "({x},{y})");
        }
    }
    // Inside the Pixelate rect the original crisp edge (row 9 white, row 10 red) is gone.
    assert_ne!(stored.pixel(30, 9), base.pixel(30, 9));
    // Outside every destructive shape the base is untouched (the Line is not baked).
    assert_eq!(stored.pixel(2, 35), base.pixel(2, 35));
    assert_eq!(stored.pixel(58, 38), base.pixel(58, 38));
    let _ = std::fs::remove_dir_all(&dir);
}

fn fonts_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()))
        .join("Fonts")
}

/// Mixed-script text renders real glyphs (fallback faces), not missing-glyph boxes: the
/// same string drawn with fallbacks differs from the primary-only render, every glyph leaves
/// ink, and the ink stays inside the measured line width (caret layout agrees).
#[test]
fn cjk_and_emoji_text_renders_with_fallback_fonts() {
    if !fonts_dir().join("msyh.ttc").exists() || !fonts_dir().join("seguiemj.ttf").exists() {
        eprintln!("skipped: CJK / emoji fonts not installed");
        return;
    }
    let f = test_fonts();
    let bare = Fonts::from_bytes(f.regular_bytes.to_vec(), f.bold_bytes.to_vec()).unwrap();
    let text = "\u{4e2d}\u{6587} \u{65e5}\u{672c}\u{8a9e} \u{d55c}\u{ad6d}\u{c5b4} \u{1f680}";
    let mut d = Doc::new(400, 60);
    d.add(
        ShapeKind::Text {
            pos: pt(4.0, 4.0),
            text: text.into(),
            size: 28.0,
            background: None,
            bold: false,
            width: None,
        },
        style([0, 0, 0, 255], 2.0),
    );
    let with = render(&white(400, 60), &d, &f);
    let without = render(&white(400, 60), &d, &bare);
    let ink = |img: &Rgba| img.data.chunks_exact(4).filter(|p| p[0] < 128).count();
    assert!(ink(&with) > 400, "{}", ink(&with));
    assert_ne!(with.data, without.data);
    let w = f.line_width(text, 28.0, false);
    let right = (0..400u32)
        .rev()
        .find(|&x| (0..60u32).any(|y| with.pixel(x, y)[0] < 128))
        .unwrap();
    assert!((right as f32) <= 4.0 + w + 2.0, "{right} vs {w}");
}

/// Visual check of mixed scripts and Beautify:
/// `STINKSHOT_EDITOR_SHOWCASE=<dir> cargo test -p stinkshot-core editor_showcase -- --ignored`.
#[test]
#[ignore]
fn editor_showcase() {
    let Some(dir) = std::env::var_os("STINKSHOT_EDITOR_SHOWCASE") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = test_fonts();
    let mut d = Doc::new(520, 200);
    let lines: [(&str, Option<[u8; 4]>); 3] = [
        (
            "English \u{4e2d}\u{6587} \u{65e5}\u{672c}\u{8a9e}\u{306e}\u{6587}",
            None,
        ),
        (
            "\u{d55c}\u{ad6d}\u{c5b4} \u{c548}\u{b155}\u{d558}\u{c138}\u{c694}",
            Some([255, 138, 61, 255]),
        ),
        (
            "Ship it \u{1f680}\u{1f600}\u{1f44d}\u{1f3fd} \u{2764}\u{fe0f} \u{2714}",
            None,
        ),
    ];
    for (i, (s, bg)) in lines.into_iter().enumerate() {
        d.add(
            ShapeKind::Text {
                pos: pt(16.0, 16.0 + i as f32 * 60.0),
                text: s.into(),
                size: 30.0,
                background: bg,
                bold: i == 0,
                width: None,
            },
            style([20, 20, 20, 255], 2.0),
        );
    }
    let img = render(&white(520, 200), &d, &f);
    let png = crate::image::ImageFormat::Png;
    img.save(&dir.join("cjk.png"), png, 90).unwrap();
    use crate::beautify::{Aspect, BackgroundKind, Beautify};
    let variants = [
        ("beautify_gradient.png", Beautify::default()),
        (
            "beautify_blurred_169.png",
            Beautify {
                background: BackgroundKind::Blurred,
                aspect: Aspect::Wide,
                ..Default::default()
            },
        ),
        (
            "beautify_solid_square.png",
            Beautify {
                background: BackgroundKind::Solid,
                color: "#1f1f23".into(),
                aspect: Aspect::Square,
                padding: 40,
                ..Default::default()
            },
        ),
    ];
    for (name, b) in variants {
        crate::beautify::apply(&img, &b)
            .save(&dir.join(name), png, 90)
            .unwrap();
    }
}

// ----- cursor object ---------------------------------------------------------------------------

/// A 4x6 test pointer: opaque black left column, translucent red elsewhere in the top half,
/// transparent bottom half.
fn arrow_cursor() -> CursorImage {
    let mut img = Rgba::new(4, 6);
    for y in 0..3 {
        img.set_pixel(0, y, [0, 0, 0, 255]);
        for x in 1..4 {
            img.set_pixel(x, y, [255, 0, 0, 128]);
        }
    }
    CursorImage::new(img)
}

#[test]
fn cursor_shape_is_drawn_1_to_1_and_ignores_style() {
    let (kind, st) = cursor_shape(pt(10.0, 20.0), arrow_cursor());
    assert!(
        matches!(kind, ShapeKind::Cursor { rect, .. } if rect == RectF::new(10.0, 20.0, 4.0, 6.0))
    );
    assert!(!st.shadow);
    let mut d = Doc::new(40, 40);
    let id = d.add(kind, st);
    let fonts = test_fonts();
    let s = d.get(id).unwrap();
    assert_eq!(shape_bounds(s, &fonts), RectF::new(10.0, 20.0, 4.0, 6.0));
    assert_eq!(d.hit_test(pt(11.0, 21.0), 0.0), Some(id));
    assert_eq!(d.hit_test(pt(30.0, 30.0), 0.0), None);
}

#[test]
fn cursor_translates_and_round_trips_compactly() {
    let mut d = Doc::new(40, 40);
    let (k, st) = cursor_shape(pt(1.0, 2.0), arrow_cursor());
    let id = d.add(k, st);
    translate_shape(&mut d.get_mut(id).unwrap().kind, 5.0, -1.0);
    assert!(
        matches!(&d.get(id).unwrap().kind, ShapeKind::Cursor { rect, .. } if rect.x == 6.0 && rect.y == 1.0)
    );
    let json = d.to_json().unwrap();
    assert!(json.contains("\"type\": \"cursor\""), "{json}");
    // The bitmap is a base64 PNG string, not a byte array.
    assert!(json.contains("\"image\": \"iVBOR"), "{json}");
    let back = Doc::from_json(&json).unwrap();
    assert_eq!(back, d);
}

#[test]
fn cursor_with_bad_image_fails_to_parse() {
    let json = r#"{"version":1,"base_width":10,"base_height":10,"next_id":2,"shapes":[{"id":1,
        "kind":{"type":"cursor","rect":{"x":0,"y":0,"w":4,"h":4},"image":"not base64!"},
        "style":{"color":[0,0,0,255],"width":0,"fill":null,"shadow":false}}]}"#;
    assert!(Doc::from_json(json).is_err());
}

#[test]
fn cursor_composites_over_the_base() {
    let base = white(20, 20);
    let mut d = Doc::new(20, 20);
    let (k, st) = cursor_shape(pt(5.0, 5.0), arrow_cursor());
    d.add(k, st);
    let fonts = test_fonts();
    let out = render(&base, &d, &fonts);
    // Opaque pixel copied exactly; translucent red blended over white; transparent untouched.
    assert_eq!(out.pixel(5, 5), [0, 0, 0, 255]);
    let p = out.pixel(6, 5);
    assert!(
        p[0] == 255 && (120..=135).contains(&p[1]) && p[1] == p[2] && p[3] == 255,
        "{p:?}"
    );
    assert_eq!(out.pixel(6, 9), WHITE);
    assert_eq!(out.pixel(4, 5), WHITE);
    // Only the pointer's top half changed.
    assert_eq!(count_changed(&base, &out, base.bounds()), 12);
    // The editor's partial render agrees with the full one.
    let work = bake_destructive(&base, &d);
    let part = render_region(&work, &d, &fonts, PxRect::new(4, 4, 6, 6));
    assert_eq!(part.pixel(1, 1), out.pixel(5, 5));
    assert_eq!(part.pixel(2, 1), out.pixel(6, 5));
}

#[test]
fn resized_cursor_scales_into_its_rect() {
    let base = white(30, 30);
    let mut d = Doc::new(30, 30);
    let (mut k, st) = cursor_shape(pt(0.0, 0.0), arrow_cursor());
    if let ShapeKind::Cursor { rect, .. } = &mut k {
        *rect = RectF::new(2.0, 2.0, 8.0, 12.0);
    }
    d.add(k, st);
    let out = render(&base, &d, &test_fonts());
    // The black column now spans x 2..4, the transparent half y 8..14.
    assert!(out.pixel(2, 3)[0] < 40);
    assert_eq!(out.pixel(5, 12), WHITE);
    assert_eq!(out.pixel(12, 3), WHITE);
}

#[test]
fn cursor_patch_renders_alone() {
    let (k, st) = cursor_shape(pt(3.0, 3.0), arrow_cursor());
    let s = Shape {
        id: 1,
        kind: k,
        style: st,
    };
    let (r, img) = render_shape_patch(&s, &test_fonts()).unwrap();
    let p = img.pixel((3 - r.x) as u32, (3 - r.y) as u32);
    assert_eq!(p, [0, 0, 0, 255]);
}
