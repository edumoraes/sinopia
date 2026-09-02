//! The brush: its settings, the tip a stroke carries into the document,
//! and the ring the pointer shows. Pure.
//!
//! Settings are session state (§6.2), global to the window as in
//! Photoshop, and only the keyboard changes them: `[` `]` for size, `{`
//! `}` for hardness, the digits for opacity.

use crate::doc::{Path, Stroke};
use crate::editor::PEN_WIDTH;
use crate::scene::{Prim, Rgba, polyline_prims};

/// Brush size in world units (logical px at zoom 1): the diameter.
pub const SIZE_MIN: f64 = 1.0;
pub const SIZE_MAX: f64 = 500.0;
/// What `{` and `}` change the hardness by.
pub const HARDNESS_STEP: f64 = 0.25;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Brush {
    pub size: f64,
    /// The stroke's opacity as one shape, 0–1.
    pub opacity: f64,
    /// How much of the radius is crisp, 0–1: 1 is a pencil's edge, 0
    /// fades from the center out.
    pub hardness: f64,
}

impl Default for Brush {
    fn default() -> Brush {
        Brush {
            size: 16.0,
            opacity: 1.0,
            hardness: 0.5,
        }
    }
}

/// What `]` adds at `size` — Photoshop's steps: one below ten, ten below
/// a hundred, twenty-five below two hundred, fifty below three hundred,
/// a hundred past that.
pub fn size_step(size: f64) -> f64 {
    if size < 10.0 {
        1.0
    } else if size < 100.0 {
        10.0
    } else if size < 200.0 {
        25.0
    } else if size < 300.0 {
        50.0
    } else {
        100.0
    }
}

impl Brush {
    pub fn grow(&mut self) {
        self.size = (self.size + size_step(self.size)).min(SIZE_MAX);
    }

    /// Steps by what a grow from just below would have added, so a grow
    /// and a shrink undo each other at the band edges too.
    pub fn shrink(&mut self) {
        self.size = (self.size - size_step(self.size - 1.0)).max(SIZE_MIN);
    }

    pub fn harder(&mut self) {
        self.hardness = (self.hardness + HARDNESS_STEP).min(1.0);
    }

    pub fn softer(&mut self) {
        self.hardness = (self.hardness - HARDNESS_STEP).max(0.0);
    }

    /// `1`–`9` are 10%–90%, `0` is 100%; anything else is not a digit
    /// and changes nothing.
    pub fn set_opacity_digit(&mut self, digit: u8) {
        self.opacity = match digit {
            0 => 1.0,
            1..=9 => f64::from(digit) / 10.0,
            _ => return,
        };
    }

    pub fn tip(&self) -> Tip {
        Tip {
            width: self.size,
            opacity: self.opacity,
            hardness: self.hardness,
        }
    }
}

/// What a stroke is drawn with, taken at the press and written into the
/// path on release: the pencil's constant or the brush's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tip {
    /// World units.
    pub width: f64,
    pub opacity: f64,
    pub hardness: f64,
}

impl Tip {
    pub const PENCIL: Tip = Tip {
        width: PEN_WIDTH,
        opacity: 1.0,
        hardness: 1.0,
    };

    pub fn of(path: &Path) -> Tip {
        Tip {
            width: path.width,
            opacity: path.opacity,
            hardness: path.hardness,
        }
    }

    /// The tip one stroke of a `paint` was laid with. A raster layer keeps
    /// them stroke by stroke, so each one is composited as it was made.
    pub fn of_stroke(s: &Stroke) -> Tip {
        Tip {
            width: s.width,
            opacity: s.opacity,
            hardness: s.hardness,
        }
    }

    /// A crisp, opaque stroke draws straight onto the frame; anything
    /// else has to be composited as one shape, or its segments would
    /// darken wherever they overlap.
    pub fn is_direct(&self) -> bool {
        self.opacity >= 1.0 && self.hardness >= 1.0
    }
}

/// Points around the ring.
const RING_POINTS: usize = 48;

