//! What the hands put on screen (pure): the laser and its trail, a ring
//! on every pinch and a line between two, the chevron of a turned slide,
//! and the presenter's camera in the corner of a show — the board's own
//! prims, drawn over everything at the screen's rate.

use super::gesture::{FLASH_S, Gestures, Pt, TRAIL_S, Turn};
use crate::scene::{Prim, Rgba, ScreenRect, View, parse_color, with_alpha};

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

/// The presenter's camera in the bottom right corner of the window: a
/// card of rounded corners over a soft shadow, with a hairline round it,
/// faded in by `alpha`. `size` is the picture's, which keeps its shape.
pub fn card(slot: u32, size: (u32, u32), alpha: f32, view: &View) -> Vec<Prim> {
    let s = view.scale as f32;
    let (vw, vh) = (view.viewport.w as f32, view.viewport.h as f32);
    let w = (vw * 0.22).min(460.0 * s);
    let h = w * size.1 as f32 / size.0.max(1) as f32;
    let r = ScreenRect {
        x: vw - w - 28.0 * s,
        y: vh - h - 28.0 * s,
        w,
        h,
    };
    let round = 18.0 * s;
    let shadow = ScreenRect { y: r.y + 6.0 * s, ..r };
    vec![
        Prim::soft(shadow, round, 16.0 * s, [0.0, 0.0, 0.0, 0.35 * alpha]),
        Prim {
            radius: round,
            color: [1.0, 1.0, 1.0, alpha],
            ..Prim::image(r, r.center(), 0.0, slot)
        },
        Prim::rounded(r, round, [1.0, 1.0, 1.0, 0.55 * alpha]).ring(2.0 * s),
    ]
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
        let prims = card(7, (480, 360), 1.0, &view());
        let picture = prims.iter().find(|p| p.slot == 7).expect("the picture");
        let [x, y, w, h] = picture.geom;
        assert!((w - 1920.0 * 0.22).abs() < 0.01 && (h - w * 0.75).abs() < 0.01);
        assert!((x + w - (1920.0 - 28.0)).abs() < 0.01 && (y + h - (1080.0 - 28.0)).abs() < 0.01);
        assert_eq!(picture.radius, 18.0, "rounded");
        let faded = card(7, (480, 360), 0.5, &view());
        assert_eq!(faded.iter().find(|p| p.slot == 7).unwrap().color[3], 0.5);
    }
}
