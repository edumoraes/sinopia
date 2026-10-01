//! What the hands put on screen (pure): the laser and its trail, a ring
//! on every pinch and a line between two, the chevron of a turned slide,
//! and the presenter's camera in the corner of a show — the board's own
//! prims, drawn over everything at the screen's rate.

use super::gesture::{FLASH_S, Gestures, Pt, TRAIL_S, Turn};
use crate::doc::Card;
use crate::scene::{KIND_BLOB, Prim, Rgba, ScreenRect, View, parse_color, with_alpha};

/// The marks of a frame: the laser's trail, to be laid down as one union
/// so its spans do not bead where they meet, and the rest, over it.
pub struct Marks {
    pub trail: Vec<Prim>,
    pub over: Vec<Prim>,
}

/// Where a point of the camera's frame lands on the window: the frame
/// covers it, cut at the long side, the way the presenter sees it. What
/// the hands point at and what they zoom about are both read through it.
pub fn cover(size: (u32, u32), view: &View) -> impl Fn(Pt) -> (f32, f32) + use<> {
    let (cw, ch) = (f64::from(size.0.max(1)), f64::from(size.1.max(1)));
    let (vw, vh) = (f64::from(view.viewport.w), f64::from(view.viewport.h));
    let k = (vw / cw).max(vh / ch);
    let (ox, oy) = ((vw - cw * k) / 2.0, (vh - ch * k) / 2.0);
    move |p: Pt| ((ox + p[0] * cw * k) as f32, (oy + p[1] * ch * k) as f32)
}

fn square(x: f32, y: f32, half: f32) -> ScreenRect {
    ScreenRect {
        x: x - half,
        y: y - half,
        w: 2.0 * half,
        h: 2.0 * half,
    }
}

/// A disc fading from the middle out to `reach` px.
fn glow(x: f32, y: f32, reach: f32, color: Rgba) -> Prim {
    Prim {
        falloff: 1.6,
        ..Prim::soft(square(x, y, reach / 2.0), reach / 2.0, reach, color)
    }
}

pub fn marks(g: &Gestures, view: &View, now: f64) -> Marks {
    let at = cover(g.size, view);
    let s = view.scale as f32;
    let red = parse_color("#ff140d");
    let hot = parse_color("#ff7366");
    let gold = parse_color("#ffc700");
    let white: Rgba = [1.0, 1.0, 1.0, 1.0];

    // The trail: each span as faint and as thin as it is old, a wide soft
    // pass and a narrow bright one; strokes that are not one another's
    // are not joined.
    let mut trail = Vec::new();
    for pair in g.laser.trail.windows(2) {
        let ((_, a, sa), (t, b, sb)) = (pair[0], pair[1]);
        let life = (1.0 - (now - t) / TRAIL_S) as f32;
        if sa != sb || life <= 0.0 {
            continue;
        }
        let fade = life.powf(1.6);
        let thin = 0.35 + 0.65 * life;
        let (p, q) = (at(a), at(b));
        trail.push(Prim::soft_segment(p, q, 7.0 * s * thin, 6.0 * s, with_alpha(red, 0.22 * fade)));
        trail.push(Prim::soft_segment(p, q, 2.0 * s * thin, 1.0 * s, with_alpha(hot, 0.9 * fade)));
    }

    let mut over = Vec::new();
    // The tip: a wide red halo breathing a little, a hot rim, a white core.
    if let Some(tip) = g.laser.tip
        && g.laser.glow > 0.0
    {
        let (x, y) = at(tip);
        let lit = g.laser.glow as f32;
        let pulse = 1.0 + 0.08 * (now * 14.0).sin() as f32;
        over.push(glow(x, y, 38.0 * s * pulse, with_alpha(red, 0.55 * lit)));
        over.push(glow(x, y, 11.0 * s, with_alpha(hot, lit)));
        over.push(glow(x, y, 5.0 * s, with_alpha(white, lit)));
    }

    // A pinch: a gold ring with a wash in it; two, a line between.
    if let [a, b] = g.pinches[..] {
        over.push(Prim::segment(at(a), at(b), 1.0 * s, with_alpha(gold, 0.45)));
    }
    for p in &g.pinches {
        let (x, y) = at(*p);
        over.push(Prim::circle(x, y, 20.0 * s, with_alpha(gold, 0.27)));
        over.push(Prim::circle(x, y, 20.0 * s, with_alpha(gold, 0.9)).ring(3.0 * s));
    }

    // A slide turned: a chevron on the side it went to — › for the next,
    // ‹ for the one before — drifting out as it fades.
    if let Some((way, at_t)) = g.flash {
        let life = (1.0 - (now - at_t) / FLASH_S).max(0.0) as f32;
        let side = if way == Turn::Next { 1.0 } else { -1.0 };
        let (vw, vh) = (view.viewport.w as f32, view.viewport.h as f32);
        let x = vw / 2.0 + side * (vw * 0.36 + 40.0 * s * (1.0 - life));
        let y = vh / 2.0;
        let size = 48.0 * s;
        over.push(Prim::circle(x, y, 90.0 * s, [0.0, 0.0, 0.0, 0.35 * life]));
        let ink = with_alpha(white, 0.9 * life);
        let (back, tip) = (x - side * size / 2.0, x + side * size / 2.0);
        over.push(Prim::segment((back, y - size), (tip, y), 7.0 * s, ink));
        over.push(Prim::segment((tip, y), (back, y + size), 7.0 * s, ink));
    }
    Marks { trail, over }
}