/// The pointer's ring: a one-logical-pixel circle of `radius` px around
/// `center`, so the brush's size shows before it paints.
pub fn ring_prims(center: (f32, f32), radius: f32, scale: f32, color: Rgba) -> Vec<Prim> {
    let points: Vec<(f32, f32)> = (0..=RING_POINTS)
        .map(|i| {
            let a = i as f32 / RING_POINTS as f32 * std::f32::consts::TAU;
            (center.0 + radius * a.cos(), center.1 + radius * a.sin())
        })
        .collect();
    polyline_prims(&points, 0.5 * scale, color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Path;
    use crate::editor::PEN_WIDTH;
    use crate::scene::KIND_SEGMENT;

    #[test]
    fn defaults_are_a_mid_sized_soft_opaque_brush() {
        let b = Brush::default();
        assert_eq!((b.size, b.opacity, b.hardness), (16.0, 1.0, 0.5));
    }

    #[test]
    fn grow_and_shrink_follow_photoshop_steps_within_bounds() {
        let mut b = Brush {
            size: 9.0,
            ..Brush::default()
        };
        b.grow();
        assert_eq!(b.size, 10.0, "by one below ten");
        b.grow();
        assert_eq!(b.size, 20.0, "by ten below a hundred");
        b.size = 175.0;
        b.grow();
        assert_eq!(b.size, 200.0, "by twenty-five below two hundred");
        b.grow();
        assert_eq!(b.size, 250.0, "by fifty below three hundred");
        b.grow();
        b.grow();
        assert_eq!(b.size, 400.0, "by a hundred past it");
        b.grow();
        b.grow();
        assert_eq!(b.size, SIZE_MAX, "and never past the largest");

        b.size = 10.0;
        b.shrink();
        assert_eq!(b.size, 9.0, "back down by one");
        b.size = 100.0;
        b.shrink();
        assert_eq!(b.size, 90.0);
        b.size = 200.0;
        b.shrink();
        assert_eq!(b.size, 175.0, "a grow is undone by a shrink");
        b.size = 1.0;
        b.shrink();
        assert_eq!(b.size, SIZE_MIN, "and never below the smallest");
    }

    #[test]
    fn harder_and_softer_step_a_quarter_and_clamp() {
        let mut b = Brush::default();
        b.harder();
        assert_eq!(b.hardness, 0.75);
        b.harder();
        b.harder();
        assert_eq!(b.hardness, 1.0);
        b.softer();
        b.softer();
        b.softer();
        b.softer();
        assert_eq!(b.hardness, 0.0);
        b.softer();
        assert_eq!(b.hardness, 0.0);
    }

    #[test]
    fn opacity_digits_map_one_to_nine_and_zero() {
        let mut b = Brush::default();
        b.set_opacity_digit(1);
        assert_eq!(b.opacity, 0.1);
        b.set_opacity_digit(5);
        assert_eq!(b.opacity, 0.5);
        b.set_opacity_digit(9);
        assert_eq!(b.opacity, 0.9);
        b.set_opacity_digit(0);
        assert_eq!(b.opacity, 1.0);
        b.set_opacity_digit(5);
        b.set_opacity_digit(12);
        assert_eq!(b.opacity, 0.5, "not a digit: ignored");
    }

    #[test]
    fn tip_of_a_pencil_and_of_a_brush() {
        assert_eq!(
            Tip::PENCIL,
            Tip {
                width: PEN_WIDTH,
                opacity: 1.0,
                hardness: 1.0
            }
        );
        assert!(Tip::PENCIL.is_direct(), "the pencil needs no compositing");
        let soft = Brush::default().tip();
        assert_eq!((soft.width, soft.opacity, soft.hardness), (16.0, 1.0, 0.5));
        assert!(!soft.is_direct(), "a soft edge has to be composited");
        let hard = Brush {
            hardness: 1.0,
            ..Brush::default()
        };
        assert!(hard.tip().is_direct());
        let faint = Brush {
            hardness: 1.0,
            opacity: 0.5,
            ..Brush::default()
        };
        assert!(!faint.tip().is_direct(), "so does translucency");
    }

    #[test]
    fn tip_of_a_path_reads_its_fields() {
        let p = Path {
            id: "p".into(),
            layer: "l".into(),
            curves: vec![],
            stroke: "#000".into(),
            width: 7.0,
            opacity: 0.25,
            hardness: 0.75,
            rotation: 0.0,
        };
        assert_eq!(
            Tip::of(&p),
            Tip {
                width: 7.0,
                opacity: 0.25,
                hardness: 0.75
            }
        );
    }

    #[test]
    fn ring_is_a_closed_polyline_around_the_centre() {
        let (cx, cy, r) = (100.0, 50.0, 12.0);
        let prims = ring_prims((cx, cy), r, 2.0, [0.0, 0.0, 0.0, 1.0]);
        assert!(prims.len() >= 24, "smooth enough to read as a circle");
        for p in &prims {
            assert_eq!(p.kind, KIND_SEGMENT);
            assert_eq!(p.radius, 1.0, "one logical pixel wide on a 2x display");
            for (x, y) in [(p.geom[0], p.geom[1]), (p.geom[2], p.geom[3])] {
                let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
                assert!((d - r).abs() < 1e-3, "{d} off the radius");
            }
        }
        let first = prims[0].geom;
        let last = prims[prims.len() - 1].geom;
        assert!(
            (first[0] - last[2]).abs() < 1e-3 && (first[1] - last[3]).abs() < 1e-3,
            "the ring closes"
        );
    }
}