/// How fast a blob card's outline moves on: its phase, a second.
const BLOB_TURN: f64 = 0.6;

/// The presenter's camera in the bottom right corner of the window, in
/// the `shape` the board asks for, over a soft shadow and with a hairline
/// round it, faded in by `alpha`. `size` is the picture's: the rounded
/// card keeps its shape, and the round, the square and the blob are a
/// square as tall as that card, showing the picture's middle square. A
/// blob's outline moves on with `now`, in seconds.
pub fn card(slot: u32, size: (u32, u32), alpha: f32, view: &View, shape: Card, now: f64) -> Vec<Prim> {
    let s = view.scale as f32;
    let (vw, vh) = (view.viewport.w as f32, view.viewport.h as f32);
    let (cw, ch) = (size.0.max(1) as f32, size.1.max(1) as f32);
    let wide = (vw * 0.22).min(460.0 * s);
    let tall = wide * ch / cw;
    let (w, h, uv) = match shape {
        Card::Rounded => (wide, tall, [0.0, 0.0, 1.0, 1.0]),
        Card::Round | Card::Square | Card::Blob => (tall, tall, middle_square(cw, ch)),
    };
    let r = ScreenRect {
        x: vw - w - 28.0 * s,
        y: vh - h - 28.0 * s,
        w,
        h,
    };
    let shadow = ScreenRect { y: r.y + 6.0 * s, ..r };
    let (shade, rim, edge) = ([0.0, 0.0, 0.0, 0.35 * alpha], [1.0, 1.0, 1.0, 0.55 * alpha], 2.0 * s);
    let round = match shape {
        Card::Blob => {
            let phase = (now * BLOB_TURN) as f32;
            return vec![
                Prim {
                    feather: 16.0 * s,
                    ..blob(shadow, phase, shade)
                },
                Prim {
                    uv,
                    slot,
                    paper: [phase, 1.0, 0.0, 0.0],
                    ..blob(r, phase, [1.0, 1.0, 1.0, alpha])
                },
                blob(r, phase, rim).ring(edge),
            ];
        }
        Card::Round => w / 2.0,
        Card::Square => 6.0 * s,
        Card::Rounded => 18.0 * s,
    };
    vec![
        Prim::soft(shadow, round, 16.0 * s, shade),
        Prim {
            radius: round,
            uv,
            color: [1.0, 1.0, 1.0, alpha],
            ..Prim::image(r, r.center(), 0.0, slot)
        },
        Prim::rounded(r, round, rim).ring(edge),
    ]
}

/// The middle square of a picture `cw` by `ch` px, as the slice of it to
/// map: its long side cut evenly at both ends.
fn middle_square(cw: f32, ch: f32) -> [f32; 4] {
    if cw >= ch {
        let cut = (1.0 - ch / cw) / 2.0;
        [cut, 0.0, 1.0 - cut, 1.0]
    } else {
        let cut = (1.0 - cw / ch) / 2.0;
        [0.0, cut, 1.0, 1.0 - cut]
    }
}

/// The blob inscribed in box `r` at `phase`, in flat `color` — and, given
/// a slice and a slot and `paper[1]` set, the picture cut to it.
fn blob(r: ScreenRect, phase: f32, color: Rgba) -> Prim {
    Prim {
        kind: KIND_BLOB,
        paper: [phase, 0.0, 0.0, 0.0],
        ..Prim::rect(r, color)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Camera;
    use crate::scene::Viewport;

    fn view() -> View {
        View {
            camera: Camera {
                x: 0.0,
                y: 0.0,
                zoom: 1.0,
            },
            viewport: Viewport { w: 1920, h: 1080 },
            scale: 1.0,
        }
    }

    #[test]
    fn the_camera_covers_the_window_cut_at_the_long_side() {
        // 640 by 480 over 1920 by 1080: scaled by 3, 1440 tall, 180 cut
        // off the top and the bottom each.
        let at = cover((640, 480), &view());
        assert_eq!(at([0.5, 0.5]), (960.0, 540.0));
        assert_eq!(at([0.0, 0.0]), (0.0, -180.0));
        assert_eq!(at([1.0, 1.0]), (1920.0, 1260.0));
    }

    #[test]
    fn nothing_is_drawn_for_hands_doing_nothing() {
        let m = marks(&Gestures::default(), &view(), 0.0);
        assert!(m.trail.is_empty() && m.over.is_empty());
    }

    #[test]
    fn the_card_stands_in_the_bottom_right_corner_keeping_the_pictures_shape() {
        let prims = card(7, (480, 360), 1.0, &view(), Card::Rounded, 0.0);
        let picture = prims.iter().find(|p| p.slot == 7).expect("the picture");
        let [x, y, w, h] = picture.geom;
        assert!((w - 1920.0 * 0.22).abs() < 0.01 && (h - w * 0.75).abs() < 0.01);
        assert!((x + w - (1920.0 - 28.0)).abs() < 0.01 && (y + h - (1080.0 - 28.0)).abs() < 0.01);
        assert_eq!(picture.radius, 18.0, "rounded");
        assert_eq!(picture.uv, [0.0, 0.0, 1.0, 1.0], "the whole picture");
        let faded = card(7, (480, 360), 0.5, &view(), Card::Rounded, 0.0);
        assert_eq!(faded.iter().find(|p| p.slot == 7).unwrap().color[3], 0.5);
    }

    #[test]
    fn the_card_is_cut_to_the_shape_the_board_asks_for() {
        let v = view();
        let picture = |shape, now| {
            card(7, (640, 480), 1.0, &v, shape, now)
                .into_iter()
                .find(|p| p.slot == 7)
                .expect("the picture")
        };
        let rounded = picture(Card::Rounded, 0.0);
        // Round: a square as tall as the rounded card, rounded all the
        // way, in the same corner, showing the picture's middle square.
        let round = picture(Card::Round, 0.0);
        let [x, y, w, h] = round.geom;
        assert_eq!(w, h);
        assert!((h - rounded.geom[3]).abs() < 0.01, "as tall as the rounded one");
        assert_eq!(round.radius, w / 2.0, "a circle");
        assert_eq!(round.uv, [0.125, 0.0, 0.875, 1.0]);
        assert!((x + w - (1920.0 - 28.0)).abs() < 0.01 && (y + h - (1080.0 - 28.0)).abs() < 0.01);
        let square = picture(Card::Square, 0.0);
        assert_eq!((square.geom, square.uv), (round.geom, round.uv));
        assert!(square.radius > 0.0 && square.radius < 10.0, "a square, its corners barely softened");
        let blob = picture(Card::Blob, 0.0);
        assert_eq!((blob.kind, blob.geom, blob.uv), (KIND_BLOB, round.geom, round.uv));
        assert_eq!(blob.paper[1], 1.0, "it carries the picture");
        assert_ne!(picture(Card::Blob, 1.0).paper[0], blob.paper[0], "its outline moves with the clock");
        // Its edge and its shadow are blobs too, moving with it.
        let edges = card(7, (640, 480), 1.0, &v, Card::Blob, 0.0);
        assert!(edges.iter().filter(|p| p.kind == KIND_BLOB).count() >= 3, "{edges:?}");
    }

}
