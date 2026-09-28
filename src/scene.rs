//! Scene → render data, without touching the GPU (pure, testable).
//!
//! Camera convention: `camera.x/y` is the WORLD point shown at the viewport
//! center; `zoom * scale` multiplies world units into physical pixels, so one
//! world unit is one logical pixel at zoom 1 regardless of the display.
//! `screen = (world - camera) * zoom * scale + viewport / 2`
//!
//! Everything drawn is a [`Prim`]: a signed-distance primitive the shader
//! rasterizes with analytic antialiasing — a rounded box (rect fills, grid
//! dots, dock panel) or a round-capped segment (pen strokes, icons). One
//! pipeline, painter's order.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};

use crate::brush::Tip;
use crate::curve::{self, Cubic};
use crate::doc::{BlendMode, Camera, Document, Element, Envelope, Kind, Layer, Paper, Pressure, Stamp};

/// Viewport in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    pub w: u32,
    pub h: u32,
}

/// The whole world → screen mapping: camera, viewport and HiDPI scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub camera: Camera,
    pub viewport: Viewport,
    /// Window scale factor (physical px per logical px).
    pub scale: f64,
}

impl View {
    pub fn px_per_world(&self) -> f64 {
        self.camera.zoom * self.scale
    }

    pub fn world_to_screen(&self, wx: f64, wy: f64) -> (f64, f64) {
        let k = self.px_per_world();
        (
            (wx - self.camera.x) * k + f64::from(self.viewport.w) / 2.0,
            (wy - self.camera.y) * k + f64::from(self.viewport.h) / 2.0,
        )
    }

    pub fn screen_to_world(&self, sx: f64, sy: f64) -> (f64, f64) {
        let k = self.px_per_world();
        (
            (sx - f64::from(self.viewport.w) / 2.0) / k + self.camera.x,
            (sy - f64::from(self.viewport.h) / 2.0) / k + self.camera.y,
        )
    }

    /// The camera that shows `world` at `screen` (physical px) at `zoom`.
    /// Every pan and zoom is a case of this. Zoom is clamped to
    /// [`ZOOM_MIN`]..=[`ZOOM_MAX`]; a non-finite zoom leaves the camera as is.
    pub fn showing(&self, world: (f64, f64), screen: (f64, f64), zoom: f64) -> Camera {
        if !zoom.is_finite() {
            return self.camera;
        }
        let zoom = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        let k = zoom * self.scale;
        Camera {
            x: world.0 - (screen.0 - f64::from(self.viewport.w) / 2.0) / k,
            y: world.1 - (screen.1 - f64::from(self.viewport.h) / 2.0) / k,
            zoom,
        }
    }

    /// Multiplies the zoom by `factor`, keeping the world point under
    /// `screen` where it is.
    pub fn zoomed_at(&self, factor: f64, screen: (f64, f64)) -> Camera {
        let world = self.screen_to_world(screen.0, screen.1);
        self.showing(world, screen, self.camera.zoom * factor)
    }

    /// Moves the content by `(dx, dy)` physical px (the camera goes the
    /// other way).
    pub fn panned_by(&self, dx: f64, dy: f64) -> Camera {
        let k = self.px_per_world();
        Camera {
            x: self.camera.x - dx / k,
            y: self.camera.y - dy / k,
            zoom: self.camera.zoom,
        }
    }
}

/// Zoom range: 10% to 1000%.
pub const ZOOM_MIN: f64 = 0.1;
pub const ZOOM_MAX: f64 = 10.0;

/// Linear RGBA, straight (non-premultiplied) alpha.
pub type Rgba = [f32; 4];

/// Rect outline thickness, in screen px (constant under zoom).
pub const STROKE_PX: f32 = 2.0;

/// Fallback (gray) for a missing or invalid color — a document with junk in
/// it must not take the renderer down.
pub const FALLBACK_COLOR: Rgba = [0.5, 0.5, 0.5, 1.0];

/// Stands in for an image whose texture has not been uploaded yet.
pub const PLACEHOLDER_COLOR: Rgba = [0.85, 0.85, 0.87, 1.0];

/// CSS hex (`#rgb` or `#rrggbb`) → linear RGBA. Invalid → fallback.
pub fn parse_color(hex: &str) -> Rgba {
    try_parse_color(hex).unwrap_or(FALLBACK_COLOR)
}

/// CSS hex (`#rgb` or `#rrggbb`) → linear RGBA, or `None` if malformed.
pub fn try_parse_color(hex: &str) -> Option<Rgba> {
    let s = hex.strip_prefix('#')?;
    let expanded: Vec<u8> = match s.len() {
        3 => s.bytes().flat_map(|b| [b, b]).collect(),
        6 => s.bytes().collect(),
        _ => return None,
    };
    let mut rgb = [0.0f32; 3];
    for (i, pair) in expanded.as_chunks::<2>().0.iter().enumerate() {
        let text = std::str::from_utf8(pair).ok()?;
        let byte = u8::from_str_radix(text, 16).ok()?;
        rgb[i] = srgb_to_linear(f32::from(byte) / 255.0);
    }
    Some([rgb[0], rgb[1], rgb[2], 1.0])
}

pub(crate) fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear interpolation of the color channels; result is opaque.
pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        1.0,
    ]
}

/// A colour as `#rrggbb` — the form a document holds. The inverse of
/// [`parse_color`] for everything a theme derives, which is opaque: the
/// channels go back through the gamma they came in by, since a document
/// speaks sRGB and the renderer speaks linear. A round trip is exact to
/// the byte, which is all a hex can carry.
pub fn to_hex(c: Rgba) -> String {
    let byte = |v: f32| (linear_to_srgb(v).clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(c[0]), byte(c[1]), byte(c[2]))
}

pub(crate) fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

pub fn with_alpha(c: Rgba, alpha: f32) -> Rgba {
    [c[0], c[1], c[2], alpha]
}

/// Axis-aligned rectangle in screen px.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScreenRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl ScreenRect {
    pub fn contains(&self, px: f64, py: f64) -> bool {
        let (px, py) = (px as f32, py as f32);
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// Shrinks by `d` on every side (grows for negative `d`).
    pub fn inset(&self, d: f32) -> ScreenRect {
        ScreenRect {
            x: self.x + d,
            y: self.y + d,
            w: self.w - 2.0 * d,
            h: self.h - 2.0 * d,
        }
    }

    pub fn offset(&self, dx: f32, dy: f32) -> ScreenRect {
        ScreenRect {
            x: self.x + dx,
            y: self.y + dy,
            ..*self
        }
    }

    #[cfg(test)]
    pub fn contains_rect(&self, other: &ScreenRect) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.x + other.w <= self.x + self.w
            && other.y + other.h <= self.y + self.h
    }

    /// The overlap, or `None` when there is no area to it.
    pub fn intersect(&self, other: &ScreenRect) -> Option<ScreenRect> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x + self.w).min(other.x + other.w);
        let y1 = (self.y + self.h).min(other.y + other.h);
        (x1 > x0 && y1 > y0).then_some(ScreenRect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }

    /// The smallest rect holding both.
    pub fn union(&self, other: &ScreenRect) -> ScreenRect {
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = (self.x + self.w).max(other.x + other.w);
        let y1 = (self.y + self.h).max(other.y + other.h);
        ScreenRect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }

    /// Grown outward to whole pixels.
    pub fn snapped(&self) -> ScreenRect {
        let x0 = self.x.floor();
        let y0 = self.y.floor();
        ScreenRect {
            x: x0,
            y: y0,
            w: (self.x + self.w).ceil() - x0,
            h: (self.y + self.h).ceil() - y0,
        }
    }
}

pub const KIND_BOX: u32 = 0;
pub const KIND_SEGMENT: u32 = 1;
/// A box that samples the texture in its slot instead of a flat color.
pub const KIND_IMAGE: u32 = 2;
/// A box whose own coverage is eaten into by the texture in its slot:
/// a round nib wearing a grain, which is not the same as being one.
pub const KIND_GRAIN: u32 = 3;

/// Which texture slot the renderer has uploaded for each blob hash. An
/// image the renderer has not caught up with yet is missing from the map.
pub type ImageSlots = std::collections::HashMap<String, u32>;

/// Multiplies the sampled texel: an image passes through untouched.
const NO_TINT: Rgba = [1.0, 1.0, 1.0, 1.0];

/// Where the art the canvas stamps from is: the sheet's texture slot,
/// how it is cut up, and which cell each image is in by the name a
/// stroke calls it. What [`ImageSlots`] is for a board's images — a
/// stroke names its nib and its paper, the renderer says where they are.
///
/// One sheet holds both, in two bands: the nibs in a grid of `px`-square
/// cells `cols` across from the top, then the papers under them in
/// cells of their own size. A dab can wear a nib and a paper at once —
/// thirty of the shipped brushes do — and one sheet is what lets it be
/// one draw.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shapes {
    pub slot: u32,
    pub cols: u16,
    pub rows: u16,
    /// One nib cell, in texels.
    pub px: u16,
    pub cells: std::collections::HashMap<String, u16>,
    /// One paper cell, in texels. Bigger than a nib's: a tile of paper
    /// covers hundreds of world units, not one dab.
    pub paper_px: u16,
    pub papers: std::collections::HashMap<String, u16>,
}

/// One nib image's place: its cell of the sheet, and the slot the sheet
/// is in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    pub uv: [f32; 4],
    pub slot: u32,
}

/// A nib's own art, once the sheet has been asked where it is:
/// Sketchbook's two kinds, which do quite different things. A shape is
/// stamped in place of the round dab and is its own edge; a grain is
/// worn over one, which keeps its edge and its ramp and is eaten into.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Art {
    #[default]
    Round,
    Shape(Cell),
    Grain(Cell),
}

/// The paper as it lies under the window: which cell of the sheet, and
/// where on it a pixel of the screen sits. `scale` is screen px into
/// tiles and `offset` is where world zero falls, already wrapped into
/// the one tile it is in — the camera can be a long way from the
/// origin, and `fract` of a big number has no precision left to give.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weave {
    pub cell: Cell,
    pub scale: f32,
    pub offset: (f32, f32),
    /// How deep it bites, 0–1.
    pub depth: f32,
}

impl Weave {
    /// The paper under a window whose world zero is at `origin` screen
    /// px, one tile of it `period` px across.
    pub fn new(cell: Cell, period: f64, origin: (f64, f64), depth: f64) -> Weave {
        let scale = 1.0 / period;
        Weave {
            cell,
            scale: scale as f32,
            offset: (
                (-origin.0 * scale).rem_euclid(1.0) as f32,
                (-origin.1 * scale).rem_euclid(1.0) as f32,
            ),
            depth: depth as f32,
        }
    }
}

/// A nib measured for the screen: what a tip and a view make of it,
/// which is everything the walk needs that the [`Stamp`] itself does
/// not say.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Nib {
    /// The crisp half-width, in px.
    pub radius: f32,
    /// The edge ramp hardness gave up of that radius, in px.
    pub feather: f32,
    /// World units into px: what a scatter stated in world units is
    /// thrown by.
    pub px_per_world: f32,
    /// What its edge does over the ramp: [`Profile::falloff`].
    pub falloff: f32,
    /// Its own art, when it carries any.
    pub art: Art,
    /// The paper it is dragged over, when it is dragged over one. Not
    /// the nib's own, which is why it is beside `art` and not in it: a
    /// nib may wear a grain and a paper at once, and the two do
    /// opposite things as the nib turns.
    pub paper: Option<Weave>,
}

impl Shapes {
    /// How many papers stand across the sheet, and how many rows of
    /// them there are. A paper cell has to divide the sheet's width,
    /// which is the asset's business to keep true.
    fn paper_grid(&self) -> (u16, u16) {
        let across = match self.paper_px {
            0 => 0,
            px => self.cols * self.px / px,
        };
        match across {
            0 => (0, 0),
            n => (n, (self.papers.len() as u16).div_ceil(n)),
        }
    }

    /// The sheet, in texels: the nib band, then the paper band under it.
    fn size(&self) -> (f32, f32) {
        let (_, down) = self.paper_grid();
        (
            f32::from(self.cols) * f32::from(self.px),
            f32::from(self.rows) * f32::from(self.px) + f32::from(down) * f32::from(self.paper_px),
        )
    }

    /// Where the nib a stroke names is, or `None` when this build's
    /// sheet does not carry it — the stroke then lays a plain round
    /// nib, because a board painted somewhere else must still open.
    pub fn cell(&self, name: &str) -> Option<Cell> {
        let cell = *self.cells.get(name)?;
        if self.cols == 0 || self.rows == 0 || cell >= self.cols * self.rows {
            return None;
        }
        let (w, h) = self.size();
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        let px = f32::from(self.px);
        let (x, y) = (f32::from(cell % self.cols) * px, f32::from(cell / self.cols) * px);
        Some(Cell {
            uv: [x / w, y / h, (x + px) / w, (y + px) / h],
            slot: self.slot,
        })
    }

    /// Where the paper a stroke names is, on the same terms — and drawn
    /// in half a texel on every side. A paper is sampled by wrapping,
    /// which a nib is not: without the inset the texel at the seam is
    /// blended with the cell next door and the tiling shows as a grid.
    pub fn paper(&self, name: &str) -> Option<Cell> {
        let cell = *self.papers.get(name)?;
        let (across, down) = self.paper_grid();
        if across == 0 || cell >= across * down {
            return None;
        }
        let (w, h) = self.size();
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        let px = f32::from(self.paper_px);
        let top = f32::from(self.rows) * f32::from(self.px);
        let (x, y) = (f32::from(cell % across) * px, top + f32::from(cell / across) * px);
        let (du, dv) = (0.5 / w, 0.5 / h);
        Some(Cell {
            uv: [x / w + du, y / h + dv, (x + px) / w - du, (y + px) / h - dv],
            slot: self.slot,
        })
    }
}

/// GPU-ready primitive. Layout mirrors the shader's instance input.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Prim {
    /// Box: `x, y, w, h`. Segment: `ax, ay, bx, by`. Screen px.
    pub geom: [f32; 4],
    pub color: Rgba,
    /// Box corner radius, or segment half-width.
    pub radius: f32,
    /// Width of the edge ramp in px, centered on the outline. 0 = crisp
    /// (one pixel of antialiasing); larger values make soft shadows.
    pub feather: f32,
    pub kind: u32,
    /// Box only: turned by this many radians (clockwise) about its center.
    pub angle: f32,
    /// [`KIND_IMAGE`] only: `u0, v0, u1, v1` — the slice of the texture
    /// the box maps onto. An image takes the whole sheet; a glyph takes
    /// its own cell of the atlas.
    pub uv: [f32; 4],
    /// What the prim is cut to: `x, y, w, h` in screen px. A zero width
    /// or height is no cut at all, which is what [`NO_CLIP`] says.
    pub clip: [f32; 4],
    /// What the edge ramp does over its own width: an exponent on the
    /// coverage, 1 for the plain ramp everything but a nib is drawn
    /// with. See [`Profile::falloff`].
    pub falloff: f32,
    /// The cell of the paper the prim is dragged over: `u0, v0, u1, v1`
    /// of the same sheet `slot` names. Wrapped over, not mapped onto —
    /// the paper is the canvas's and stands still under a dab that
    /// turns. Only read when `weave` says the paper bites at all.
    pub paper: [f32; 4],
    /// How that paper lies under the prim: screen px into tiles, where
    /// world zero falls in tiles, and how deep it bites. A depth of
    /// zero is no paper, which is what everything but a dab carries.
    pub weave: [f32; 4],
    /// [`KIND_IMAGE`] only: which texture to sample. Read on the CPU, to
    /// pick the bind group — the shader never sees it.
    pub slot: u32,
}

/// A clip that cuts nothing: what every prim carries until it is put
/// inside something with an edge.
pub const NO_CLIP: [f32; 4] = [0.0, 0.0, 0.0, 0.0];

/// No paper under it: a depth of nothing, which is what every prim but
/// a papered dab carries.
const NO_PAPER: [f32; 4] = [0.0, 0.0, 0.0, 0.0];

/// The whole texture: what anything that is not a glyph samples.
const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// An edge that ramps straight across its width: everything on screen
/// but a nib whose brush names another profile.
const PLAIN_RAMP: f32 = 1.0;

impl Prim {
    pub fn rect(r: ScreenRect, color: Rgba) -> Prim {
        Prim::rounded(r, 0.0, color)
    }

    pub fn rounded(r: ScreenRect, radius: f32, color: Rgba) -> Prim {
        Prim::soft(r, radius, 0.0, color)
    }

    pub fn soft(r: ScreenRect, radius: f32, feather: f32, color: Rgba) -> Prim {
        Prim {
            geom: [r.x, r.y, r.w, r.h],
            color,
            radius,
            feather,
            kind: KIND_BOX,
            angle: 0.0,
            uv: WHOLE,
            clip: NO_CLIP,
            falloff: PLAIN_RAMP,
            paper: NO_PAPER,
            weave: NO_PAPER,
            slot: 0,
        }
    }

    /// Box `r` turned by `angle` radians about `pivot`: the box keeps its
    /// size, its center orbits the pivot.
    pub fn turned(r: ScreenRect, pivot: (f32, f32), angle: f32, color: Rgba) -> Prim {
        if angle == 0.0 {
            return Prim::rect(r, color);
        }
        let (cx, cy) = r.center();
        let (dx, dy) = (cx - pivot.0, cy - pivot.1);
        let (s, c) = angle.sin_cos();
        let (cx, cy) = (pivot.0 + c * dx - s * dy, pivot.1 + s * dx + c * dy);
        Prim {
            angle,
            ..Prim::rect(
                ScreenRect {
                    x: cx - r.w / 2.0,
                    y: cy - r.h / 2.0,
                    w: r.w,
                    h: r.h,
                },
                color,
            )
        }
    }

    /// Box `r` filled with the texture in `slot`, turned by `angle` about
    /// `pivot`. The distance field is the same as a plain box's, so the
    /// turn, the corners and the antialiasing come along.
    pub fn image(r: ScreenRect, pivot: (f32, f32), angle: f32, slot: u32) -> Prim {
        Prim {
            kind: KIND_IMAGE,
            slot,
            ..Prim::turned(r, pivot, angle, NO_TINT)
        }
    }

    /// One cell of a sheet, painted as it is: the box `r` filled with the
    /// slice `uv` names of the texture in `slot`, untinted. What draws a
    /// brush's icon, which carries its own colors.
    pub fn sprite(r: ScreenRect, uv: [f32; 4], slot: u32) -> Prim {
        Prim::glyph(r, uv, slot, NO_TINT)
    }

    /// One glyph: the box `r` filled with the cell `uv` names in the atlas
    /// living in `slot`. The atlas is white, so `color` is the ink.
    pub fn glyph(r: ScreenRect, uv: [f32; 4], slot: u32, color: Rgba) -> Prim {
        Prim {
            kind: KIND_IMAGE,
            uv,
            slot,
            ..Prim::rect(r, color)
        }
    }

    /// The same prim scaled by `k` and turned by `angle` radians
    /// (clockwise) about `pivot`, then moved by `by`. A box orbits the
    /// pivot and spins with it; a segment's endpoints simply travel.
    /// What a card in flight is drawn through, contents and all.
    pub fn transformed(self, pivot: (f32, f32), k: f32, angle: f32, by: (f32, f32)) -> Prim {
        let (sin, cos) = angle.sin_cos();
        let map = |x: f32, y: f32| {
            let (dx, dy) = (k * (x - pivot.0), k * (y - pivot.1));
            (
                pivot.0 + cos * dx - sin * dy + by.0,
                pivot.1 + sin * dx + cos * dy + by.1,
            )
        };
        if self.kind == KIND_SEGMENT {
            let [ax, ay, bx, by] = self.geom;
            let (ax, ay) = map(ax, ay);
            let (bx, by) = map(bx, by);
            return Prim {
                geom: [ax, ay, bx, by],
                radius: self.radius * k,
                feather: self.feather * k,
                ..self
            };
        }
        let [x, y, w, h] = self.geom;
        let (cx, cy) = map(x + w / 2.0, y + h / 2.0);
        let (w, h) = (w * k, h * k);
        Prim {
            geom: [cx - w / 2.0, cy - h / 2.0, w, h],
            radius: self.radius * k,
            feather: self.feather * k,
            angle: self.angle + angle,
            ..self
        }
    }

    /// The same prim, cut to `to`. What falls outside is not drawn, and
    /// the cut is antialiased like every other edge — the clip is one
    /// more box in the same distance field.
    pub fn clipped(self, to: ScreenRect) -> Prim {
        Prim {
            clip: [to.x, to.y, to.w, to.h],
            ..self
        }
    }

    /// One dab of a stamped stroke: a nib `half` px across each way,
    /// ramping over `feather`, turned by `angle` about its own center.
    /// A round nib when its half extents are equal; a flattened capsule
    /// when they are not, which is as close to an ellipse as the box
    /// field comes.
    pub fn dab(center: (f32, f32), half: (f32, f32), feather: f32, angle: f32, color: Rgba) -> Prim {
        let r = ScreenRect {
            x: center.0 - half.0,
            y: center.1 - half.1,
            w: 2.0 * half.0,
            h: 2.0 * half.1,
        };
        Prim {
            angle,
            ..Prim::soft(r, half.0.min(half.1), feather, color)
        }
    }

    /// A dab that stamps a nib's own shape: the cell `nib` names of the
    /// sheet it lives on, over the box a round dab would have filled and
    /// turned the same way. The sheet is full gray under its coverage,
    /// so `color` is the ink — the glyph atlas over again, and the shape
    /// is its own edge, with no ramp of the box's to add.
    pub fn shaped_dab(
        center: (f32, f32),
        half: (f32, f32),
        angle: f32,
        cell: Cell,
        color: Rgba,
    ) -> Prim {
        Prim {
            kind: KIND_IMAGE,
            uv: cell.uv,
            slot: cell.slot,
            // Square corners: the shape's own alpha is the only edge,
            // and a rounded box would bite into it.
            radius: 0.0,
            ..Prim::dab(center, half, 0.0, angle, color)
        }
    }

    /// A round dab wearing a grain: the same box the plain dab fills,
    /// with its own edge and its own ramp, and the cell `cell` names
    /// eating into its coverage. The grain turns with the nib, being
    /// the nib's own — unlike the canvas's paper, which would stand
    /// still under it.
    pub fn grained_dab(
        center: (f32, f32),
        half: (f32, f32),
        feather: f32,
        angle: f32,
        cell: Cell,
        color: Rgba,
    ) -> Prim {
        Prim {
            kind: KIND_GRAIN,
            uv: cell.uv,
            slot: cell.slot,
            ..Prim::dab(center, half, feather, angle, color)
        }
    }

    /// Whether the prim reads the texture in its slot at all, which is
    /// what decides where one run of them ends and the next begins. An
    /// image or a glyph maps one onto itself and a grain is eaten into
    /// by one — and so is any dab dragged over a paper, whatever kind
    /// it is otherwise. Everything else is a flat colour and does not
    /// care which texture is bound behind it.
    fn samples(&self) -> bool {
        self.kind == KIND_IMAGE || self.kind == KIND_GRAIN || self.weave[3] > 0.0
    }

    /// The same dab, dragged over a paper: its coverage is eaten into
    /// by `weave`'s cell wherever the paper's own is thin. The cell is
    /// on the sheet the nibs are on, so a dab already stamping a shape
    /// or wearing a grain keeps the one slot it had; a plain round one
    /// takes the sheet's, since it has to sample it now.
    pub fn papered(self, weave: Weave) -> Prim {
        Prim {
            paper: weave.cell.uv,
            weave: [weave.scale, weave.offset.0, weave.offset.1, weave.depth],
            slot: weave.cell.slot,
            ..self
        }
    }

    pub fn circle(cx: f32, cy: f32, radius: f32, color: Rgba) -> Prim {
        let r = ScreenRect {
            x: cx - radius,
            y: cy - radius,
            w: 2.0 * radius,
            h: 2.0 * radius,
        };
        Prim::rounded(r, radius, color)
    }

    pub fn segment(a: (f32, f32), b: (f32, f32), half_width: f32, color: Rgba) -> Prim {
        Prim::soft_segment(a, b, half_width, 0.0, color)
    }

    /// A segment whose edge ramps over `feather` px instead of one.
    pub fn soft_segment(
        a: (f32, f32),
        b: (f32, f32),
        half_width: f32,
        feather: f32,
        color: Rgba,
    ) -> Prim {
        Prim {
            geom: [a.0, a.1, b.0, b.1],
            color,
            radius: half_width,
            feather,
            kind: KIND_SEGMENT,
            angle: 0.0,
            uv: WHOLE,
            clip: NO_CLIP,
            falloff: PLAIN_RAMP,
            paper: NO_PAPER,
            weave: NO_PAPER,
            slot: 0,
        }
    }

    /// The box `r` filled with the scratch texture's own pixels under it:
    /// what lays a composited group back on the frame. The scratch holds
    /// premultiplied color, so `opacity` scales every channel.
    pub fn composite(r: ScreenRect, viewport: Viewport, slot: u32, opacity: f32) -> Prim {
        let (w, h) = (viewport.w as f32, viewport.h as f32);
        let uv = [r.x / w, r.y / h, (r.x + r.w) / w, (r.y + r.h) / h];
        Prim::glyph(r, uv, slot, [opacity; 4])
    }

    /// The same composite, laid with `mode`. The mode rides where a dab
    /// carries its paper, one past its number so that nothing is none: a
    /// composite is never dragged over a paper, and the shader reads the
    /// paper only where the weave says it bites.
    pub fn in_mode(self, mode: BlendMode) -> Prim {
        Prim {
            paper: [(mode_number(mode) + 1) as f32, 0.0, 0.0, 0.0],
            weave: [0.0; 4],
            ..self
        }
    }

    /// The mode a composite is laid with, when it is laid with one.
    #[allow(dead_code)] // the planner's tests read it back
    pub fn mode(&self) -> Option<BlendMode> {
        if self.kind != KIND_IMAGE || self.weave[3] > 0.0 || self.paper[0] < 1.0 {
            return None;
        }
        BlendMode::ALL.get(self.paper[0] as usize - 1).copied()
    }

    /// The painted area plus the edge ramp: what the shader rasterizes,
    /// and so what a wipe has to cover. A prim that is cut answers only
    /// the part the cut lets through — a stroke mostly outside a frame
    /// does not make the compositor pay for ink nobody sees — and one
    /// cut away entirely answers an empty box.
    pub fn painted_bounds(&self) -> ScreenRect {
        let painted = self.bounds().inset(-(self.feather.max(1.0) * 0.5 + 1.0));
        if self.clip == NO_CLIP {
            return painted;
        }
        let [x, y, w, h] = self.clip;
        let cut = ScreenRect { x, y, w, h };
        painted.intersect(&cut).unwrap_or(ScreenRect {
            x: painted.x,
            y: painted.y,
            w: 0.0,
            h: 0.0,
        })
    }

    /// Painted area, ignoring the antialiasing ramp.
    pub fn bounds(&self) -> ScreenRect {
        let [a, b, c, d] = self.geom;
        if self.kind == KIND_SEGMENT {
            let r = self.radius;
            let (x0, x1) = (a.min(c) - r, a.max(c) + r);
            let (y0, y1) = (b.min(d) - r, b.max(d) + r);
            ScreenRect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            }
        } else {
            let (s, co) = self.angle.sin_cos();
            // Half extents of the turned box, projected on the axes.
            let (hw, hh) = (c / 2.0, d / 2.0);
            let (ex, ey) = (co.abs() * hw + s.abs() * hh, s.abs() * hw + co.abs() * hh);
            ScreenRect {
                x: a + hw - ex,
                y: b + hh - ey,
                w: 2.0 * ex,
                h: 2.0 * ey,
            }
        }
    }
}

/// A stretch of prims the renderer can draw in one go: they all sample
/// the same texture, so one bind group covers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    /// Instance range, `start..end`.
    pub start: u32,
    pub end: u32,
    /// The texture every [`KIND_IMAGE`] prim in the range samples.
    pub slot: u32,
}

/// Cuts `prims` into runs by the texture they need. Only [`KIND_IMAGE`]
/// prims sample, so a run breaks where one image follows another with a
/// different texture — never on the flat prims between them. Paint order
/// is preserved, which is what keeps a board's z-order honest with a
/// single instance buffer.
pub fn runs(prims: &[Prim]) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    let mut start = 0u32;
    let mut slot: Option<u32> = None;
    for (i, p) in prims.iter().enumerate() {
        if !p.samples() {
            continue;
        }
        match slot {
            Some(s) if s != p.slot => {
                out.push(Run {
                    start,
                    end: i as u32,
                    slot: s,
                });
                start = i as u32;
                slot = Some(p.slot);
            }
            _ => slot = Some(p.slot),
        }
    }
    if !prims.is_empty() {
        out.push(Run {
            start,
            end: prims.len() as u32,
            slot: slot.unwrap_or(0),
        });
    }
    out
}

/// Polyline in screen px → one round-capped segment per span; overlapping
/// caps make the joins. Repeated points are skipped; a degenerate polyline
/// is still visible as a dot.
pub fn polyline_prims(points: &[(f32, f32)], half_width: f32, color: Rgba) -> Vec<Prim> {
    soft_polyline_prims(points, half_width, 0.0, color)
}

/// An icon drawn as polylines on a `grid`-unit square, mapped onto a
/// `box_px` logical px box centered in `rect` and stroked `stroke_px`
/// logical px wide with round caps and joins — how the chrome draws its
/// icons. A closed outline repeats its first point.
pub fn icon_prims(
    lines: &[&[(f32, f32)]],
    rect: ScreenRect,
    grid: f32,
    box_px: f32,
    stroke_px: f32,
    scale: f32,
    color: Rgba,
) -> Vec<Prim> {
    let (cx, cy) = rect.center();
    let unit = box_px / grid * scale;
    let half_width = stroke_px / 2.0 * scale;
    let half_grid = grid / 2.0;
    lines
        .iter()
        .flat_map(|line| {
            let points: Vec<(f32, f32)> = line
                .iter()
                .map(|&(x, y)| (cx + (x - half_grid) * unit, cy + (y - half_grid) * unit))
                .collect();
            polyline_prims(&points, half_width, color)
        })
        .collect()
}

/// [`polyline_prims`] with an edge ramp `feather` px wide on every span.
pub fn soft_polyline_prims(
    points: &[(f32, f32)],
    half_width: f32,
    feather: f32,
    color: Rgba,
) -> Vec<Prim> {
    let Some(&first) = points.first() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut prev = first;
    for &p in &points[1..] {
        if p == prev {
            continue;
        }
        out.push(Prim::soft_segment(prev, p, half_width, feather, color));
        prev = p;
    }
    if out.is_empty() {
        let r = ScreenRect {
            x: first.0 - half_width,
            y: first.1 - half_width,
            w: 2.0 * half_width,
            h: 2.0 * half_width,
        };
        out.push(Prim::soft(r, half_width, feather, color));
    }
    out
}

/// Half of a pen width in screen px. Ink scales with zoom but never drops
/// below one pixel, so zoomed-out strokes stay visible.
fn half_width_px(width: f64, view: &View) -> f32 {
    ((width * view.px_per_world()) as f32 / 2.0).max(0.5)
}

/// What a stroke `width` world units wide at `hardness` is drawn with:
/// `(radius, feather)` in px. The ramp takes `1 − hardness` of the
/// radius and the geometry gives it up, so the ramp ends where the crisp
/// edge would have been: a soft stroke fades inside its width, it does
/// not grow past it.
pub fn soft_radius(width: f64, hardness: f64, view: &View) -> (f32, f32) {
    let r = half_width_px(width, view);
    let feather = (1.0 - hardness.clamp(0.0, 1.0)) as f32 * r;
    (r - feather / 2.0, feather)
}

/// What one unit of Sketchbook's spacing is worth, as a fraction of the
/// nib's width. Its own help names the band — 0.1 is "a very dense
/// brush", 10.0 the top — and, decisively, 1.2 as the Pencil's default.
/// A default pencil has to draw solid, and that is what settles the
/// unit: at a quarter of a width each, 1.2 is 30% of a diameter, where
/// every paint program's default spacing sits, and the ink pinches to
/// 95% of its width between two dabs. Read as whole widths that same
/// pencil would not touch itself at all, and read as radii it would
/// pinch to 80% — a beaded stroke, which is not what Sketchbook ships.
const SPACING_UNIT: f32 = 0.25;

/// The closest two dabs are allowed to sit, in px. Below this they stop
/// telling apart and only cost: a hair-thin brush at the tightest
/// spacing would otherwise lay tens of thousands of them per stroke.
const STAMP_STEP_MIN: f32 = 1.0;

/// A nib is never let vanish, however flat it is squished.
pub const NIB_MIN_PX: f32 = 0.5;

/// One throw of the dice, in `-1..1`: splitmix64 over the stroke's own
/// seed, the dab's place in it and a salt that keeps one property's
/// throw from tracking another's. Deterministic on purpose — the
/// document is redrawn every frame, and a dab that rolled again each
/// time would shimmer.
fn dice(seed: u64, index: u32, salt: u64) -> f64 {
    let mut z = seed
        .wrapping_add(u64::from(index).wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_add(salt);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // The top 53 bits are the ones a f64 can hold exactly.
    (z >> 11) as f64 / f64::from(1u32 << 26) / f64::from(1u32 << 27) * 2.0 - 1.0
}

/// The seed a stroke scatters by: drawn from where it began, in world
/// units, and from nothing that moves or grows. The camera must not
/// re-roll it, and neither must the next point of a stroke still being
/// drawn — a live stroke and the path it is fitted into start at the
/// same place, so the ink does not jump at the release.
fn seed_at(start: [f64; 2]) -> u64 {
    start[0]
        .to_bits()
        .rotate_left(17)
        .wrapping_mul(0xD6E8_FEB8_6659_FD93)
        ^ start[1].to_bits()
}

/// How far a swept stroke's outline may stray from the taper the pen
/// asked for, in px: the same quarter pixel the curve is flattened to,
/// so the width is as true to the hand as the line is to the curve.
const TAPER_TOLERANCE_PX: f32 = 0.25;

/// The spans a swept nib lays along `points` (screen px). The pencil is
/// the only nib that sweeps, and it thins with the hand: `drive` is how
/// much of its width a lighter touch takes away and `pen` is what the
/// hand did, read by how far along the span falls — the same question
/// a dab is asked.
///
/// A span is a capsule of one width, so a taper is laid as a run of
/// them: each span is cut into as many as it takes for the radius to
/// move less than [`TAPER_TOLERANCE_PX`] across one. A stroke the pen
/// never touched is one capsule a span, as it always was.
fn swept_prims(
    points: &[(f32, f32)],
    radius: f32,
    feather: f32,
    drive: Pressure,
    pen: &Envelope,
    color: Rgba,
) -> Vec<Prim> {
    if drive.size == 0.0 || pen.pressure.is_empty() {
        return soft_polyline_prims(points, radius, feather, color);
    }
    let Some(&first) = points.first() else {
        return Vec::new();
    };
    let spans: Vec<f32> = points
        .windows(2)
        .map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1))
        .collect();
    let total: f32 = spans.iter().sum();
    // What the nib is worth `u` of the way along. The ramp is a share
    // of the radius, so it thins with it, exactly as a dab's does.
    let nib = |u: f32| {
        let press = pen.pressure_at(u);
        let narrow = Pressure::scale(drive.size, f64::from(press)) as f32;
        ((radius * narrow).max(NIB_MIN_PX), feather * narrow)
    };
    if total <= 0.0 {
        let (r, f) = nib(0.0);
        return soft_polyline_prims(&[first], r, f, color);
    }
    let mut out = Vec::new();
    let mut walked = 0.0f32;
    for (i, &span) in spans.iter().enumerate() {
        let (a, b) = (points[i], points[i + 1]);
        if span <= 0.0 {
            continue;
        }
        let (u0, u1) = (walked / total, (walked + span) / total);
        // How many capsules this span needs for its own change of
        // width to stay under the tolerance.
        let cuts = (((nib(u0).0 - nib(u1).0).abs() / TAPER_TOLERANCE_PX).ceil() as usize).max(1);
        for k in 0..cuts {
            let (s, e) = (k as f32 / cuts as f32, (k + 1) as f32 / cuts as f32);
            let at = |k: f32| (a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k);
            let (r, f) = nib(u0 + (u1 - u0) * (s + e) / 2.0);
            out.push(Prim::soft_segment(at(s), at(e), r, f, color));
        }
        walked += span;
    }
    if out.is_empty() {
        let (r, f) = nib(0.0);
        return soft_polyline_prims(&[first], r, f, color);
    }
    out
}

/// The dabs a stamped nib lays along `points` (screen px): one where
/// the press was, then one every `spacing` units of arc length after
/// it. The tail left over past the last dab is not stamped — a stroke
/// ends on a dab, as it does in Sketchbook, and the nib's own radius
/// covers the gap at any spacing anyone paints with.
///
/// A nib with a scatter is thrown off true dab by dab: its radius by
/// `scatter.size` world units and its angle by `scatter.rotation`
/// degrees, each from `seed` so the same stroke lands the same way
/// every frame. The rhythm is not thrown — see [`Property::honored`].
///
/// `pen` is what the hand did along the way. Where the brush says
/// pressure drives it, a dab is that much narrower and lays that much
/// less ink for a lighter touch, and the gap after it closes with the
/// nib — the spacing is a share of the nib's width, so a thinner nib
/// steps shorter. A stylus that turns the nib turns each dab by what it
/// said there, on top of the nib's own angle.
pub fn stamp_prims(
    points: &[(f32, f32)],
    stamp: &Stamp,
    pen: &Envelope,
    seed: u64,
    nib: Nib,
    color: Rgba,
) -> Vec<Prim> {
    let Some(&first) = points.first() else {
        return Vec::new();
    };
    let Nib {
        radius,
        feather,
        px_per_world,
        falloff,
        art,
        paper,
    } = nib;
    let squish = (stamp.roundness.clamp(0.0, 1.0) as f32).max(NIB_MIN_PX / radius.max(NIB_MIN_PX));
    let angle = stamp.rotation as f32;
    let gap = stamp.spacing as f32 * SPACING_UNIT;
    let flow = stamp.flow.clamp(0.0, 1.0) as f32;
    let drive = stamp.pressure;
    let scatter = stamp.scatter;
    let true_nib = scatter.is_true();
    let size_throw = scatter.size as f32 * px_per_world;
    let leaning = !pen.twist.is_empty();

    let lay = |at: (f32, f32), half: (f32, f32), feather: f32, turn: f32, ink: Rgba| {
        let dab = match art {
            // A shape is its own edge, with no ramp of the box's to bend.
            Art::Shape(cell) => Prim::shaped_dab(at, half, turn, cell, ink),
            Art::Grain(cell) => Prim {
                falloff,
                ..Prim::grained_dab(at, half, feather, turn, cell, ink)
            },
            Art::Round => Prim {
                falloff,
                ..Prim::dab(at, half, feather, turn, ink)
            },
        };
        // The paper goes on last and over any of the three: it is the
        // canvas's, so what the nib is says nothing about it.
        match paper {
            Some(weave) => dab.papered(weave),
            None => dab,
        }
    };
    // The way the stroke is going where the dab lands, in radians, when
    // the nib runs along it — a nib that stands still has none.
    let heading = |h: f32| if stamp.follow { h } else { 0.0 };
    // `n` is the dab's place in the stroke: what the dice are rolled
    // against, so a dab keeps its own throw however the walk arrives.
    // `u` is how far along the stroke it lands, which is what the pen
    // is asked by.
    let dab = |at: (f32, f32), n: u32, h: f32, u: f32| {
        // What a lighter touch takes off the nib and off the ink. The
        // ramp is a share of the radius, so it thins with it.
        let press = pen.pressure_at(u);
        let narrow = Pressure::scale(drive.size, f64::from(press)) as f32;
        let (radius, feather) = ((radius * narrow).max(NIB_MIN_PX), feather * narrow);
        let lighter = (Pressure::scale(drive.flow, f64::from(press))
            * Pressure::scale(drive.opacity, f64::from(press))) as f32;
        // Flow is what one dab lays; the stroke's opacity is the ceiling
        // the composite puts on the pile.
        let ink = [color[0], color[1], color[2], color[3] * flow * lighter];
        let turn = if leaning { pen.twist_at(u) } else { 0.0 };
        let step = (gap * (2.0 * radius + feather)).max(STAMP_STEP_MIN);
        let prim = if true_nib {
            lay(
                at,
                (radius, (radius * squish).max(NIB_MIN_PX)),
                feather,
                heading(h) + (angle + turn).to_radians(),
                ink,
            )
        } else {
            let r = (radius + size_throw * dice(seed, n, SALT_SIZE) as f32).max(NIB_MIN_PX);
            let softness = r / radius.max(NIB_MIN_PX);
            let turn = angle + turn + scatter.rotation as f32 * dice(seed, n, SALT_ANGLE) as f32;
            lay(
                at,
                (r, (r * squish).max(NIB_MIN_PX)),
                feather * softness,
                heading(h) + turn.to_radians(),
                ink,
            )
        };
        (prim, step)
    };

    // How long the stroke is, so that a dab knows how far along it
    // lands: the pen is read by the fraction, never by the pixel.
    let total: f32 = points
        .windows(2)
        .map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1))
        .sum();
    let mut out = Vec::new();
    let mut n = 0u32;
    // Where the walk is, and where the next dab goes, both measured
    // from the press — so the spacing is of the stroke and not of a
    // span, and a step that changes with the nib still lands true.
    let mut walked = 0.0f32;
    let mut next = 0.0f32;
    let mut prev = first;
    for &p in &points[1..] {
        let (dx, dy) = (p.0 - prev.0, p.1 - prev.1);
        let span = dx.hypot(dy);
        if span <= 0.0 {
            continue;
        }
        let along = dy.atan2(dx);
        while next <= walked + span {
            let k = (next - walked) / span;
            let u = if total > 0.0 { next / total } else { 0.0 };
            let (prim, step) = dab((prev.0 + dx * k, prev.1 + dy * k), n, along, u);
            out.push(prim);
            n += 1;
            next += step;
        }
        walked += span;
        prev = p;
    }
    // A stroke that never went anywhere is still a dab: one press, laid
    // where it was made, with the nib pointing where the brush put it.
    if out.is_empty() {
        out.push(dab(first, 0, 0.0, 0.0).0);
    }
    out
}

/// Salts, so that a dab's size and angle are thrown independently.
const SALT_SIZE: u64 = 0x51_7C_C1_B7_27_22_0A_95;
const SALT_ANGLE: u64 = 0x2545_F491_4F6C_DD1D;

/// What a tip lays along a screen polyline: a row of dabs if it stamps,
/// one swept span per segment if it does not. The pencil sweeps; every
/// brush stamps.
fn tip_prims(
    screen: &[(f32, f32)],
    tip: &Tip,
    pen: &Envelope,
    seed: u64,
    color: Rgba,
    view: &View,
    shapes: &Shapes,
) -> Vec<Prim> {
    let (radius, feather) = soft_radius(tip.width, tip.hardness, view);
    match &tip.stamp {
        Some(stamp) => stamp_prims(
            screen,
            stamp,
            pen,
            seed,
            Nib {
                radius,
                feather,
                px_per_world: view.px_per_world() as f32,
                falloff: stamp.profile.falloff(),
                // A nib whose art this build's sheet does not carry
                // lays a plain round dab: a board painted elsewhere
                // still opens, rather than opening blank.
                art: match (stamp.shape.as_deref(), stamp.grain.as_deref()) {
                    (Some(n), _) => shapes.cell(n).map_or(Art::Round, Art::Shape),
                    (None, Some(n)) => shapes.cell(n).map_or(Art::Round, Art::Grain),
                    (None, None) => Art::Round,
                },
                paper: paper_weave(stamp.paper.as_ref(), view, shapes),
            },
            color,
        ),
        None => swept_prims(screen, radius, feather, tip.drive(), pen, color),
    }
}

/// How thin a tile of paper is allowed to get on the screen. Past this
/// the weave is finer than the pixels reading it and the paper is not
/// there to be seen — so it stops getting finer rather than turning
/// into noise that changes every time the board is zoomed.
const PAPER_MIN_PX: f64 = 2.0;

/// The paper a stroke names, as it lies under this window — or `None`
/// when it names none, or one this build's sheet does not carry. The
/// stroke then lays its ink undivided, for the reason a nib the sheet
/// does not carry lays a round dab.
fn paper_weave(paper: Option<&Paper>, view: &View, shapes: &Shapes) -> Option<Weave> {
    let paper = paper?;
    let cell = shapes.paper(&paper.name)?;
    let period = (paper.period * view.px_per_world()).max(PAPER_MIN_PX);
    // World zero, not the stroke's start: the paper is the board's, so
    // two strokes crossing one place meet the same fibres.
    Some(Weave::new(
        cell,
        period,
        view.world_to_screen(0.0, 0.0),
        paper.depth,
    ))
}

/// Stroke in progress (a raw polyline in world units) → screen prims.
/// `pen` is what the hand has said so far, evened out the same way the
/// release will write it down.
pub fn stroke_prims(
    points: &[[f64; 2]],
    tip: &Tip,
    pen: &Envelope,
    color: Rgba,
    view: &View,
    shapes: &Shapes,
) -> Vec<Prim> {
    let screen: Vec<(f32, f32)> = points
        .iter()
        .map(|[x, y]| {
            let (sx, sy) = view.world_to_screen(*x, *y);
            (sx as f32, sy as f32)
        })
        .collect();
    let seed = points.first().copied().map_or(0, seed_at);
    tip_prims(&screen, tip, pen, seed, color, view, shapes)
}

/// How far the flattened polyline may stray from the curve, in px.
const FLATTEN_TOLERANCE_PX: f64 = 0.25;

/// Committed `path` (cubics in world units) → screen prims. The control
/// points are projected first — Béziers are affine-invariant — so the
/// flattening tolerance is in pixels whatever the zoom.
pub fn path_prims(
    curves: &[Cubic],
    tip: &Tip,
    pen: &Envelope,
    color: Rgba,
    view: &View,
    shapes: &Shapes,
) -> Vec<Prim> {
    let mut screen: Vec<(f32, f32)> = Vec::new();
    for c in curves {
        let projected = c.map(|[x, y]| {
            let (sx, sy) = view.world_to_screen(x, y);
            [sx, sy]
        });
        screen.extend(
            curve::flatten(&projected, FLATTEN_TOLERANCE_PX)
                .into_iter()
                .map(|[x, y]| (x as f32, y as f32)),
        );
    }
    let seed = curves.first().map_or(0, |c| seed_at(c[0]));
    tip_prims(&screen, tip, pen, seed, color, view, shapes)
}

/// How prims meet what is already on the surface they are drawn onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blend {
    /// One over the next, straight alpha: what the window is painted
    /// with, and what a finished stroke does to the sheet it lands on.
    Over,
    /// Every channel a max: the union of their coverage, and no more.
    /// What a swept stroke needs — its spans overlap at every joint,
    /// and a soft edge would bead there if they added up.
    Union,
    /// One over the next, premultiplied: coverage builds where they
    /// cross. What a stamped stroke needs — flow is what one dab lays,
    /// and a stroke crossing itself is darker for it, as paint is.
    Build,
    /// Coverage taken away instead of added: what an eraser does to the
    /// sheet its own layer is built on.
    Erase,
    /// A mix with what is there, at the lay's strength: what a group that
    /// passes through is laid back as, over the copy it was opened on.
    Mix,
    /// Laid with a blend mode: it reads what it is laid on, from a copy
    /// taken just before, and meets it as the mode says.
    Mode(BlendMode),
}

/// A blend mode's number: where it stands in [`BlendMode::ALL`], which is
/// what the shader's own table is built from.
pub fn mode_number(mode: BlendMode) -> u32 {
    BlendMode::ALL.iter().position(|m| *m == mode).unwrap_or(0) as u32
}

/// A stretch of a frame's prims that is composited as one shape: drawn
/// into the scratch texture, meeting each other as `blend` says, then
/// laid on what is under them once at `opacity`, meeting *that* as
/// `lands` says. `bounds` is what the shader rasterizes for them, ramp
/// included.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Group {
    pub start: u32,
    pub end: u32,
    pub opacity: f32,
    pub blend: Blend,
    /// How the finished shape meets what it is laid on: ink over it, or
    /// ink taken out of it.
    pub lands: Blend,
    pub bounds: ScreenRect,
}

/// A stretch of a frame's prims built on a surface of its own before it
/// is laid on what is under it: a layer, a group or a frame composited
/// as one — at its opacity, as `lays` says — or one raster layer's paint,
/// whose strokes have to be able to take ink out of each other without
/// touching the board under them. Sheets nest, one surface a depth. What
/// asks for none of this does not get one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sheet {
    pub start: u32,
    pub end: u32,
    pub bounds: ScreenRect,
    /// The strength it is laid down at.
    pub opacity: f32,
    /// How it meets what it is laid on.
    pub lays: Blend,
    /// It opens on a copy of what is under it rather than on nothing:
    /// a group passing through works on the board itself.
    pub backdrop: bool,
}

/// Everything on screen: the prims in paint order, which stretches of
/// them are composited as groups, and which are built on a sheet.
/// Groups do not overlap and come in order; sheets come outer first and
/// nest, and a group lies wholly inside the innermost sheet around it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Frame {
    pub prims: Vec<Prim>,
    pub groups: Vec<Group>,
    pub sheets: Vec<Sheet>,
}

impl Frame {
    pub fn new() -> Frame {
        Frame::default()
    }

    /// Prims drawn straight onto whatever they are inside.
    pub fn extend(&mut self, prims: impl IntoIterator<Item = Prim>) {
        self.prims.extend(prims);
    }

    /// Prims composited as one shape at `opacity`, meeting each other as
    /// `blend` says and what they land on as `lands` says. Nothing to
    /// draw makes no group.
    pub fn group(&mut self, prims: Vec<Prim>, opacity: f32, blend: Blend, lands: Blend) {
        let Some(bounds) = prims
            .iter()
            .map(Prim::painted_bounds)
            .reduce(|a, b| a.union(&b))
        else {
            return;
        };
        let start = self.prims.len() as u32;
        self.prims.extend(prims);
        self.groups.push(Group {
            start,
            end: self.prims.len() as u32,
            opacity,
            blend,
            lands,
            bounds,
        });
    }

    /// Whatever `build` puts in the frame, built on a sheet of its own
    /// and laid on what is under it as one, at full strength: what a
    /// paint that rubs itself out is drawn on.
    pub fn sheet(&mut self, build: impl FnOnce(&mut Frame)) {
        self.layer(1.0, Blend::Over, false, build);
    }

    /// Whatever `build` puts in the frame, built on a surface of its own
    /// — opened on a copy of what is under it when `backdrop` — and laid
    /// on what is under it once, at `opacity`, as `lays` says. Nothing
    /// drawn makes no surface.
    pub fn layer(&mut self, opacity: f32, lays: Blend, backdrop: bool, build: impl FnOnce(&mut Frame)) {
        let start = self.prims.len() as u32;
        // Its place is taken before what is inside is built, so the
        // sheets come outer first.
        let at = self.sheets.len();
        self.sheets.push(Sheet {
            start,
            end: start,
            bounds: ScreenRect::default(),
            opacity,
            lays,
            backdrop,
        });
        build(self);
        let end = self.prims.len() as u32;
        match self.prims[start as usize..end as usize]
            .iter()
            .map(Prim::painted_bounds)
            .reduce(|a, b| a.union(&b))
        {
            Some(bounds) => {
                self.sheets[at].end = end;
                self.sheets[at].bounds = bounds;
            }
            None => {
                self.sheets.remove(at);
            }
        }
    }

    /// A stroke's prims, direct or grouped as its tip demands. A
    /// stamped stroke builds; a swept one unions; an eraser is taken
    /// back out of what it is laid on.
    pub fn stroke(&mut self, prims: Vec<Prim>, tip: &Tip) {
        if tip.is_direct() {
            self.extend(prims);
        } else {
            let blend = match tip.stamp {
                Some(_) => Blend::Build,
                None => Blend::Union,
            };
            self.group(prims, tip.opacity as f32, blend, tip.lands());
        }
    }

    /// `other` painted after everything here.
    /// Every prim cut to `to`: nothing of the frame is drawn outside it.
    pub fn cut(&mut self, to: ScreenRect) {
        for p in &mut self.prims {
            *p = p.clipped(to);
        }
    }

    pub fn append(&mut self, other: Frame) {
        let offset = self.prims.len() as u32;
        self.prims.extend(other.prims);
        self.groups.extend(other.groups.into_iter().map(|g| Group {
            start: g.start + offset,
            end: g.end + offset,
            ..g
        }));
        self.sheets.extend(other.sheets.into_iter().map(|s| Sheet {
            start: s.start + offset,
            end: s.end + offset,
            ..s
        }));
    }
}

/// How many surfaces deep `frame`'s sheets go: what it asks the renderer
/// to hold, one a depth.
pub fn depth(frame: &Frame) -> usize {
    let mut open: Vec<u32> = Vec::new();
    let mut most = 0;
    for s in &frame.sheets {
        while open.last().is_some_and(|&end| end <= s.start) {
            open.pop();
        }
        open.push(s.end);
        most = most.max(open.len());
    }
    most
}

/// What a pass draws onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Onto {
    /// The window itself.
    Window,
    /// The sheet at this depth, from 1: a layer, a group or a frame
    /// composited as one, or a raster layer's paint that rubs itself out.
    Sheet(u8),
    /// The scratch one stroke is composited in.
    Scratch,
    /// A copy of what a surface laid with a mode is about to be laid on:
    /// what the mode reads.
    Backdrop,
}

/// What a previous pass drew offscreen, laid down now: which prim
/// samples it, and how it meets what is already there. A mix's strength
/// is the prim's own alpha, as a composite's opacity always is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lay {
    pub prim: u32,
    pub blend: Blend,
}

/// A region of one surface copied onto another before a pass begins,
/// at the same place: what a surface passing through opens on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Copy {
    pub from: Onto,
    pub to: Onto,
    pub rect: ScreenRect,
}

/// One render pass of a frame, over the prims [`passes`] hands back:
/// what it draws onto, what is copied onto it or the box it clears first
/// (where a surface is opened), what it lays down before anything else,
/// and the prims it then draws in order, meeting each other as `blend`
/// says. The first pass of a frame is always onto the window, and is the
/// one that clears it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pass {
    pub onto: Onto,
    pub copy: Option<Copy>,
    pub wipe: Option<u32>,
    pub lay: Option<Lay>,
    pub start: u32,
    pub end: u32,
    pub blend: Blend,
}

/// The compositing plan being built: the prims with the wipe and
/// composite boxes the passes need appended, and the passes themselves.
struct Plan<'a> {
    prims: Vec<Prim>,
    passes: Vec<Pass>,
    viewport: Viewport,
    window: ScreenRect,
    scratch: u32,
    /// The sheets' slots, one a depth from 1.
    sheets: &'a [u32],
    /// Where the prims that have not been drawn yet begin.
    cursor: u32,
    /// What the next pass has to lay down before anything else.
    pending: Option<Lay>,
    /// A box that clears the surface the next pass draws onto.
    wipe: Option<u32>,
    /// A copy the next pass opens its surface with.
    copy: Option<Copy>,
}

type Upcoming<'a, T> = std::iter::Peekable<std::slice::Iter<'a, T>>;

impl Plan<'_> {
    /// A box appended to the prims, and its index.
    fn box_at(&mut self, prim: Prim) -> u32 {
        self.prims.push(prim);
        self.prims.len() as u32 - 1
    }

    /// Draws everything up to `end` onto `onto`. A pass with nothing to
    /// draw, open or lay down is not one — except the first onto the
    /// window, which is what clears it.
    fn run(&mut self, onto: Onto, end: u32) {
        let first = self.passes.is_empty() && onto == Onto::Window;
        if first || self.wipe.is_some() || self.copy.is_some() || self.pending.is_some() || end > self.cursor
        {
            self.passes.push(Pass {
                onto,
                copy: self.copy.take(),
                wipe: self.wipe.take(),
                lay: self.pending.take(),
                start: self.cursor,
                end,
                blend: Blend::Over,
            });
        }
        self.cursor = end;
    }

    /// The groups that begin before `limit`, each drawn offscreen and
    /// left waiting to be laid on `onto`. A group whose bounds miss the
    /// viewport is dropped, prims and all.
    fn groups(&mut self, onto: Onto, groups: &mut Upcoming<Group>, limit: u32) {
        while let Some(g) = groups.peek().filter(|g| g.start < limit).copied() {
            groups.next();
            self.run(onto, g.start);
            let Some(bounds) = g.bounds.intersect(&self.window) else {
                self.cursor = g.end;
                continue;
            };
            let bounds = bounds.snapped();
            let wipe = self.box_at(Prim::rect(bounds, [0.0; 4]));
            self.passes.push(Pass {
                onto: Onto::Scratch,
                copy: None,
                wipe: Some(wipe),
                lay: None,
                start: g.start,
                end: g.end,
                blend: g.blend,
            });
            let prim = Prim::composite(bounds, self.viewport, self.scratch, g.opacity);
            let prim = self.box_at(prim);
            self.pending = Some(Lay {
                prim,
                blend: g.lands,
            });
            self.cursor = g.end;
        }
    }

    /// Everything up to `end` onto `onto`, which is `depth` sheets deep:
    /// each sheet that begins in it opened a depth further in, drawn, and
    /// laid back. A sheet whose bounds miss the viewport is dropped with
    /// everything in it; one past the last surface there is is drawn
    /// straight onto this one, which is wrong only in how it blends.
    fn span(&mut self, onto: Onto, depth: usize, end: u32, sheets: &mut Upcoming<Sheet>, groups: &mut Upcoming<Group>) {
        while let Some(s) = sheets.peek().filter(|s| s.start < end).copied() {
            sheets.next();
            self.groups(onto, groups, s.start);
            let Some(bounds) = s.bounds.intersect(&self.window) else {
                self.run(onto, s.start);
                while sheets.peek().is_some_and(|n| n.start < s.end) {
                    sheets.next();
                }
                while groups.peek().is_some_and(|g| g.start < s.end) {
                    groups.next();
                }
                self.cursor = s.end;
                continue;
            };
            let Some(&slot) = self.sheets.get(depth) else {
                self.span(onto, depth, s.end, sheets, groups);
                continue;
            };
            self.run(onto, s.start);
            let bounds = bounds.snapped();
            let child = Onto::Sheet(depth as u8 + 1);
            // Opened by the first pass drawn onto it, whatever that is.
            if s.backdrop {
                self.copy = Some(Copy {
                    from: onto,
                    to: child,
                    rect: bounds,
                });
            } else {
                self.wipe = Some(self.box_at(Prim::rect(bounds, [0.0; 4])));
            }
            self.span(child, depth + 1, s.end, sheets, groups);
            let mut prim = Prim::composite(bounds, self.viewport, slot, s.opacity);
            // A mode reads what it is laid on: that is copied out just
            // before the pass that lays it, and the mode rides on the box.
            if let Blend::Mode(mode) = s.lays {
                prim = prim.in_mode(mode);
                self.copy = Some(Copy {
                    from: onto,
                    to: Onto::Backdrop,
                    rect: bounds,
                });
            }
            let prim = self.box_at(prim);
            self.pending = Some(Lay {
                prim,
                blend: s.lays,
            });
        }
        self.groups(onto, groups, end);
        self.run(onto, end);
    }
}

/// The compositing plan for `frame`: its prims with one wipe and one
/// composite box appended per group and per sheet, and the passes to
/// draw them in. `sheets` are the slots of the surfaces the sheets are
/// built on, one a depth from 1. A group or a sheet whose bounds miss the
/// viewport is dropped, prims and all — no pass covers them. Always
/// begins with a pass onto the window, so there is one to clear it with.
pub fn passes(frame: &Frame, viewport: Viewport, scratch: u32, sheets: &[u32]) -> (Vec<Prim>, Vec<Pass>) {
    let window = ScreenRect {
        x: 0.0,
        y: 0.0,
        w: viewport.w as f32,
        h: viewport.h as f32,
    };
    let mut plan = Plan {
        prims: frame.prims.clone(),
        passes: Vec::new(),
        viewport,
        window,
        scratch,
        sheets,
        cursor: 0,
        pending: None,
        wipe: None,
        copy: None,
    };
    let mut groups = frame.groups.iter().peekable();
    let mut upcoming = frame.sheets.iter().peekable();
    plan.span(Onto::Window, 0, frame.prims.len() as u32, &mut upcoming, &mut groups);
    (plan.prims, plan.passes)
}

/// The stroke being drawn, and the layer it is going to land on:
/// painted with that layer's paint so it meets the ink already there
/// the way it will once it is let go of — which is the only way an
/// eraser can show what it is doing, since it rubs out its own layer's
/// sheet and nothing under it.
pub struct Live<'a> {
    pub layer: &'a str,
    pub prims: Vec<Prim>,
    pub tip: &'a Tip,
}

/// A frame's boundary in screen px — the box its contents are cut to.
pub fn frame_rect(f: &crate::doc::Frame, view: &View) -> ScreenRect {
    let (x, y) = view.world_to_screen(f.x, f.y);
    ScreenRect {
        x: x as f32,
        y: y as f32,
        w: (f.w * view.px_per_world()) as f32,
        h: (f.h * view.px_per_world()) as f32,
    }
}

/// Every prim cut to `to`, or left alone when there is nothing to cut it
/// to. Nothing a document paints carries a clip of its own, so this sets
/// rather than intersects; a cut inside a cut would have to intersect.
fn cut_all(prims: Vec<Prim>, to: Option<ScreenRect>) -> Vec<Prim> {
    match to {
        None => prims,
        Some(r) => prims.into_iter().map(|p| p.clipped(r)).collect(),
    }
}

/// Flattens the document into a frame in paint order — the layers'
/// order, then document order within a layer, hidden layers left out.
/// A layer, a group or a frame that asks to be composited as one — below
/// full strength, or a group passing through below it — is built on a
/// surface of its own; nothing else is, so a board that asks for none of
/// it is drawn exactly as it always was. Rects paint fill first, then the
/// four outline edges (constant px thickness, aligned inwards), all
/// turned about the rect center by its rotation; paths become strokes,
/// direct or composited as their tip demands; images become one textured
/// box each, from `images`.
pub fn document_prims(
    doc: &Document,
    view: &View,
    images: &ImageSlots,
    shapes: &Shapes,
    edge: Rgba,
    live: Option<Live>,
) -> Frame {
    // The stroke in progress is cut by the frame its layer is in, on
    // both the paths below: joining its layer's content, and drawn last
    // over everything when it opens a layer that is not there yet.
    // Reading the layer rather than the pointer is what keeps the live
    // ink and the ink it becomes agreeing about which boundary they are
    // under.
    let live_cut = live
        .as_ref()
        .and_then(|l| doc.frame_holding(l.layer))
        .map(|f| frame_rect(f, view));
    let mut on: HashMap<&str, Vec<&Element>> = HashMap::new();
    for el in &doc.elements {
        on.entry(el.layer()).or_default().push(el);
    }
    let walk = Walk {
        doc,
        view,
        images,
        shapes,
        edge,
        on,
        live_cut,
    };
    let mut frame = Frame::new();
    let mut live = live;
    walk.stack(&mut frame, &doc.layers, None, &mut live);
    // A stroke opening a layer that is not there yet has nothing to join:
    // it is painted last, over everything, until it lands. An eraser
    // there has nothing to rub out and paints nothing at all, which is
    // exactly what it will do when it is let go of.
    if let Some(l) = live.filter(|l| !l.tip.erases()) {
        frame.stroke(cut_all(l.prims, live_cut), l.tip);
    }
    frame
}

/// What [`document_prims`] walks the tree with: the board, how it is
/// seen, and what stands on each layer in document order.
struct Walk<'a> {
    doc: &'a Document,
    view: &'a View,
    images: &'a ImageSlots,
    shapes: &'a Shapes,
    edge: Rgba,
    on: HashMap<&'a str, Vec<&'a Element>>,
    live_cut: Option<ScreenRect>,
}

/// Whether anything in `layers` blends with what is under it through
/// them: a layer with a mode, or a group passing through that holds one.
/// A group that isolates keeps its own layers' modes to itself.
fn blends(layers: &[Layer]) -> bool {
    layers.iter().filter(|l| l.visible).any(|l| match l.blend {
        BlendMode::Normal => false,
        BlendMode::PassThrough => blends(&l.layers),
        _ => true,
    })
}

impl Walk<'_> {
    /// `layers` bottom to top onto `frame`, everything cut to `cut` when
    /// they stand in a frame's stack.
    fn stack(&self, frame: &mut Frame, layers: &[Layer], cut: Option<ScreenRect>, live: &mut Option<Live>) {
        for layer in layers.iter().filter(|l| l.visible) {
            let mut draw = |f: &mut Frame| match layer.kind {
                Kind::Group => self.stack(f, &layer.layers, cut, live),
                Kind::Frame => {
                    let Some(fr) = self.doc.frame_on(&layer.id) else {
                        return;
                    };
                    f.extend(self.ground(fr));
                    self.stack(f, &fr.layers, Some(frame_rect(fr, self.view)), live);
                }
                Kind::Raster | Kind::Vector | Kind::Text => self.leaf(f, &layer.id, cut, live),
            };
            match self.composited(layer) {
                Some((opacity, lays, backdrop)) => frame.layer(opacity, lays, backdrop, draw),
                None => draw(frame),
            }
        }
    }

    /// How a layer asks to be composited, when it asks at all: its
    /// strength, how it is laid, and whether it opens on what is under
    /// it. A mode always asks, being read against what is under it. A
    /// group passing through at full strength asks for nothing — its
    /// layers go straight onto what is under it — and below full strength
    /// works on a copy of that and is mixed back. A normal group or frame
    /// asks below full strength, or when what it holds blends: it is what
    /// keeps its layers' modes to themselves.
    fn composited(&self, layer: &Layer) -> Option<(f32, Blend, bool)> {
        let opacity = layer.opacity as f32;
        match (layer.kind, layer.blend) {
            (Kind::Group, BlendMode::PassThrough) => {
                (opacity < 1.0).then_some((opacity, Blend::Mix, true))
            }
            (_, BlendMode::Normal) => {
                let inside = match layer.kind {
                    Kind::Group => blends(&layer.layers),
                    Kind::Frame => self.doc.frame_on(&layer.id).is_some_and(|f| blends(&f.layers)),
                    Kind::Raster | Kind::Vector | Kind::Text => false,
                };
                (opacity < 1.0 || inside).then_some((opacity, Blend::Over, false))
            }
            (_, mode) => Some((opacity, Blend::Mode(mode), false)),
        }
    }

    /// An area: its ground, then a hairline edge. The edge is what makes
    /// an area with no ground visible and hittable at all, and it goes
    /// down before the contents, so ink laid inside covers it as ink does.
    fn ground(&self, f: &crate::doc::Frame) -> Vec<Prim> {
        let r = frame_rect(f, self.view);
        let mut out = Vec::new();
        if let Some(hex) = &f.background {
            out.push(Prim::rect(r, parse_color(hex)));
        }
        let t = STROKE_PX;
        let inner_h = (r.h - 2.0 * t).max(0.0);
        for (x, y, w, h) in [
            (r.x, r.y, r.w, t),
            (r.x, r.y + r.h - t, r.w, t),
            (r.x, r.y + t, t, inner_h),
            (r.x + r.w - t, r.y + t, t, inner_h),
        ] {
            out.push(Prim::rect(ScreenRect { x, y, w, h }, self.edge));
        }
        out
    }

    /// What stands on layer `id`, in document order. The stroke in
    /// progress, when this is the layer it lands on, joins the paint on
    /// top of it — where it meets the ink already there the way it will
    /// at the release, and the only place an eraser has anything to rub —
    /// or, when the top of the layer is not a paint, goes on top of it.
    fn leaf(&self, frame: &mut Frame, id: &str, cut: Option<ScreenRect>, live: &mut Option<Live>) {
        let elements = self.on.get(id).map_or(&[][..], Vec::as_slice);
        let mine = live.as_ref().is_some_and(|l| l.layer == id);
        for (k, element) in elements.iter().enumerate() {
            let last = k + 1 == elements.len();
            match element {
                // A paint is one object made of many strokes: each is
                // drawn with the ink it was laid with, and composited on
                // its own — painting twice over the same place darkens
                // it, as pixels do.
                Element::Paint(p) => {
                    let mut strokes: Vec<(Vec<Prim>, Tip)> = p
                        .strokes
                        .iter()
                        .map(|s| {
                            let tip = Tip::of_stroke(s);
                            let color = parse_color(&s.stroke);
                            let prims = path_prims(&s.curves, &tip, &s.pen, color, self.view, self.shapes);
                            (cut_all(prims, cut), tip)
                        })
                        .collect();
                    if mine
                        && last
                        && let Some(l) = live.take()
                    {
                        strokes.push((cut_all(l.prims, self.live_cut), l.tip.clone()));
                    }
                    // One stroke rubbing the others out is what asks for
                    // a sheet: without one there would be nothing to rub
                    // but the board itself.
                    if strokes.iter().any(|(_, tip)| tip.erases()) {
                        frame.sheet(|f| {
                            for (prims, tip) in strokes {
                                f.stroke(prims, &tip);
                            }
                        });
                    } else {
                        for (prims, tip) in strokes {
                            frame.stroke(prims, &tip);
                        }
                    }
                }
                Element::Path(p) => {
                    let tip = Tip::of(p);
                    let prims = path_prims(&p.curves, &tip, &p.pen, parse_color(&p.stroke), self.view, self.shapes);
                    frame.stroke(cut_all(prims, cut), &tip);
                }
                other => frame.extend(cut_all(self.boxed(other), cut)),
            }
        }
        // The top of the layer is not a paint: the stroke goes on top of
        // it, where the paint it opens at the release will stand. An
        // eraser there has nothing of its own to rub out.
        if mine
            && let Some(l) = live.take_if(|l| !l.tip.erases())
        {
            frame.stroke(cut_all(l.prims, self.live_cut), l.tip);
        }
    }

    /// A rect, an image or a frame: boxes, turned about their centres.
    fn boxed(&self, element: &Element) -> Vec<Prim> {
        let view = self.view;
        let mut out = Vec::new();
        match element {
            Element::Rect(r) => {
                let (sx, sy) = view.world_to_screen(r.x, r.y);
                let (sx, sy) = (sx as f32, sy as f32);
                let sw = (r.w * view.px_per_world()) as f32;
                let sh = (r.h * view.px_per_world()) as f32;
                let pivot = (sx + sw / 2.0, sy + sh / 2.0);
                let angle = r.rotation.to_radians() as f32;

                let fill = r.fill.as_deref().map(parse_color);
                let stroke = r.stroke.as_deref().map(parse_color);

                // An element with no color at all still has to show up.
                let fill = fill.or(if stroke.is_none() {
                    Some(FALLBACK_COLOR)
                } else {
                    None
                });
                if let Some(color) = fill {
                    let r = ScreenRect {
                        x: sx,
                        y: sy,
                        w: sw,
                        h: sh,
                    };
                    out.push(Prim::turned(r, pivot, angle, color));
                }
                if let Some(color) = stroke {
                    let t = STROKE_PX;
                    let inner_h = (sh - 2.0 * t).max(0.0);
                    let edges = [
                        (sx, sy, sw, t),
                        (sx, sy + sh - t, sw, t),
                        (sx, sy + t, t, inner_h),
                        (sx + sw - t, sy + t, t, inner_h),
                    ];
                    for (x, y, w, h) in edges {
                        out.push(Prim::turned(ScreenRect { x, y, w, h }, pivot, angle, color));
                    }
                }
            }
            Element::Image(i) => {
                let (sx, sy) = view.world_to_screen(i.x, i.y);
                let r = ScreenRect {
                    x: sx as f32,
                    y: sy as f32,
                    w: (i.w * view.px_per_world()) as f32,
                    h: (i.h * view.px_per_world()) as f32,
                };
                let pivot = r.center();
                let angle = i.rotation.to_radians() as f32;
                out.push(match self.images.get(&i.blob) {
                    Some(&slot) => Prim::image(r, pivot, angle, slot),
                    // Decoding happens off the frame path; until the
                    // texture lands, the element still occupies its box.
                    None => Prim::turned(r, pivot, angle, PLACEHOLDER_COLOR),
                });
            }
            // A frame on a layer that is not its own frame layer is not a
            // board the parse lets in; a frame is drawn by its layer.
            Element::Frame(_) | Element::Path(_) | Element::Paint(_) | Element::Text(_) => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::{Brush, Dynamics, Tip};
    use crate::doc::{Camera, Kind, Layer, Mark, Paint, Path, Profile, Rect, Scatter, Stroke};

    const VP: Viewport = Viewport { w: 100, h: 100 };

    /// No shapes uploaded: every nib is a plain round one.
    fn no_sheet() -> Shapes {
        Shapes::default()
    }

    /// A stroke drawn with no pen: pressed all the way, end to end.
    fn no_pen() -> Envelope {
        Envelope::default()
    }

    #[test]
    fn a_swept_stroke_thins_with_the_hand_and_stays_true_to_the_taper() {
        let v = view(0.0, 0.0, 1.0);
        let line = [[0.0, 0.0], [400.0, 0.0]];
        let pencil = tip(20.0, 1.0, 1.0);
        // A stroke the pen never touched is one capsule a span, as it
        // always was: the taper costs nothing when there is none.
        let flat = stroke_prims(&line, &pencil, &no_pen(), WHITE, &v, &no_sheet());
        assert_eq!(flat.len(), 1, "one span, one capsule");
        assert_eq!(flat[0].radius, 10.0);

        // Leaned on at the start and let go by the end, it tapers.
        let leaned = pressed(&[1.0, 0.0]);
        let taper = stroke_prims(&line, &pencil, &leaned, WHITE, &v, &no_sheet());
        assert!(taper.len() > 8, "cut into {} capsules", taper.len());
        let radii: Vec<f32> = taper.iter().map(|p| p.radius).collect();
        assert!(
            radii.windows(2).all(|w| w[1] <= w[0]),
            "the width only falls: {radii:?}"
        );
        // Full pressure is the whole width; none of it is what
        // `PENCIL_DRIVE` leaves, and never nothing at all.
        assert!((radii[0] - 10.0).abs() < 0.5, "{}", radii[0]);
        let least = radii[radii.len() - 1];
        assert!(least > NIB_MIN_PX, "{least} is a hairline, not a gap");
        assert!((least - 5.0).abs() < 0.5, "half the width at no pressure: {least}");
        // No step in the outline is wider than the tolerance, so the
        // taper is as true as the flattened line under it.
        assert!(
            radii.windows(2).all(|w| (w[0] - w[1]).abs() <= TAPER_TOLERANCE_PX + 1e-3),
            "{radii:?}"
        );
        // And the capsules still cover the whole line, end to end.
        let (first, last) = (taper[0].geom, taper[taper.len() - 1].geom);
        assert_eq!((first[0], first[1]), (flat[0].geom[0], flat[0].geom[1]));
        assert!((last[2] - flat[0].geom[2]).abs() < 1e-3, "{}", last[2]);
    }

    #[test]
    fn only_the_pencil_sweeps_so_only_it_carries_the_builds_own_drive() {
        // A brush says how much the pen drives it on its own nib, where
        // a slider reaches; the pencil has no sliders, so the build
        // answers for it.
        assert_eq!(Tip::PENCIL.drive(), crate::brush::PENCIL_DRIVE);
        let nib = stamped(20.0, ROUND);
        assert_eq!(nib.drive(), ROUND.pressure);
    }

    #[test]
    fn a_prim_carries_its_clip_and_nothing_else_changes() {
        let r = ScreenRect {
            x: 10.0,
            y: 20.0,
            w: 40.0,
            h: 10.0,
        };
        let band = ScreenRect {
            x: 0.0,
            y: 22.0,
            w: 100.0,
            h: 4.0,
        };
        let plain = Prim::rounded(r, 4.0, [1.0; 4]);
        assert_eq!(plain.clip, NO_CLIP, "nothing is cut until it is put somewhere");
        let cut = plain.clipped(band);
        assert_eq!(cut.clip, [band.x, band.y, band.w, band.h]);
        assert_eq!(Prim { clip: NO_CLIP, ..cut }, plain, "only the clip differs");
        // The cut travels with the prim's own geometry untouched, so a
        // clipped prim still measures as the whole thing.
        assert_eq!(cut.bounds(), plain.bounds());
        // The instance layout the shader is fed mirrors the struct.
        assert_eq!(std::mem::size_of::<Prim>(), 120);
    }

    #[test]
    fn transformed_grows_turns_and_moves_a_box_and_a_segment() {
        let r = ScreenRect {
            x: 10.0,
            y: 20.0,
            w: 40.0,
            h: 10.0,
        };
        let pivot = r.center();

        // Grown about its own center: the center holds, the size does not.
        let box_ = Prim::rounded(r, 4.0, [1.0; 4]).transformed(pivot, 2.0, 0.0, (0.0, 0.0));
        assert_eq!(box_.geom[2], 2.0 * r.w);
        assert_eq!(box_.geom[3], 2.0 * r.h);
        assert_eq!(box_.bounds().center(), pivot, "it grows where it stands");
        assert_eq!(box_.radius, 8.0, "the corner grows with the box");

        // A quarter turn clockwise about the pivot, then a shift. The
        // box keeps its geometry and carries the angle to the shader.
        let turned = Prim::rounded(r, 4.0, [1.0; 4]).transformed(
            pivot,
            1.0,
            std::f32::consts::FRAC_PI_2,
            (5.0, -5.0),
        );
        let (cx, cy) = (
            turned.geom[0] + turned.geom[2] / 2.0,
            turned.geom[1] + turned.geom[3] / 2.0,
        );
        assert!((cx - (pivot.0 + 5.0)).abs() < 1e-4, "the pivot only shifts");
        assert!((cy - (pivot.1 - 5.0)).abs() < 1e-4);
        assert_eq!(turned.angle, std::f32::consts::FRAC_PI_2);
        assert_eq!(turned.geom[2], 40.0, "the box is turned, not reshaped");

        // A segment has no angle to carry: its endpoints travel instead.
        let seg = Prim::segment((0.0, 0.0), (10.0, 0.0), 2.0, [1.0; 4]).transformed(
            (0.0, 0.0),
            3.0,
            std::f32::consts::FRAC_PI_2,
            (1.0, 1.0),
        );
        assert_eq!(seg.angle, 0.0);
        assert_eq!(seg.radius, 6.0);
        assert!((seg.geom[0] - 1.0).abs() < 1e-4);
        assert!((seg.geom[1] - 1.0).abs() < 1e-4);
        assert!((seg.geom[2] - 1.0).abs() < 1e-4, "right became down");
        assert!((seg.geom[3] - 31.0).abs() < 1e-4);
    }

    fn tip(width: f64, opacity: f64, hardness: f64) -> Tip {
        Tip {
            width,
            opacity,
            hardness,
            dynamics: Dynamics::None,
            stamp: None,
        }
    }

    fn path_with(tip: Tip) -> Element {
        Element::Path(Path {
            id: "p".into(),
            layer: String::new(),
            curves: vec![[[0.0, 0.0], [3.0, 0.0], [6.0, 0.0], [9.0, 0.0]]],
            stroke: "#000".into(),
            width: tip.width,
            opacity: tip.opacity,
            hardness: tip.hardness,
            rotation: 0.0,
            stamp: tip.stamp,
            pen: Envelope::default(),
        })
    }

    #[test]
    fn soft_radius_keeps_the_stroke_inside_its_width() {
        // Width 8 at zoom 1 is a radius of 4 px. Hardness 1 is the crisp
        // edge; hardness 0 spends the whole radius on the ramp, so the
        // geometry shrinks to half and the ramp ends where the edge was.
        let v = view(0.0, 0.0, 1.0);
        assert_eq!(soft_radius(8.0, 1.0, &v), (4.0, 0.0));
        assert_eq!(soft_radius(8.0, 0.0, &v), (2.0, 4.0));
        assert_eq!(soft_radius(8.0, 0.5, &v), (3.0, 2.0));
        // The one-pixel floor still applies to the nominal radius.
        assert_eq!(soft_radius(2.0, 0.0, &view(0.0, 0.0, 0.1)), (0.25, 0.5));
    }

    #[test]
    fn soft_stroke_prims_carry_the_feather() {
        let v = view(0.0, 0.0, 1.0);
        let got = stroke_prims(&[[0.0, 0.0], [10.0, 0.0]], &tip(8.0, 1.0, 0.5), &no_pen(), WHITE, &v, &no_sheet());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, KIND_SEGMENT);
        assert_eq!((got[0].radius, got[0].feather), (3.0, 2.0));
        // A tap is a dot, and a dot can be soft too.
        let dot = stroke_prims(&[[0.0, 0.0]], &tip(8.0, 1.0, 0.0), &no_pen(), WHITE, &v, &no_sheet());
        assert_eq!(dot.len(), 1);
        assert_eq!(dot[0].kind, KIND_BOX);
        assert_eq!((dot[0].radius, dot[0].feather), (2.0, 4.0));
        assert_eq!(dot[0].geom, [48.0, 48.0, 4.0, 4.0]);
    }

    /// A round nib, dabbed half a width apart.
    const ROUND: Stamp = Stamp {
        shape: None,
        grain: None,
        paper: None,
        follow: false,
        spacing: 0.5,
        roundness: 1.0,
        rotation: 0.0,
        profile: Profile::RegularSolid,
        mark: Mark::Ink,
        flow: 1.0,
        scatter: Scatter {
            size: 0.0,
            rotation: 0.0,
        },
        pressure: Pressure::NONE,
    };

    fn stamped(width: f64, stamp: Stamp) -> Tip {
        Tip {
            width,
            opacity: 1.0,
            hardness: 1.0,
            dynamics: Dynamics::None,
            stamp: Some(stamp),
        }
    }

    /// The pen pressed `readings` hard along the stroke, evenly spaced.
    fn pressed(readings: &[f32]) -> Envelope {
        Envelope {
            pressure: readings.to_vec(),
            twist: Vec::new(),
        }
    }

    /// A sheet of one nib, called `name`.
    fn one_nib(name: &str) -> Shapes {
        Shapes {
            slot: 5,
            cols: 1,
            rows: 1,
            px: 128,
            cells: std::iter::once((name.to_owned(), 0)).collect(),
            paper_px: 0,
            papers: std::collections::HashMap::new(),
        }
    }

    /// A sheet of one nib and one paper, the paper cell as wide as the
    /// whole nib row so that the two bands sit one under the other.
    fn one_paper(name: &str) -> Shapes {
        Shapes {
            slot: 5,
            cols: 2,
            rows: 1,
            px: 100,
            cells: std::collections::HashMap::new(),
            paper_px: 200,
            papers: std::iter::once((name.to_owned(), 0)).collect(),
        }
    }

    #[test]
    fn a_grain_is_worn_over_a_round_dab_and_a_shape_stands_in_for_one() {
        let v = view(0.0, 0.0, 1.0);
        let sheet = one_nib("bristle");
        let grained = stamped(
            20.0,
            Stamp {
                grain: Some("bristle".into()),
                profile: Profile::Airbrush,
                ..ROUND
            },
        );
        let got = stroke_prims(&[[0.0, 0.0]], &grained, &no_pen(), WHITE, &v, &sheet);
        let dab = got[0];
        assert_eq!(dab.kind, KIND_GRAIN);
        assert_eq!(dab.slot, 5);
        assert_eq!(dab.uv, [0.0, 0.0, 1.0, 1.0], "its own cell of the sheet");
        // It keeps everything a round dab has: the corner, the ramp and
        // the profile's bend. Only its coverage is eaten into.
        let plain = stroke_prims(&[[0.0, 0.0]], &stamped(20.0, ROUND), &no_pen(), WHITE, &v, &no_sheet());
        assert_eq!((dab.geom, dab.radius), (plain[0].geom, plain[0].radius));
        assert_eq!(dab.feather, plain[0].feather);
        assert_eq!(dab.falloff, Profile::Airbrush.falloff());

        // A shape is the dab: square corners, no ramp, no bend, since
        // its own alpha is the only edge there is.
        let shaped = stamped(
            20.0,
            Stamp {
                shape: Some("bristle".into()),
                profile: Profile::Airbrush,
                ..ROUND
            },
        );
        let got = stroke_prims(&[[0.0, 0.0]], &shaped, &no_pen(), WHITE, &v, &sheet);
        assert_eq!(got[0].kind, KIND_IMAGE);
        assert_eq!((got[0].radius, got[0].feather, got[0].falloff), (0.0, 0.0, 1.0));
    }

    /// A paper `period` world units to the tile, biting by `depth`.
    fn dragged(width: f64, stamp: Stamp, period: f64, depth: f64) -> Tip {
        stamped(
            width,
            Stamp {
                paper: Some(Paper {
                    name: "canvas".into(),
                    period,
                    depth,
                }),
                ..stamp
            },
        )
    }

    #[test]
    fn every_kind_of_dab_wears_the_paper_over_whatever_else_it_is() {
        let v = view(0.0, 0.0, 1.0);
        let sheet = Shapes {
            cells: std::iter::once(("bristle".to_owned(), 0u16)).collect(),
            ..one_paper("canvas")
        };
        let cell = sheet.paper("canvas").expect("the sheet carries it");
        for nib in [
            ROUND,
            Stamp {
                grain: Some("bristle".into()),
                ..ROUND
            },
            Stamp {
                shape: Some("bristle".into()),
                ..ROUND
            },
        ] {
            let kind = format!("{:?}/{:?}", nib.shape, nib.grain);
            let tip = dragged(20.0, nib, 400.0, 0.75);
            let got = stroke_prims(&[[0.0, 0.0]], &tip, &no_pen(), WHITE, &v, &sheet);
            let dab = got[0];
            assert_eq!(dab.paper, cell.uv, "{kind} misses the paper's cell");
            assert_eq!(dab.weave[3], 0.75, "{kind} misses its depth");
            assert_eq!(dab.weave[0], 1.0 / 400.0, "{kind} misses its scale");
            // Whatever the dab is, it samples the paper off the one
            // sheet the nibs are on.
            assert_eq!(dab.slot, sheet.slot, "{kind}");
        }
    }

    #[test]
    fn the_paper_belongs_to_the_board_and_not_to_the_stroke() {
        let v = view(0.0, 0.0, 1.0);
        let sheet = one_paper("canvas");
        let tip = dragged(20.0, ROUND, 400.0, 1.0);
        let weave = |at: [f64; 2], v: &View| {
            stroke_prims(&[at], &tip, &no_pen(), WHITE, v, &sheet)[0].weave
        };
        // Two strokes a long way apart lie on the same sheet of paper:
        // where it is anchored has nothing to do with where either one
        // began, so both read it the same way.
        assert_eq!(weave([0.0, 0.0], &v), weave([137.0, -409.0], &v));
        // The board carries the paper, so panning moves it under the
        // window, and zooming stretches a tile with the ink.
        assert_ne!(weave([0.0, 0.0], &view(50.0, 0.0, 1.0)), weave([0.0, 0.0], &v));
        assert_eq!(weave([0.0, 0.0], &view(0.0, 0.0, 2.0))[0], 1.0 / 800.0);
        // However far the camera is from world zero, the offset it
        // hands the shader is inside one tile: `fract` of a big number
        // has no precision left, so the wrapping is done here.
        let far = weave([0.0, 0.0], &view(1.0e7, -1.0e7, 1.0));
        for k in [far[1], far[2]] {
            assert!((0.0..1.0).contains(&k), "{k} is not inside a tile");
        }
    }

    #[test]
    fn a_paper_the_sheet_does_not_carry_leaves_the_ink_undivided() {
        let v = view(0.0, 0.0, 1.0);
        let tip = dragged(20.0, ROUND, 400.0, 1.0);
        // A board painted on a build that had this paper still opens,
        // and its ink is laid whole rather than not at all.
        let got = stroke_prims(&[[0.0, 0.0]], &tip, &no_pen(), WHITE, &v, &one_paper("other"));
        assert_eq!(got[0].weave[3], 0.0, "no paper bites");
        assert_eq!(got[0].paper, [0.0; 4]);
        // And so does one whose brush was turned down to no depth.
        let none = dragged(20.0, ROUND, 400.0, 0.0);
        let got = stroke_prims(&[[0.0, 0.0]], &none, &no_pen(), WHITE, &v, &one_paper("canvas"));
        assert_eq!(got[0].weave[3], 0.0);
    }

    #[test]
    fn a_tile_thinner_than_the_pixels_reading_it_stops_getting_finer() {
        let sheet = one_paper("canvas");
        let tip = dragged(20.0, ROUND, 400.0, 1.0);
        let scale = |zoom: f64| {
            let v = view(0.0, 0.0, zoom);
            stroke_prims(&[[0.0, 0.0]], &tip, &no_pen(), WHITE, &v, &sheet)[0].weave[0]
        };
        // Zoomed far out the weave is finer than the screen can read,
        // and a paper that went on getting finer would be noise that
        // changed every time the board moved.
        assert_eq!(scale(0.0001), (1.0 / PAPER_MIN_PX) as f32);
        assert!(scale(1.0) < scale(0.0001));
    }

    #[test]
    fn a_papers_cell_is_drawn_in_half_a_texel_and_a_nibs_is_not() {
        // Four nib cells of 64 across and four rows of them, then a
        // band of 128-texel papers two across and two down under that:
        // 256 by 512 texels in all.
        let sheet = Shapes {
            slot: 3,
            cols: 4,
            rows: 4,
            px: 64,
            cells: std::iter::once(("nib".to_owned(), 5u16)).collect(),
            paper_px: 128,
            papers: ["a", "b", "c", "d"]
                .into_iter()
                .enumerate()
                .map(|(i, n)| (n.to_owned(), i as u16))
                .collect(),
        };
        // A nib's cell reaches its own edges: it is mapped onto the dab
        // and never sampled past them.
        let nib = sheet.cell("nib").expect("the nib is on it");
        assert_eq!(nib.uv, [0.25, 0.125, 0.5, 0.25]);
        // A paper's is drawn in half a texel, because it is sampled by
        // wrapping: the seam has to come back to this paper and not to
        // the cell beside it on the sheet.
        let (du, dv) = (0.5 / 256.0, 0.5 / 512.0);
        assert_eq!(
            sheet.paper("a").expect("the paper is on it").uv,
            [du, 0.5 + dv, 0.5 - du, 0.75 - dv]
        );
        assert_eq!(
            sheet.paper("d").expect("the last of them").uv,
            [0.5 + du, 0.75 + dv, 1.0 - du, 1.0 - dv]
        );
        assert_eq!(sheet.paper("e"), None, "one the sheet does not carry");
        assert_eq!(sheet.paper("a").unwrap().slot, sheet.slot, "one sheet, one slot");
    }

    #[test]
    fn a_nib_the_sheet_does_not_carry_lays_a_plain_round_one() {
        let v = view(0.0, 0.0, 1.0);
        let sheet = one_nib("bristle");
        for lost in [
            Stamp {
                grain: Some("gone".into()),
                ..ROUND
            },
            Stamp {
                shape: Some("gone".into()),
                ..ROUND
            },
        ] {
            let got = stroke_prims(&[[0.0, 0.0]], &stamped(20.0, lost), &no_pen(), WHITE, &v, &sheet);
            assert_eq!(got[0].kind, KIND_BOX, "a board painted elsewhere still opens");
        }
    }

    #[test]
    fn a_grained_run_binds_its_sheet_like_any_other_textured_prim() {
        let v = view(0.0, 0.0, 1.0);
        let sheet = one_nib("bristle");
        let grained = stamped(
            20.0,
            Stamp {
                grain: Some("bristle".into()),
                ..ROUND
            },
        );
        let got = stroke_prims(&[[-20.0, 0.0], [20.0, 0.0]], &grained, &no_pen(), WHITE, &v, &sheet);
        let runs = runs(&got);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].slot, 5, "the dabs are drawn against their sheet");
    }

    #[test]
    fn a_dab_carries_the_bend_its_profile_asks_for() {
        let v = view(0.0, 0.0, 1.0);
        let soft = stamped(
            20.0,
            Stamp {
                profile: Profile::Airbrush,
                ..ROUND
            },
        );
        let got = stroke_prims(&[[0.0, 0.0]], &soft, &no_pen(), WHITE, &v, &no_sheet());
        assert_eq!(got[0].falloff, Profile::Airbrush.falloff());
        // Everything that is not a nib ramps straight across.
        let plain = stroke_prims(&[[0.0, 0.0]], &stamped(20.0, ROUND), &no_pen(), WHITE, &v, &no_sheet());
        assert_eq!(plain[0].falloff, 1.0);
        assert_eq!(Prim::rect(plain[0].bounds(), WHITE).falloff, 1.0);
        let swept = stroke_prims(&[[0.0, 0.0], [20.0, 0.0]], &tip(8.0, 1.0, 0.5), &no_pen(), WHITE, &v, &no_sheet());
        assert_eq!(swept[0].falloff, 1.0, "a swept stroke has no nib to bend");
    }

    #[test]
    fn a_lighter_touch_lays_a_narrower_dab() {
        let v = view(0.0, 0.0, 1.0);
        // A nib the pen drives all of: full press is the brush's own
        // width, no press at all is nothing.
        let nib = stamped(
            40.0,
            Stamp {
                pressure: Pressure {
                    size: 1.0,
                    ..Pressure::NONE
                },
                ..ROUND
            },
        );
        let hard = stroke_prims(&[[-30.0, 0.0], [30.0, 0.0]], &nib, &pressed(&[1.0, 1.0]), WHITE, &v, &no_sheet());
        let half = stroke_prims(&[[-30.0, 0.0], [30.0, 0.0]], &nib, &pressed(&[0.5, 0.5]), WHITE, &v, &no_sheet());
        assert!((hard[0].radius - 20.0).abs() < 1e-3, "the brush's own width");
        assert!(
            (half[0].radius - 10.0).abs() < 1e-3,
            "half the press is half the nib: {}",
            half[0].radius
        );
        // The gap is a share of the nib's width, so a thinner nib steps
        // shorter and the ink stays as solid as it was.
        assert!(
            half.len() > hard.len(),
            "a narrower nib is stamped more often: {} vs {}",
            half.len(),
            hard.len()
        );
    }

    #[test]
    fn a_stroke_thins_as_the_hand_lifts() {
        let v = view(0.0, 0.0, 1.0);
        let nib = stamped(
            40.0,
            Stamp {
                pressure: Pressure {
                    size: 1.0,
                    ..Pressure::NONE
                },
                ..ROUND
            },
        );
        // Pressed all the way at one end and let go at the other.
        let got = stroke_prims(&[[-50.0, 0.0], [50.0, 0.0]], &nib, &pressed(&[1.0, 0.0]), WHITE, &v, &no_sheet());
        let first = got[0].radius;
        let last = got[got.len() - 1].radius;
        assert!((first - 20.0).abs() < 1e-3, "it starts at the full width");
        assert!(last < 1.0, "and comes to nothing: {last}");
        for pair in got.windows(2) {
            assert!(
                pair[1].radius <= pair[0].radius + 1e-4,
                "the nib only ever narrows: {} then {}",
                pair[0].radius,
                pair[1].radius
            );
        }
    }

    #[test]
    fn a_lighter_touch_lays_less_ink() {
        let v = view(0.0, 0.0, 1.0);
        let nib = stamped(
            20.0,
            Stamp {
                flow: 0.5,
                pressure: Pressure {
                    size: 0.0,
                    opacity: 0.0,
                    flow: 1.0,
                },
                ..ROUND
            },
        );
        let hard = stroke_prims(&[[0.0, 0.0]], &nib, &pressed(&[1.0]), WHITE, &v, &no_sheet());
        let soft = stroke_prims(&[[0.0, 0.0]], &nib, &pressed(&[0.25]), WHITE, &v, &no_sheet());
        assert!((hard[0].color[3] - 0.5).abs() < 1e-4, "the brush's own flow");
        assert!(
            (soft[0].color[3] - 0.125).abs() < 1e-4,
            "a quarter of the press lays a quarter of it: {}",
            soft[0].color[3]
        );
        assert_eq!(hard[0].radius, soft[0].radius, "the width is not driven");
    }

    #[test]
    fn a_nib_the_stylus_leans_turns_dab_by_dab() {
        let v = view(0.0, 0.0, 1.0);
        let nib = stamped(8.0, ROUND);
        let leaning = Envelope {
            pressure: Vec::new(),
            twist: vec![0.0, 90.0],
        };
        let got = stroke_prims(&[[-20.0, 0.0], [20.0, 0.0]], &nib, &leaning, WHITE, &v, &no_sheet());
        assert!(got[0].angle.abs() < 1e-4, "it starts where the brush put it");
        let last = got[got.len() - 1].angle;
        assert!(
            (last - std::f32::consts::FRAC_PI_2).abs() < 1e-3,
            "and ends a quarter turn round: {last}"
        );
    }

    #[test]
    fn a_stroke_with_no_pen_is_stamped_as_it_always_was() {
        let v = view(0.0, 0.0, 1.0);
        // The nib says the pen drives all of it; the stroke says no pen
        // ever touched it, so it lays as if none did.
        let driven = stamped(
            8.0,
            Stamp {
                pressure: Pressure {
                    size: 1.0,
                    opacity: 1.0,
                    flow: 1.0,
                },
                ..ROUND
            },
        );
        let pts = [[-20.0, 0.0], [20.0, 0.0]];
        let got = stroke_prims(&pts, &driven, &no_pen(), WHITE, &v, &no_sheet());
        let plain = stroke_prims(&pts, &stamped(8.0, ROUND), &no_pen(), WHITE, &v, &no_sheet());
        assert_eq!(got, plain, "no readings is a full press the whole way");
    }

    #[test]
    fn a_stamped_tip_lays_a_nib_every_spacing_instead_of_sweeping() {
        let v = view(0.0, 0.0, 1.0);
        // A 40-px line with an 8-wide nib at a spacing of 0.5 — an
        // eighth of its width — so a dab where the press was and one
        // every 1 px along it.
        let got = stroke_prims(&[[-20.0, 0.0], [20.0, 0.0]], &stamped(8.0, ROUND), &no_pen(), WHITE, &v, &no_sheet());
        assert_eq!(got.len(), 41, "40 px at a 1 px step, the ends counted");
        for p in &got {
            assert_eq!(p.kind, KIND_BOX, "a dab is a nib, not a swept segment");
            assert_eq!((p.radius, p.feather), (4.0, 0.0));
            assert_eq!([p.geom[2], p.geom[3]], [8.0, 8.0], "as wide as the brush, and round");
        }
        let xs: Vec<f32> = got.iter().map(|p| p.geom[0] + p.geom[2] / 2.0).collect();
        assert_eq!(xs.first(), Some(&30.0), "the first dab is at the press");
        assert_eq!(xs.last(), Some(&70.0), "the last one at the release");
        for w in xs.windows(2) {
            assert!((w[1] - w[0] - 1.0).abs() < 1e-4, "evenly spaced: {xs:?}");
        }
        for p in &got {
            assert!((p.geom[1] + p.geom[3] / 2.0 - 50.0).abs() < 1e-4, "on the line");
        }
    }

    #[test]
    fn the_pencil_sketchbook_documents_lays_a_solid_stroke() {
        // Sketchbook's own help names 1.2 as the Pencil's default
        // spacing, 0.1 as "a very dense brush" and 10.0 as the top. A
        // default pencil has to come out solid, and that is the anchor
        // that fixes what one unit of spacing is worth.
        let v = view(0.0, 0.0, 1.0);
        let nib = Stamp {
            spacing: Brush::default().spacing,
            ..ROUND
        };
        let got = stroke_prims(
            &[[-60.0, 0.0], [60.0, 0.0]],
            &stamped(40.0, nib),
            &no_pen(),
            WHITE,
            &v,
            &no_sheet(),
        );
        let r = got[0].radius;
        let xs: Vec<f32> = got.iter().map(|d| d.geom[0] + d.geom[2] / 2.0).collect();
        let waist = (r * r - ((xs[1] - xs[0]) / 2.0).powi(2)).sqrt();
        assert!(
            waist > 0.95 * r,
            "the pencil pinches to {:.0}% of its width between dabs",
            waist / r * 100.0
        );
    }

    #[test]
    fn a_round_nib_at_the_commonest_spacing_lays_a_solid_stroke() {
        // Half a radius is the gap most of the shipped brushes name,
        // and a plain round brush at it has to come out solid. What
        // gives a beaded stroke away is the waist: the width the ink
        // pinches to between two dabs.
        let v = view(0.0, 0.0, 1.0);
        let got = stroke_prims(
            &[[-60.0, 0.0], [60.0, 0.0]],
            &stamped(40.0, ROUND),
            &no_pen(),
            WHITE,
            &v,
            &no_sheet(),
        );
        let r = got[0].radius;
        let xs: Vec<f32> = got.iter().map(|d| d.geom[0] + d.geom[2] / 2.0).collect();
        let step = xs[1] - xs[0];
        let waist = (r * r - (step / 2.0).powi(2)).sqrt();
        assert!(
            waist > 0.95 * r,
            "the stroke pinches to {:.0}% of its width between dabs",
            waist / r * 100.0
        );
    }

    #[test]
    fn a_flattened_nib_keeps_its_width_and_is_turned_by_its_own_angle() {
        let v = view(0.0, 0.0, 1.0);
        let flat = Stamp {
            spacing: 0.5,
            roundness: 0.25,
            rotation: 90.0,
            profile: Profile::RegularSolid,
            mark: Mark::Ink,
            flow: 1.0,
            ..ROUND
        };
        let got = stroke_prims(&[[0.0, 0.0]], &stamped(8.0, flat.clone()), &no_pen(), WHITE, &v, &no_sheet());
        assert_eq!(got.len(), 1, "a tap is one dab");
        let d = got[0];
        assert_eq!([d.geom[2], d.geom[3]], [8.0, 2.0], "squished across its own y");
        assert_eq!(d.radius, 1.0, "the cap is the smaller of the two halves");
        assert!((d.angle - std::f32::consts::FRAC_PI_2).abs() < 1e-6, "a quarter turn");
        assert_eq!(d.bounds().center(), (50.0, 50.0), "turned about itself");

        // However flat it is squished, a nib still marks the paper.
        let hair = Stamp {
            roundness: 0.0,
            ..flat.clone()
        };
        let got = stroke_prims(&[[0.0, 0.0]], &stamped(8.0, hair), &no_pen(), WHITE, &v, &no_sheet());
        assert_eq!(got[0].geom[3], 1.0, "a nib is never let vanish");
    }

    fn path_of(curves: Vec<Cubic>, tip: Tip) -> Element {
        Element::Path(Path {
            id: "p".into(),
            layer: String::new(),
            curves,
            stroke: "#000".into(),
            width: tip.width,
            opacity: tip.opacity,
            hardness: tip.hardness,
            rotation: 0.0,
            stamp: tip.stamp,
            pen: Envelope::default(),
        })
    }

    /// One curve, ten world units long.
    fn cubic() -> Cubic {
        [[0.0, 0.0], [3.0, 0.0], [6.0, 0.0], [9.0, 0.0]]
    }

    /// A paint on the one layer a test document has.
    fn paint_of(strokes: Vec<Stroke>) -> Element {
        Element::Paint(Paint {
            id: "pt".into(),
            layer: String::new(),
            strokes,
            rotation: 0.0,
        })
    }

    fn laid(curves: Vec<Cubic>, tip: Tip) -> Stroke {
        Stroke {
            curves,
            stroke: "#000".into(),
            width: tip.width,
            opacity: tip.opacity,
            hardness: tip.hardness,
            stamp: tip.stamp,
            pen: Envelope::default(),
        }
    }

    #[test]
    fn a_paint_draws_every_stroke_with_the_ink_it_was_laid_with() {
        let v = view(0.0, 0.0, 1.0);
        let none = ImageSlots::new();
        let a: Vec<Cubic> = vec![[[0.0, 0.0], [3.0, 0.0], [6.0, 0.0], [9.0, 0.0]]];
        let b: Vec<Cubic> = vec![[[0.0, 40.0], [3.0, 40.0], [6.0, 40.0], [9.0, 40.0]]];
        let soft = tip(8.0, 0.5, 0.5);
        let paint = Element::Paint(Paint {
            id: "pt".into(),
            layer: String::new(),
            strokes: vec![laid(a.clone(), Tip::PENCIL), laid(b.clone(), soft.clone())],
            rotation: 0.0,
        });
        let together = document_prims(&doc_with(vec![paint], &v), &v, &none, &no_sheet(), EDGE, None);
        // Two strokes in one paint draw what two paths draw: nothing
        // joins them, and the soft one is still composited on its own.
        let apart = document_prims(
            &doc_with(vec![path_of(a, Tip::PENCIL), path_of(b, soft)], &v),
            &v,
            &none,
            &no_sheet(),
            EDGE,
            None,
        );
        assert_eq!(together.prims, apart.prims);
        assert_eq!(together.groups.len(), 1, "one group, for the soft stroke");
        assert_eq!(together.groups[0].opacity, 0.5);
    }

    #[test]
    fn a_hard_opaque_path_is_direct_and_a_soft_one_is_a_group() {
        let v = view(0.0, 0.0, 1.0);
        let none = ImageSlots::new();
        let direct = document_prims(&doc_with(vec![path_with(Tip::PENCIL)], &v), &v, &none, &no_sheet(), EDGE, None);
        assert_eq!(direct.prims.len(), 1);
        assert!(direct.groups.is_empty(), "the pencil needs no compositing");

        let soft = document_prims(
            &doc_with(vec![path_with(tip(8.0, 1.0, 0.5))], &v),
            &v,
            &none,
            &no_sheet(),
            EDGE,
            None,
        );
        assert_eq!(soft.prims.len(), 1);
        assert_eq!(soft.groups.len(), 1);
        assert_eq!((soft.groups[0].start, soft.groups[0].end), (0, 1));
        assert_eq!(soft.groups[0].opacity, 1.0);

        let faint = document_prims(
            &doc_with(vec![path_with(tip(2.0, 0.5, 1.0))], &v),
            &v,
            &none,
            &no_sheet(),
            EDGE,
            None,
        );
        assert_eq!(faint.groups.len(), 1);
        assert_eq!(faint.groups[0].opacity, 0.5);
    }

    #[test]
    fn group_bounds_wrap_the_prims_and_their_ramp() {
        let mut f = Frame::new();
        f.extend([Prim::rect(sr(0.0, 0.0, 1.0, 1.0), WHITE)]);
        // A segment 10 long, 2 half-wide, with a 4 px ramp: the shader
        // rasterizes max(feather, 1) / 2 + 1 = 3 px past the geometry.
        f.group(
            vec![
                Prim::soft_segment((0.0, 0.0), (10.0, 0.0), 2.0, 4.0, WHITE),
                Prim::soft_segment((10.0, 0.0), (10.0, 5.0), 2.0, 4.0, WHITE),
            ],
            0.5,
            Blend::Union,
            Blend::Over,
        );
        assert_eq!(f.prims.len(), 3);
        assert_eq!(f.groups.len(), 1);
        let g = &f.groups[0];
        assert_eq!((g.start, g.end, g.opacity), (1, 3, 0.5));
        assert_eq!(g.bounds, sr(-5.0, -5.0, 20.0, 15.0));
        // Nothing to draw makes no group.
        f.group(vec![], 0.5, Blend::Union, Blend::Over);
        assert_eq!(f.groups.len(), 1);
    }

    #[test]
    fn frame_stroke_goes_direct_or_grouped_by_the_tip() {
        let prims = vec![Prim::segment((0.0, 0.0), (1.0, 0.0), 1.0, WHITE)];
        let mut f = Frame::new();
        f.stroke(prims.clone(), &Tip::PENCIL);
        assert!(f.groups.is_empty());
        f.stroke(prims, &tip(2.0, 0.25, 1.0));
        assert_eq!(f.groups.len(), 1);
        assert_eq!((f.groups[0].start, f.groups[0].end), (1, 2));
        assert_eq!(f.groups[0].opacity, 0.25);
    }

    #[test]
    fn a_nib_that_follows_the_stroke_turns_with_it() {
        use std::f32::consts::FRAC_PI_2;
        let v = view(0.0, 0.0, 1.0);
        // Right, then down: the nib turns a quarter at the corner.
        let corner = [[-20.0, -20.0], [20.0, -20.0], [20.0, 20.0]];
        let flat = Stamp {
            roundness: 0.3,
            ..ROUND
        };
        let nib = stamped(
            8.0,
            Stamp {
                follow: true,
                ..flat.clone()
            },
        );
        let got = stroke_prims(&corner, &nib, &no_pen(), WHITE, &v, &no_sheet());
        assert!(got.len() > 4);
        assert!(
            got[0].angle.abs() < 1e-4,
            "the first dab lies along the span it starts on"
        );
        assert!(
            (got.last().unwrap().angle - FRAC_PI_2).abs() < 1e-4,
            "and the last one down the span it ends on"
        );

        // Its own angle is added to the heading, never replaced by it.
        let turned = stamped(
            8.0,
            Stamp {
                follow: true,
                rotation: 90.0,
                ..flat.clone()
            },
        );
        let got = stroke_prims(&corner, &turned, &no_pen(), WHITE, &v, &no_sheet());
        assert!((got[0].angle - FRAC_PI_2).abs() < 1e-4);

        // A nib that does not follow keeps its angle, whatever the
        // stroke does around it.
        let fixed = stamped(8.0, flat);
        let got = stroke_prims(&corner, &fixed, &no_pen(), WHITE, &v, &no_sheet());
        assert!(got.iter().all(|d| d.angle == 0.0), "the nib stands still");
    }

    #[test]
    fn a_nib_with_a_shape_stamps_its_cell_of_the_sheet() {
        let v = view(0.0, 0.0, 1.0);
        let sheet = Shapes {
            slot: 7,
            cols: 4,
            rows: 2,
            px: 128,
            cells: [("bristle".to_owned(), 5u16)].into_iter().collect(),
            paper_px: 0,
            papers: std::collections::HashMap::new(),
        };
        let nib = stamped(
            8.0,
            Stamp {
                shape: Some("bristle".to_owned()),
                grain: None,
                ..ROUND
            },
        );
        let got = stroke_prims(&[[0.0, 0.0]], &nib, &no_pen(), WHITE, &v, &sheet);
        assert_eq!(got.len(), 1, "a tap is one dab");
        let d = got[0];
        assert_eq!(d.kind, KIND_IMAGE, "the nib is the sheet's own art");
        assert_eq!(d.slot, 7);
        assert_eq!(d.uv, [0.25, 0.5, 0.5, 1.0], "cell 5 of a sheet 4 across, 2 down");
        assert_eq!([d.geom[2], d.geom[3]], [8.0, 8.0], "as wide as the brush");
        assert_eq!((d.radius, d.feather), (0.0, 0.0), "the shape is its own edge");
        assert_eq!(d.color, WHITE, "and the ink tints it, as a glyph is tinted");

        // A shape this build does not carry lays a plain round nib. A
        // board painted on another one must still open.
        let lost = stamped(
            8.0,
            Stamp {
                shape: Some("gone".to_owned()),
                grain: None,
                ..ROUND
            },
        );
        let got = stroke_prims(&[[0.0, 0.0]], &lost, &no_pen(), WHITE, &v, &sheet);
        assert_eq!(got[0].kind, KIND_BOX);
    }

    #[test]
    fn a_scattered_nib_throws_every_dab_off_true_and_lands_there_again() {
        let v = view(0.0, 0.0, 1.0);
        let wild = Stamp {
            scatter: Scatter {
                size: 3.0,
                rotation: 45.0,
            },
            ..ROUND
        };
        let pts = [[-40.0, 0.0], [40.0, 0.0]];
        let got = stroke_prims(&pts, &stamped(8.0, wild.clone()), &no_pen(), WHITE, &v, &no_sheet());
        assert!(got.len() > 4);

        // No two dabs alike: the radius, the angle and the gap are all
        // thrown off what the nib says.
        assert!(
            got.windows(2).any(|w| w[0].radius != w[1].radius),
            "the size is thrown"
        );
        assert!(
            got.windows(2).any(|w| w[0].angle != w[1].angle),
            "the angle is thrown"
        );
        // The rhythm is not thrown: the gap is the one amount whose
        // scale Sketchbook's own assets contradict.
        let xs: Vec<f32> = got.iter().map(|d| d.geom[0] + d.geom[2] / 2.0).collect();
        let gaps: Vec<f32> = xs.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(
            gaps.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-4),
            "the dabs keep their rhythm: {gaps:?}"
        );

        // And thrown no further than it was told to: a radius of 4 px
        // by 3 world units, at zoom 1.
        for d in &got {
            assert!((d.radius - 4.0).abs() <= 3.0 + 1e-4, "radius {}", d.radius);
        }

        // The throw belongs to the stroke, not to the frame it is drawn
        // in: drawing it again lands every dab where it was.
        assert_eq!(stroke_prims(&pts, &stamped(8.0, wild.clone()), &no_pen(), WHITE, &v, &no_sheet()), got);
        // Panning must not re-roll it either — the same dabs, moved.
        let moved = stroke_prims(
            &pts,
            &stamped(8.0, wild),
            &no_pen(),
            WHITE,
            &view(10.0, 0.0, 1.0),
            &no_sheet(),
        );
        assert_eq!(moved.len(), got.len());
        for (a, b) in moved.iter().zip(&got) {
            assert_eq!((a.radius, a.angle), (b.radius, b.angle));
            assert!(
                (a.geom[0] + 10.0 - b.geom[0]).abs() < 1e-3,
                "moved by the camera and no more"
            );
        }
    }

    #[test]
    fn a_flowing_nib_lays_each_dab_at_its_flow_and_the_dabs_pile_up() {
        let v = view(0.0, 0.0, 1.0);
        let nib = stamped(8.0, Stamp { flow: 0.25, ..ROUND });
        let got = stroke_prims(&[[-8.0, 0.0], [8.0, 0.0]], &nib, &no_pen(), WHITE, &v, &no_sheet());
        assert!(got.len() > 1);
        for d in &got {
            assert_eq!(d.color[3], 0.25, "a dab lays its flow; opacity is the ceiling");
        }

        // Dabs have to build toward the stroke's opacity where they
        // cross: a union would cap every crossing at one dab's worth,
        // and flow would be a second opacity.
        let mut f = Frame::new();
        f.stroke(got, &nib);
        assert_eq!(f.groups[0].blend, Blend::Build);

        // A swept stroke still unions — its spans overlap at every
        // joint, and building there would bead.
        let mut f = Frame::new();
        f.stroke(
            vec![Prim::segment((0.0, 0.0), (1.0, 0.0), 1.0, WHITE)],
            &tip(2.0, 0.25, 1.0),
        );
        assert_eq!(f.groups[0].blend, Blend::Union);
    }

    #[test]
    fn frame_append_offsets_the_groups() {
        let mut a = Frame::new();
        a.extend([Prim::rect(sr(0.0, 0.0, 1.0, 1.0), WHITE); 3]);
        let mut b = Frame::new();
        b.group(vec![Prim::rect(sr(0.0, 0.0, 1.0, 1.0), WHITE)], 0.5, Blend::Union, Blend::Over);
        b.extend([Prim::rect(sr(0.0, 0.0, 1.0, 1.0), WHITE)]);
        a.append(b);
        assert_eq!(a.prims.len(), 5);
        assert_eq!((a.groups[0].start, a.groups[0].end), (3, 4));
    }

    #[test]
    fn screen_rect_intersect_and_union() {
        let a = sr(0.0, 0.0, 10.0, 10.0);
        let b = sr(5.0, -5.0, 10.0, 10.0);
        assert_eq!(a.intersect(&b), Some(sr(5.0, 0.0, 5.0, 5.0)));
        assert_eq!(a.union(&b), sr(0.0, -5.0, 15.0, 15.0));
        assert_eq!(a.intersect(&sr(20.0, 0.0, 5.0, 5.0)), None);
        assert_eq!(a.intersect(&sr(10.0, 0.0, 5.0, 5.0)), None, "touching is empty");
    }

    fn flat() -> Prim {
        Prim::rect(sr(10.0, 10.0, 5.0, 5.0), WHITE)
    }

    /// Five prims — `a, b | g1, g2 | c` — with the middle two grouped.
    fn framed(group_at_x: f32, opacity: f32) -> Frame {
        let mut f = Frame::new();
        f.extend([flat(), flat()]);
        f.group(
            vec![
                Prim::segment((group_at_x, 20.0), (group_at_x + 10.0, 20.0), 2.0, WHITE),
                Prim::segment((group_at_x + 10.0, 20.0), (group_at_x + 10.0, 30.5), 2.0, WHITE),
            ],
            opacity,
            Blend::Union,
            Blend::Over,
        );
        f.extend([flat()]);
        f
    }

    /// A tip that rubs out instead of painting.
    fn rubber(width: f64) -> Tip {
        stamped(
            width,
            Stamp {
                mark: Mark::Erase,
                ..ROUND
            },
        )
    }

    #[test]
    fn the_stroke_being_drawn_joins_the_paint_it_is_going_to_land_on() {
        let v = view(0.0, 0.0, 1.0);
        let live_tip = stamped(8.0, ROUND);
        let prims = stroke_prims(&[[0.0, 0.0], [20.0, 0.0]], &live_tip, &no_pen(), WHITE, &v, &no_sheet());
        let n = prims.len();
        let paint = paint_of(vec![laid(vec![cubic()], stamped(8.0, ROUND))]);
        let mut doc = doc_with(vec![paint], &v);
        // An image on a layer above it: the stroke goes under it, which
        // is where it will be once it is let go of.
        doc.layers.push(Layer {
            id: "top".into(),
            ..Layer::of("Layer 2", Kind::Raster)
        });
        let layer = doc.layers[0].id.clone();
        let f = document_prims(
            &doc,
            &v,
            &ImageSlots::new(),
            &no_sheet(),
            EDGE,
            Some(Live {
                layer: &layer,
                prims: prims.clone(),
                tip: &live_tip,
            }),
        );
        assert_eq!(f.prims[f.prims.len() - n..], prims[..], "it is on the layer");

        // On a layer that holds no paint, it is drawn last all the same.
        let f = document_prims(
            &doc,
            &v,
            &ImageSlots::new(),
            &no_sheet(),
            EDGE,
            Some(Live {
                layer: "top",
                prims: prims.clone(),
                tip: &live_tip,
            }),
        );
        assert_eq!(f.prims[f.prims.len() - n..], prims[..]);
    }

    #[test]
    fn an_eraser_over_a_layer_with_no_ink_paints_nothing() {
        let v = view(0.0, 0.0, 1.0);
        let rubber = rubber(8.0);
        let prims = stroke_prims(&[[0.0, 0.0], [20.0, 0.0]], &rubber, &no_pen(), WHITE, &v, &no_sheet());
        assert!(!prims.is_empty(), "it does lay dabs");
        let doc = doc_with(vec![], &v);
        let f = document_prims(
            &doc,
            &v,
            &ImageSlots::new(),
            &no_sheet(),
            EDGE,
            Some(Live {
                layer: "nothing",
                prims,
                tip: &rubber,
            }),
        );
        assert!(
            f.prims.is_empty() && f.groups.is_empty(),
            "there is nothing to rub out, and the board is not it"
        );
    }

    #[test]
    fn an_eraser_joins_the_sheet_while_it_is_still_being_drawn() {
        let v = view(0.0, 0.0, 1.0);
        let rubber = rubber(8.0);
        let prims = stroke_prims(&[[0.0, 0.0], [20.0, 0.0]], &rubber, &no_pen(), WHITE, &v, &no_sheet());
        let paint = paint_of(vec![laid(vec![cubic()], stamped(8.0, ROUND))]);
        let doc = doc_with(vec![paint], &v);
        let layer = doc.layers[0].id.clone();
        let f = document_prims(
            &doc,
            &v,
            &ImageSlots::new(),
            &no_sheet(),
            EDGE,
            Some(Live {
                layer: &layer,
                prims,
                tip: &rubber,
            }),
        );
        assert_eq!(f.sheets.len(), 1, "the layer is put on a sheet for it");
        assert_eq!(f.groups.len(), 1);
        assert_eq!(f.groups[0].lands, Blend::Erase, "and rubbed as it goes");
    }

    #[test]
    fn a_paint_with_nothing_to_rub_out_is_drawn_as_it_always_was() {
        let v = view(0.0, 0.0, 1.0);
        let paint = paint_of(vec![laid(vec![cubic()], tip(8.0, 1.0, 0.5))]);
        let f = document_prims(&doc_with(vec![paint], &v), &v, &ImageSlots::new(), &no_sheet(), EDGE, None);
        assert!(f.sheets.is_empty(), "no sheet is opened for ink alone");
        assert_eq!(f.groups.len(), 1);
    }

    #[test]
    fn a_paint_that_rubs_itself_out_is_built_on_a_sheet() {
        let v = view(0.0, 0.0, 1.0);
        let paint = paint_of(vec![
            laid(vec![cubic()], stamped(8.0, ROUND)),
            laid(vec![cubic()], rubber(8.0)),
        ]);
        let f = document_prims(&doc_with(vec![paint], &v), &v, &ImageSlots::new(), &no_sheet(), EDGE, None);
        assert_eq!(f.sheets.len(), 1, "the layer gets a surface of its own");
        let sheet = f.sheets[0];
        assert_eq!((sheet.start, sheet.end), (0, f.prims.len() as u32));
        // The plain stroke covers on its own and goes straight onto the
        // sheet; the eraser never covers anything, so it is composited
        // and taken back out.
        assert_eq!(f.groups.len(), 1);
        assert_eq!(f.groups[0].lands, Blend::Erase);
        let g = f.groups[0];
        assert!(g.start >= sheet.start && g.end <= sheet.end, "inside the sheet");
    }

    #[test]
    fn the_sheet_is_opened_wiped_and_laid_on_the_window_once() {
        let mut f = Frame::new();
        f.extend([flat()]);
        f.sheet(|f| {
            f.extend([flat()]);
            f.group(
                vec![Prim::segment((10.0, 10.0), (20.0, 10.0), 2.0, WHITE)],
                0.5,
                Blend::Build,
                Blend::Erase,
            );
        });
        f.extend([flat()]);
        let (prims, plan) = passes(&f, VP, 9, &[8]);
        let onto: Vec<Onto> = plan.iter().map(|p| p.onto).collect();
        assert_eq!(
            onto,
            vec![Onto::Window, Onto::Sheet(1), Onto::Scratch, Onto::Sheet(1), Onto::Window],
            "the window, then the sheet is opened and rubbed, then the window"
        );
        // The sheet is cleared by the first pass drawn onto it.
        let open = plan[1];
        assert_eq!(open.wipe.map(|w| prims[w as usize].color), Some([0.0; 4]));
        assert_eq!((open.start, open.end), (1, 2), "the plain prim on it");
        // The eraser is built in the scratch and taken out of the sheet.
        assert_eq!(plan[2].blend, Blend::Build);
        let out = plan[3].lay.expect("the eraser lands on the sheet");
        assert_eq!(out.blend, Blend::Erase);
        assert_eq!(prims[out.prim as usize].slot, 9, "out of the scratch");
        // And the sheet lands on the window whole, at full strength: a
        // stroke's own opacity was spent on the way in.
        let laid = plan[4].lay.expect("the sheet is laid down");
        assert_eq!(laid.blend, Blend::Over);
        assert_eq!(prims[laid.prim as usize].slot, 8, "the sheet itself");
        assert_eq!(prims[laid.prim as usize].color, [1.0; 4]);
        assert_eq!((plan[4].start, plan[4].end), (3, 4), "and the last prim over it");
    }

    /// Two overlapping rects on the board's one layer, which is `opacity`
    /// strong.
    fn faded(opacity: f64) -> Document {
        let v = view(0.0, 0.0, 1.0);
        let mut doc = doc_with(
            vec![
                rect(-10.0, -10.0, 20.0, 20.0, None, Some("#ff0000")),
                rect(0.0, 0.0, 20.0, 20.0, None, Some("#0000ff")),
            ],
            &v,
        );
        doc.layers[0].opacity = opacity;
        doc
    }

    fn drawn(doc: &Document) -> Frame {
        document_prims(doc, &view(0.0, 0.0, 1.0), &ImageSlots::new(), &no_sheet(), EDGE, None)
    }

    #[test]
    fn a_layer_below_full_strength_is_laid_as_one() {
        let f = drawn(&faded(0.5));
        assert_eq!(f.sheets.len(), 1, "one surface for the layer");
        let s = f.sheets[0];
        assert_eq!((s.start, s.end), (0, f.prims.len() as u32), "holding both rects");
        assert_eq!((s.opacity, s.lays, s.backdrop), (0.5, Blend::Over, false));
        // At full strength nothing opens: a board that asks for nothing is
        // drawn exactly as it always was.
        let f = drawn(&faded(1.0));
        assert!(f.sheets.is_empty());
    }

    /// `faded` with its layer inside a group, `G`, that is `opacity`
    /// strong and blends as `blend`.
    fn grouped(opacity: f64, blend: BlendMode) -> Document {
        let mut doc = faded(1.0);
        let inner = doc.layers.remove(0);
        doc.layers.push(Layer {
            id: "G".into(),
            opacity,
            blend,
            layers: vec![inner],
            ..Layer::of("Group 1", Kind::Group)
        });
        doc
    }

    #[test]
    fn a_group_passing_through_at_full_strength_is_no_surface_at_all() {
        let f = drawn(&grouped(1.0, BlendMode::PassThrough));
        assert!(f.sheets.is_empty());
        assert_eq!(f.prims.len(), 2);
    }

    #[test]
    fn a_group_passing_through_below_full_strength_is_mixed_over_what_is_under_it() {
        let f = drawn(&grouped(0.25, BlendMode::PassThrough));
        assert_eq!(f.sheets.len(), 1);
        let s = f.sheets[0];
        assert_eq!((s.opacity, s.lays, s.backdrop), (0.25, Blend::Mix, true));
    }

    #[test]
    fn a_normal_group_below_full_strength_is_isolated() {
        let f = drawn(&grouped(0.25, BlendMode::Normal));
        let s = f.sheets[0];
        assert_eq!((s.opacity, s.lays, s.backdrop), (0.25, Blend::Over, false));
        // Its layer at half strength inside it: a surface in a surface.
        let mut doc = grouped(0.25, BlendMode::Normal);
        doc.layers[0].layers[0].opacity = 0.5;
        let f = drawn(&doc);
        assert_eq!(f.sheets.len(), 2);
        let (outer, inner) = (f.sheets[0], f.sheets[1]);
        assert_eq!((outer.opacity, inner.opacity), (0.25, 0.5), "outer first");
        assert!(inner.start >= outer.start && inner.end <= outer.end, "nested");
        assert_eq!(depth(&f), 2);
    }

    #[test]
    fn a_frame_below_full_strength_takes_its_ground_along() {
        let v = view(0.0, 0.0, 1.0);
        let mut doc = Document::from_json(
            r##"{
                "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
                "camera": { "x": 0, "y": 0, "zoom": 1 },
                "layers": [ { "id": "fl", "name": "Frame 1", "kind": "frame", "opacity": 0.5 } ],
                "elements": [
                    { "id": "fr", "type": "frame", "layer": "fl", "background": "#ffffff",
                      "x": -20, "y": -20, "w": 40, "h": 40, "layers": [ { "id": "in", "name": "Layer 1" } ] },
                    { "id": "r", "type": "rect", "layer": "in", "x": 0, "y": 0, "w": 5, "h": 5,
                      "stroke": null, "fill": "#000000", "text": null }
                ]
            }"##,
        )
        .unwrap();
        doc.camera = v.camera;
        let f = drawn(&doc);
        assert_eq!(f.sheets.len(), 1);
        let s = f.sheets[0];
        assert_eq!((s.start, s.end), (0, f.prims.len() as u32), "the ground, the edge and the rect");
        assert_eq!(s.opacity, 0.5);
    }

    #[test]
    fn the_live_stroke_is_drawn_inside_its_layers_surface() {
        let v = view(0.0, 0.0, 1.0);
        let doc = faded(0.5);
        let layer = doc.layers[0].id.clone();
        let live_tip = stamped(8.0, ROUND);
        let prims = stroke_prims(&[[0.0, 0.0], [20.0, 0.0]], &live_tip, &no_pen(), WHITE, &v, &no_sheet());
        let n = prims.len() as u32;
        let f = document_prims(
            &doc,
            &v,
            &ImageSlots::new(),
            &no_sheet(),
            EDGE,
            Some(Live {
                layer: &layer,
                prims,
                tip: &live_tip,
            }),
        );
        let s = f.sheets[0];
        assert_eq!(s.end, f.prims.len() as u32, "the live ink is the last of it");
        assert!(s.end - s.start >= 2 + n, "inside, at the layer's strength");
    }

    #[test]
    fn a_layer_that_blends_is_laid_with_its_mode() {
        let mut doc = faded(1.0);
        doc.layers[0].blend = BlendMode::Multiply;
        let f = drawn(&doc);
        assert_eq!(f.sheets.len(), 1, "a mode is composited as one, whatever the strength");
        let s = f.sheets[0];
        assert_eq!((s.opacity, s.lays, s.backdrop), (1.0, Blend::Mode(BlendMode::Multiply), false));
    }

    #[test]
    fn a_normal_group_holding_a_layer_that_blends_is_isolated() {
        // Its layers blend with each other and not with what is under
        // the group, however strong the group is.
        let mut doc = grouped(1.0, BlendMode::Normal);
        doc.layers[0].layers[0].blend = BlendMode::Screen;
        let f = drawn(&doc);
        assert_eq!(f.sheets.len(), 2);
        assert_eq!(f.sheets[0].lays, Blend::Over, "the group, isolated");
        assert_eq!(f.sheets[1].lays, Blend::Mode(BlendMode::Screen));
        // Passing through, its layers blend with the board itself.
        let mut doc = grouped(1.0, BlendMode::PassThrough);
        doc.layers[0].layers[0].blend = BlendMode::Screen;
        let f = drawn(&doc);
        assert_eq!(f.sheets.len(), 1, "no surface for the group");
        // And a group that blends is isolated and laid with its mode.
        let f = drawn(&grouped(1.0, BlendMode::Difference));
        assert_eq!(f.sheets[0].lays, Blend::Mode(BlendMode::Difference));
    }

    #[test]
    fn a_mode_is_laid_over_a_copy_of_what_is_under_it_carrying_its_mode() {
        let mut f = Frame::new();
        f.extend([flat()]);
        f.layer(0.75, Blend::Mode(BlendMode::Overlay), false, |f| f.extend([flat()]));
        let (prims, plan) = passes(&f, VP, 9, &[7]);
        let laid = plan[2];
        assert_eq!(laid.onto, Onto::Window);
        let lay = laid.lay.expect("the surface is laid down");
        assert_eq!(lay.blend, Blend::Mode(BlendMode::Overlay));
        // What it is laid on is copied out first, to be read under it.
        let copy = laid.copy.expect("a copy of what it is laid on");
        assert_eq!((copy.from, copy.to), (Onto::Window, Onto::Backdrop));
        assert_eq!(copy.rect, flat().painted_bounds().snapped());
        let p = prims[lay.prim as usize];
        assert_eq!(p.color, [0.75; 4], "at its strength");
        assert_eq!(p.mode(), Some(BlendMode::Overlay), "and its mode rides on the prim");
        assert_eq!(flat().mode(), None);
    }

    #[test]
    fn every_mode_has_a_number_and_the_number_gives_it_back() {
        for (i, mode) in BlendMode::ALL.iter().enumerate() {
            assert_eq!(mode_number(*mode) as usize, i);
            assert_eq!(BlendMode::ALL.get(mode_number(*mode) as usize), Some(mode));
        }
    }

    #[test]
    fn surfaces_nest_one_per_depth_and_are_laid_back_in_turn() {
        let mut f = Frame::new();
        f.extend([flat()]);
        f.layer(0.25, Blend::Over, false, |f| {
            f.extend([flat()]);
            f.layer(0.5, Blend::Over, false, |f| f.extend([flat()]));
        });
        let (prims, plan) = passes(&f, VP, 9, &[7, 8]);
        let onto: Vec<Onto> = plan.iter().map(|p| p.onto).collect();
        assert_eq!(
            onto,
            [Onto::Window, Onto::Sheet(1), Onto::Sheet(2), Onto::Sheet(1), Onto::Window]
        );
        // Each is opened by the first pass drawn onto it.
        assert!(plan[1].wipe.is_some() && plan[2].wipe.is_some());
        // The inner one is laid on the outer at its own strength, and the
        // outer on the window at its own.
        let inner = plan[3].lay.expect("the inner surface is laid down");
        assert_eq!(prims[inner.prim as usize].slot, 8);
        assert_eq!(prims[inner.prim as usize].color, [0.5; 4]);
        let outer = plan[4].lay.expect("and then the outer");
        assert_eq!(prims[outer.prim as usize].slot, 7);
        assert_eq!(prims[outer.prim as usize].color, [0.25; 4]);
        assert_eq!(outer.blend, Blend::Over);
    }

    #[test]
    fn a_surface_passing_through_opens_on_a_copy_of_what_is_under_it() {
        let mut f = Frame::new();
        f.extend([flat()]);
        f.layer(0.25, Blend::Mix, true, |f| f.extend([flat()]));
        let (prims, plan) = passes(&f, VP, 9, &[7]);
        let open = plan[1];
        assert_eq!(open.onto, Onto::Sheet(1));
        assert!(open.wipe.is_none(), "not wiped");
        let copy = open.copy.expect("a copy of the window under it");
        assert_eq!((copy.from, copy.to), (Onto::Window, Onto::Sheet(1)));
        assert_eq!(copy.rect, flat().painted_bounds().snapped());
        // Laid back as a mix, its strength riding on the composite.
        let lay = plan[2].lay.expect("laid back");
        assert_eq!(lay.blend, Blend::Mix);
        assert_eq!(prims[lay.prim as usize].color, [0.25; 4]);
    }

    #[test]
    fn past_the_last_surface_a_layer_is_drawn_straight() {
        let mut f = Frame::new();
        f.layer(0.5, Blend::Over, false, |f| {
            f.layer(0.5, Blend::Over, false, |f| f.extend([flat()]));
        });
        let (_, plan) = passes(&f, VP, 9, &[7]);
        let onto: Vec<Onto> = plan.iter().map(|p| p.onto).collect();
        assert_eq!(onto, [Onto::Window, Onto::Sheet(1), Onto::Window]);
        assert_eq!(depth(&f), 2, "it asked for two");
    }

    #[test]
    fn a_sheet_outside_the_viewport_goes_with_everything_on_it() {
        let mut f = Frame::new();
        f.extend([flat()]);
        f.sheet(|f| {
            f.group(
                vec![Prim::segment((900.0, 10.0), (920.0, 10.0), 2.0, WHITE)],
                0.5,
                Blend::Build,
                Blend::Erase,
            );
        });
        let (prims, plan) = passes(&f, VP, 9, &[8]);
        assert_eq!(prims.len(), 2, "no wipe and no composite for either");
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].onto, Onto::Window);
        assert_eq!((plan[0].start, plan[0].end), (0, 1));
    }

    #[test]
    fn passes_split_around_a_group() {
        let f = framed(20.0, 0.5);
        let (prims, plan) = passes(&f, VP, 9, &[8]);
        assert_eq!(
            plan,
            vec![
                Pass {
                    onto: Onto::Window,
                    copy: None,
                    wipe: None,
                    lay: None,
                    start: 0,
                    end: 2,
                    blend: Blend::Over,
                },
                Pass {
                    onto: Onto::Scratch,
                    copy: None,
                    wipe: Some(5),
                    lay: None,
                    start: 2,
                    end: 4,
                    blend: Blend::Union,
                },
                Pass {
                    onto: Onto::Window,
                    copy: None,
                    wipe: None,
                    lay: Some(Lay { prim: 6, blend: Blend::Over }),
                    start: 4,
                    end: 5,
                    blend: Blend::Over,
                },
            ]
        );
        assert_eq!(prims.len(), 7);
        assert_eq!(&prims[..5], &f.prims[..]);
        // The wipe is the group's bounds, snapped out to whole pixels,
        // in transparent black.
        let bounds = f.groups[0].bounds;
        assert_eq!(bounds, sr(16.5, 16.5, 17.0, 17.5));
        let snapped = sr(16.0, 16.0, 18.0, 18.0);
        assert_eq!(prims[5], Prim::rect(snapped, [0.0; 4]));
        // The composite lays the scratch's own pixels back over the same
        // box at the group's opacity.
        assert_eq!(prims[6].kind, KIND_IMAGE);
        assert_eq!(prims[6].slot, 9);
        assert_eq!(prims[6].geom, [16.0, 16.0, 18.0, 18.0]);
        assert_eq!(prims[6].color, [0.5; 4]);
        assert_eq!(prims[6].uv, [0.16, 0.16, 0.34, 0.34]);
    }

    #[test]
    fn a_group_outside_the_viewport_is_dropped_with_its_prims() {
        let f = framed(200.0, 0.5);
        let (prims, plan) = passes(&f, VP, 9, &[8]);
        assert_eq!(prims.len(), 5, "no wipe, no composite");
        assert_eq!(
            plan,
            vec![
                Pass {
                    onto: Onto::Window,
                    copy: None,
                    wipe: None,
                    lay: None,
                    start: 0,
                    end: 2,
                    blend: Blend::Over,
                },
                Pass {
                    onto: Onto::Window,
                    copy: None,
                    wipe: None,
                    lay: None,
                    start: 4,
                    end: 5,
                    blend: Blend::Over,
                },
            ]
        );
    }

    #[test]
    fn a_group_half_outside_is_clipped_to_the_viewport() {
        let f = framed(95.0, 1.0);
        let (prims, plan) = passes(&f, VP, 9, &[8]);
        assert_eq!(plan.len(), 3);
        assert_eq!(prims[5].geom, [91.0, 16.0, 9.0, 18.0]);
        assert_eq!(prims[6].uv, [0.91, 0.16, 1.0, 0.34]);
    }

    #[test]
    fn a_frame_without_groups_is_one_direct_pass() {
        let mut f = Frame::new();
        f.extend([flat(), flat()]);
        let (prims, plan) = passes(&f, VP, 9, &[8]);
        assert_eq!(prims.len(), 2);
        assert_eq!(plan, vec![Pass {
                    onto: Onto::Window,
                    copy: None,
                    wipe: None,
                    lay: None,
                    start: 0,
                    end: 2,
                    blend: Blend::Over,
                }]);
        // Even an empty frame is one pass: it is what clears the window.
        let (prims, plan) = passes(&Frame::new(), VP, 9, &[8]);
        assert!(prims.is_empty());
        assert_eq!(plan, vec![Pass {
                    onto: Onto::Window,
                    copy: None,
                    wipe: None,
                    lay: None,
                    start: 0,
                    end: 0,
                    blend: Blend::Over,
                }]);
    }

    #[test]
    fn a_group_that_opens_the_frame_still_gets_a_clearing_pass_first() {
        let mut f = Frame::new();
        f.group(
            vec![Prim::segment((10.0, 10.0), (20.0, 10.0), 2.0, WHITE)],
            0.5,
            Blend::Union,
            Blend::Over,
        );
        let (_, plan) = passes(&f, VP, 9, &[8]);
        assert_eq!(
            plan,
            vec![
                Pass {
                    onto: Onto::Window,
                    copy: None,
                    wipe: None,
                    lay: None,
                    start: 0,
                    end: 0,
                    blend: Blend::Over,
                },
                Pass {
                    onto: Onto::Scratch,
                    copy: None,
                    wipe: Some(1),
                    lay: None,
                    start: 0,
                    end: 1,
                    blend: Blend::Union,
                },
                Pass {
                    onto: Onto::Window,
                    copy: None,
                    wipe: None,
                    lay: Some(Lay { prim: 2, blend: Blend::Over }),
                    start: 1,
                    end: 1,
                    blend: Blend::Over,
                },
            ]
        );
    }

    #[test]
    fn composite_samples_the_scratch_over_its_own_bounds() {
        let p = Prim::composite(sr(10.0, 20.0, 30.0, 40.0), Viewport { w: 100, h: 200 }, 4, 0.75);
        assert_eq!(p.kind, KIND_IMAGE);
        assert_eq!(p.slot, 4);
        assert_eq!(p.geom, [10.0, 20.0, 30.0, 40.0]);
        assert_eq!(p.uv, [0.1, 0.1, 0.4, 0.3]);
        // Premultiplied texels: opacity scales every channel.
        assert_eq!(p.color, [0.75; 4]);
        assert_eq!(p.angle, 0.0);
        assert_eq!(p.radius, 0.0);
    }

    #[test]
    fn painted_order_puts_a_lower_layer_first_and_hides_a_hidden_one() {
        let v = view(0.0, 0.0, 1.0);
        let mut doc = doc_with(
            vec![
                rect(0.0, 0.0, 10.0, 10.0, None, Some("#fff")),
                rect(20.0, 0.0, 10.0, 10.0, None, Some("#000")),
            ],
            &v,
        );
        doc.layers.push(Layer {
            id: "top".into(),
            ..Layer::of("Layer 2", Kind::Raster)
        });
        // The white rect is first in `elements` but on the top layer.
        doc.elements[0].set_layer("top");
        let got = document_prims(&doc, &v, &ImageSlots::new(), &no_sheet(), EDGE, None).prims;
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].color, [0.0, 0.0, 0.0, 1.0], "the lower layer paints first");
        assert_eq!(got[1].color, WHITE);
        doc.layers[1].visible = false;
        let got = document_prims(&doc, &v, &ImageSlots::new(), &no_sheet(), EDGE, None).prims;
        assert_eq!(got.len(), 1, "a hidden layer paints nothing");
    }
    const BLOB: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const WHITE: Rgba = [1.0, 1.0, 1.0, 1.0];

    fn view(x: f64, y: f64, zoom: f64) -> View {
        View {
            camera: Camera { x, y, zoom },
            viewport: VP,
            scale: 1.0,
        }
    }

    fn doc_with(elements: Vec<Element>, view: &View) -> Document {
        let mut d = Document::new("t");
        d.camera = view.camera;
        d.elements = elements;
        let layer = d.layers[0].id.clone();
        for el in &mut d.elements {
            el.set_layer(&layer);
        }
        d
    }

    fn rect(x: f64, y: f64, w: f64, h: f64, stroke: Option<&str>, fill: Option<&str>) -> Element {
        Element::Rect(Rect {
            id: "el".into(),
            layer: String::new(),
            x,
            y,
            w,
            h,
            rotation: 0.0,
            stroke: stroke.map(Into::into),
            fill: fill.map(Into::into),
            text: None,
        })
    }

    fn sr(x: f32, y: f32, w: f32, h: f32) -> ScreenRect {
        ScreenRect { x, y, w, h }
    }

    #[test]
    fn world_origin_lands_on_viewport_center() {
        assert_eq!(view(0.0, 0.0, 1.0).world_to_screen(0.0, 0.0), (50.0, 50.0));
    }

    #[test]
    fn camera_pan_and_zoom_transform_points() {
        // Camera looking at (10, 20) with zoom 2: (10, 20) itself is centered…
        let v = view(10.0, 20.0, 2.0);
        assert_eq!(v.world_to_screen(10.0, 20.0), (50.0, 50.0));
        // …and a point 5 units to the right shows 10px right of center.
        assert_eq!(v.world_to_screen(15.0, 20.0), (60.0, 50.0));
    }

    #[test]
    fn scale_factor_multiplies_zoom() {
        // World units are logical pixels at zoom 1: on a 2x display each
        // maps to two physical pixels.
        let v = View {
            scale: 2.0,
            ..view(0.0, 0.0, 1.0)
        };
        assert_eq!(v.px_per_world(), 2.0);
        assert_eq!(v.world_to_screen(5.0, 0.0), (60.0, 50.0));
    }

    #[test]
    fn screen_to_world_inverts_world_to_screen() {
        let v = View {
            scale: 1.5,
            ..view(-3.5, 8.0, 2.5)
        };
        for (wx, wy) in [(0.0, 0.0), (10.0, -4.0), (123.25, 7.5)] {
            let (sx, sy) = v.world_to_screen(wx, wy);
            let (bx, by) = v.screen_to_world(sx, sy);
            assert!(
                (bx - wx).abs() < 1e-9 && (by - wy).abs() < 1e-9,
                "({wx},{wy})"
            );
        }
    }

    #[test]
    fn showing_puts_a_world_point_under_a_screen_point_at_a_zoom() {
        let v = view(0.0, 0.0, 1.0);
        let cam = v.showing((10.0, 20.0), (80.0, 30.0), 2.0);
        let moved = View { camera: cam, ..v };
        assert_eq!(cam.zoom, 2.0);
        assert_eq!(moved.world_to_screen(10.0, 20.0), (80.0, 30.0));
    }

    #[test]
    fn showing_clamps_the_zoom_and_ignores_junk() {
        let v = view(0.0, 0.0, 1.0);
        assert_eq!(v.showing((0.0, 0.0), (50.0, 50.0), 1e9).zoom, ZOOM_MAX);
        assert_eq!(v.showing((0.0, 0.0), (50.0, 50.0), 1e-9).zoom, ZOOM_MIN);
        assert_eq!(v.showing((0.0, 0.0), (50.0, 50.0), f64::NAN), v.camera);
        assert_eq!(v.showing((0.0, 0.0), (50.0, 50.0), 0.0).zoom, ZOOM_MIN);
    }

    #[test]
    fn zoomed_at_keeps_the_point_under_the_cursor_fixed() {
        let v = View {
            scale: 2.0,
            ..view(5.0, -5.0, 1.0)
        };
        let (wx, wy) = v.screen_to_world(20.0, 70.0);
        let zoomed = View {
            camera: v.zoomed_at(1.25, (20.0, 70.0)),
            ..v
        };
        assert_eq!(zoomed.camera.zoom, 1.25);
        let (sx, sy) = zoomed.world_to_screen(wx, wy);
        assert!(
            (sx - 20.0).abs() < 1e-9 && (sy - 70.0).abs() < 1e-9,
            "{sx},{sy}"
        );
    }

    #[test]
    fn panned_by_moves_the_content_with_the_delta() {
        // Dragging 10px right and 4px down on a 2x display at zoom 2 moves the
        // camera 2.5 / 1 world units the other way.
        let v = View {
            scale: 2.0,
            ..view(1.0, 1.0, 2.0)
        };
        let cam = v.panned_by(10.0, 4.0);
        assert_eq!(
            cam,
            Camera {
                x: -1.5,
                y: 0.0,
                zoom: 2.0
            }
        );
        let moved = View { camera: cam, ..v };
        let before = v.world_to_screen(0.0, 0.0);
        let after = moved.world_to_screen(0.0, 0.0);
        assert_eq!((after.0 - before.0, after.1 - before.1), (10.0, 4.0));
    }

    #[test]
    fn parses_hex_colors_to_linear_rgba() {
        assert_eq!(parse_color("#000"), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(parse_color("#ffffff"), WHITE);
        // #222 in sRGB: 0x22/255 ≈ 0.1333 → linear ≈ 0.0159963.
        let c = parse_color("#222");
        assert!((c[0] - 0.0159963).abs() < 1e-4, "{c:?}");
        assert_eq!(c[0], c[1]);
        assert_eq!(c[1], c[2]);
        assert_eq!(c[3], 1.0);
        // #rgb expands by doubling each digit: #7aa == #77aaaa.
        assert_eq!(parse_color("#7aa"), parse_color("#77aaaa"));
    }

    #[test]
    fn invalid_colors_fall_back_to_gray() {
        for bad in ["", "#", "#12", "#12345", "red", "#gggggg"] {
            assert_eq!(parse_color(bad), FALLBACK_COLOR, "color {bad:?}");
            assert_eq!(try_parse_color(bad), None, "color {bad:?}");
        }
        assert_eq!(try_parse_color("#fff"), Some(WHITE));
    }

    #[test]
    fn mix_interpolates_channels_and_keeps_alpha_opaque() {
        let black = [0.0, 0.0, 0.0, 1.0];
        assert_eq!(mix(black, WHITE, 0.25), [0.25, 0.25, 0.25, 1.0]);
        assert_eq!(mix(black, WHITE, 0.0), black);
        assert_eq!(mix(black, WHITE, 1.0), WHITE);
    }

    #[test]
    fn with_alpha_replaces_only_the_alpha_channel() {
        assert_eq!(with_alpha(WHITE, 0.3), [1.0, 1.0, 1.0, 0.3]);
    }

    #[test]
    fn screen_rect_contains_points_inside_and_on_the_near_edges() {
        let r = sr(10.0, 20.0, 30.0, 40.0);
        assert!(r.contains(10.0, 20.0));
        assert!(r.contains(25.0, 50.0));
        assert!(!r.contains(9.99, 30.0));
        assert!(!r.contains(40.01, 30.0));
        assert!(!r.contains(20.0, 60.01));
        assert_eq!(r.center(), (25.0, 40.0));
        assert_eq!(r.inset(5.0), sr(15.0, 25.0, 20.0, 30.0));
        assert_eq!(r.inset(-1.0), sr(9.0, 19.0, 32.0, 42.0));
        assert_eq!(r.offset(1.0, -2.0), sr(11.0, 18.0, 30.0, 40.0));
    }

    #[test]
    fn screen_rect_contains_rect_requires_full_containment() {
        let r = sr(10.0, 20.0, 30.0, 40.0);
        assert!(r.contains_rect(&sr(10.0, 20.0, 30.0, 40.0)));
        assert!(r.contains_rect(&sr(15.0, 25.0, 5.0, 5.0)));
        assert!(!r.contains_rect(&sr(5.0, 25.0, 10.0, 5.0)));
        assert!(!r.contains_rect(&sr(15.0, 25.0, 30.0, 5.0)));
    }

    #[track_caller]
    fn assert_close4(a: [f32; 4], b: [f32; 4]) {
        for k in 0..4 {
            assert!((a[k] - b[k]).abs() < 1e-4, "{a:?} != {b:?}");
        }
    }

    #[test]
    fn turned_box_keeps_its_size_and_orbits_the_pivot() {
        // A 10×2 box centered 10px right of the pivot, turned a quarter
        // clockwise: its center lands 10px below the pivot.
        let a = std::f32::consts::FRAC_PI_2;
        let p = Prim::turned(sr(15.0, 9.0, 10.0, 2.0), (10.0, 10.0), a, WHITE);
        assert_eq!(p.kind, KIND_BOX);
        assert_close4(p.geom, [5.0, 19.0, 10.0, 2.0]);
        assert_eq!(p.angle, a);
        // No turn is a plain rect.
        assert_eq!(
            Prim::turned(sr(1.0, 2.0, 3.0, 4.0), (0.0, 0.0), 0.0, WHITE),
            Prim::rect(sr(1.0, 2.0, 3.0, 4.0), WHITE)
        );
    }

    #[test]
    fn turned_box_bounds_wrap_the_turned_box() {
        let a = std::f32::consts::FRAC_PI_2;
        let p = Prim::turned(sr(0.0, 0.0, 10.0, 2.0), (5.0, 1.0), a, WHITE);
        let b = p.bounds();
        assert_close4([b.x, b.y, b.w, b.h], [4.0, -4.0, 2.0, 10.0]);
    }

    #[test]
    fn rotated_rect_turns_fill_and_edges_about_its_center() {
        let v = view(0.0, 0.0, 1.0);
        let mut el = rect(0.0, 0.0, 20.0, 10.0, Some("#000"), Some("#fff"));
        if let Element::Rect(r) = &mut el {
            r.rotation = 90.0;
        }
        let got = document_prims(&doc_with(vec![el], &v), &v, &ImageSlots::new(), &no_sheet(), EDGE, None).prims;
        assert_eq!(got.len(), 5);
        let a = std::f32::consts::FRAC_PI_2;
        // The fill is the unturned box, turned in place.
        assert_eq!(got[0].geom, [50.0, 50.0, 20.0, 10.0]);
        assert_eq!(got[0].angle, a);
        // The top edge orbits the rect center (60, 55): its own center goes
        // from (60, 50 + t/2) to (65 - t/2, 55).
        let t = STROKE_PX;
        assert_close4(got[1].geom, [55.0 - t / 2.0, 55.0 - t / 2.0, 20.0, t]);
        for p in &got[1..] {
            assert_eq!(p.angle, a, "{p:?}");
        }
    }

    #[test]
    fn fill_only_rect_becomes_one_box() {
        let v = view(0.0, 0.0, 1.0);
        let doc = doc_with(vec![rect(0.0, 0.0, 10.0, 10.0, None, Some("#fff"))], &v);
        assert_eq!(
            document_prims(&doc, &v, &ImageSlots::new(), &no_sheet(), EDGE, None).prims,
            vec![Prim::rect(sr(50.0, 50.0, 10.0, 10.0), WHITE)]
        );
    }

    #[test]
    fn stroke_only_rect_becomes_four_inner_edges() {
        let v = view(0.0, 0.0, 1.0);
        let doc = doc_with(vec![rect(10.0, 10.0, 20.0, 20.0, Some("#fff"), None)], &v);
        let t = STROKE_PX;
        assert_eq!(
            document_prims(&doc, &v, &ImageSlots::new(), &no_sheet(), EDGE, None).prims,
            vec![
                // top, bottom, left, right — aligned inwards.
                Prim::rect(sr(60.0, 60.0, 20.0, t), WHITE),
                Prim::rect(sr(60.0, 80.0 - t, 20.0, t), WHITE),
                Prim::rect(sr(60.0, 60.0 + t, t, 20.0 - 2.0 * t), WHITE),
                Prim::rect(sr(80.0 - t, 60.0 + t, t, 20.0 - 2.0 * t), WHITE),
            ]
        );
    }

    #[test]
    fn fill_and_stroke_paint_fill_first() {
        let v = view(0.0, 0.0, 1.0);
        let doc = doc_with(
            vec![rect(0.0, 0.0, 10.0, 10.0, Some("#000"), Some("#fff"))],
            &v,
        );
        let got = document_prims(&doc, &v, &ImageSlots::new(), &no_sheet(), EDGE, None).prims;
        assert_eq!(got.len(), 5);
        assert_eq!(got[0].color, WHITE, "fill first");
        assert_eq!(got[1].color, [0.0, 0.0, 0.0, 1.0], "stroke after");
    }

    #[test]
    fn zoom_scales_rect_position_and_size() {
        let v = view(0.0, 0.0, 2.0);
        let doc = doc_with(vec![rect(1.0, 0.0, 5.0, 5.0, None, Some("#fff"))], &v);
        let got = document_prims(&doc, &v, &ImageSlots::new(), &no_sheet(), EDGE, None).prims;
        assert_eq!(got[0].geom, [52.0, 50.0, 10.0, 10.0]);
    }

    #[test]
    fn rect_without_any_color_still_paints_with_fallback() {
        let v = view(0.0, 0.0, 1.0);
        let doc = doc_with(vec![rect(0.0, 0.0, 4.0, 4.0, None, None)], &v);
        let got = document_prims(&doc, &v, &ImageSlots::new(), &no_sheet(), EDGE, None).prims;
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].color, FALLBACK_COLOR);
    }

    #[test]
    fn single_point_polyline_is_a_dot() {
        assert_eq!(
            polyline_prims(&[(10.0, 10.0)], 2.0, WHITE),
            vec![Prim::circle(10.0, 10.0, 2.0, WHITE)]
        );
        assert!(polyline_prims(&[], 2.0, WHITE).is_empty());
    }

    #[test]
    fn polyline_becomes_one_segment_per_span_skipping_repeats() {
        let got = polyline_prims(
            &[(0.0, 0.0), (10.0, 0.0), (10.0, 0.0), (10.0, 5.0)],
            1.5,
            WHITE,
        );
        assert_eq!(
            got,
            vec![
                Prim::segment((0.0, 0.0), (10.0, 0.0), 1.5, WHITE),
                Prim::segment((10.0, 0.0), (10.0, 5.0), 1.5, WHITE),
            ]
        );
        // A polyline whose points all coincide is still visible as a dot.
        assert_eq!(
            polyline_prims(&[(3.0, 3.0), (3.0, 3.0)], 1.0, WHITE),
            vec![Prim::circle(3.0, 3.0, 1.0, WHITE)]
        );
    }

    #[test]
    fn prim_bounds_cover_the_painted_area() {
        assert_eq!(
            Prim::segment((0.0, 0.0), (10.0, 0.0), 2.0, WHITE).bounds(),
            sr(-2.0, -2.0, 14.0, 4.0)
        );
        assert_eq!(
            Prim::circle(5.0, 5.0, 3.0, WHITE).bounds(),
            sr(2.0, 2.0, 6.0, 6.0)
        );
        assert_eq!(
            Prim::rounded(sr(1.0, 2.0, 3.0, 4.0), 1.0, WHITE).bounds(),
            sr(1.0, 2.0, 3.0, 4.0)
        );
    }

    #[test]
    fn path_element_is_stroked_in_screen_space() {
        let v = view(0.0, 0.0, 2.0);
        let doc = doc_with(
            vec![Element::Path(Path {
                id: "p".into(),
                layer: String::new(),
                curves: vec![[[0.0, 0.0], [3.0, 0.0], [6.0, 0.0], [9.0, 0.0]]],
                stroke: "#000".into(),
                width: 2.0,
                opacity: 1.0,
                hardness: 1.0,
                rotation: 0.0,
                stamp: None,
                pen: Envelope::default(),
            })],
            &v,
        );
        // Width is in world units: 2 * zoom 2 = 4px wide → half-width 2.
        assert_eq!(
            document_prims(&doc, &v, &ImageSlots::new(), &no_sheet(), EDGE, None).prims,
            vec![Prim::segment(
                (50.0, 50.0),
                (68.0, 50.0),
                2.0,
                [0.0, 0.0, 0.0, 1.0]
            )]
        );
    }

    #[test]
    fn path_prims_flatten_curves_in_screen_pixels() {
        let c = [[0.0, 0.0], [0.0, 55.0], [45.0, 100.0], [100.0, 100.0]];
        let at_1x = path_prims(&[c], &Tip::PENCIL, &no_pen(), WHITE, &view(0.0, 0.0, 1.0), &no_sheet());
        assert!(at_1x.len() > 1, "a curve is more than one segment");
        let first = at_1x[0].geom;
        let last = at_1x[at_1x.len() - 1].geom;
        assert_eq!((first[0], first[1]), (50.0, 50.0));
        assert_eq!((last[2], last[3]), (150.0, 150.0));
        // Flattening tolerance is in pixels, so zooming in adds segments.
        let at_4x = path_prims(&[c], &Tip::PENCIL, &no_pen(), WHITE, &view(0.0, 0.0, 4.0), &no_sheet());
        assert!(
            at_4x.len() > at_1x.len(),
            "{} vs {}",
            at_4x.len(),
            at_1x.len()
        );
    }

    #[test]
    fn path_prims_join_consecutive_cubics_without_a_gap() {
        let a = [[0.0, 0.0], [3.0, 0.0], [6.0, 0.0], [9.0, 0.0]];
        let b = [[9.0, 0.0], [12.0, 0.0], [15.0, 0.0], [18.0, 0.0]];
        let got = path_prims(&[a, b], &Tip::PENCIL, &no_pen(), WHITE, &view(0.0, 0.0, 1.0), &no_sheet());
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0].geom, [50.0, 50.0, 59.0, 50.0]);
        assert_eq!(got[1].geom, [59.0, 50.0, 68.0, 50.0]);
    }

    #[test]
    fn stroke_width_never_drops_below_one_pixel() {
        let v = view(0.0, 0.0, 0.1);
        let got = stroke_prims(&[[0.0, 0.0], [100.0, 0.0]], &Tip::PENCIL, &no_pen(), WHITE, &v, &no_sheet());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].radius, 0.5);
    }

    fn image(x: f64, y: f64, w: f64, h: f64, rotation: f64) -> Element {
        Element::Image(crate::doc::Image {
            id: "i1".into(),
            layer: String::new(),
            x,
            y,
            w,
            h,
            rotation,
            blob: BLOB.into(),
        })
    }

    #[test]
    fn image_paints_one_textured_box_turned_about_its_center() {
        let v = view(0.0, 0.0, 2.0);
        let slots = ImageSlots::from([(BLOB.to_owned(), 7)]);
        let doc = doc_with(vec![image(0.0, 0.0, 20.0, 10.0, 90.0)], &v);
        let got = document_prims(&doc, &v, &slots, &no_sheet(), EDGE, None).prims;
        // One instance: the SDF box carries the texture, so the turn, the
        // rounded corners and the antialiasing come from the same field.
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].kind, KIND_IMAGE);
        assert_eq!(got[0].slot, 7);
        // World 20x10 at zoom 2 is 40x20 px, drawn from the viewport center.
        assert_close4(got[0].geom, [50.0, 50.0, 40.0, 20.0]);
        assert_eq!(got[0].angle, std::f32::consts::FRAC_PI_2);
        // Nothing tints it: the texel passes through as it is.
        assert_eq!(got[0].color, WHITE);
        // And it maps onto the whole sheet, not a slice of one.
        assert_eq!(got[0].uv, WHOLE);
    }

    #[test]
    fn a_glyph_samples_its_own_cell_and_takes_the_ink() {
        let cell = [0.25, 0.5, 0.3, 0.6];
        let ink = [0.1, 0.2, 0.3, 1.0];
        let g = Prim::glyph(sr(4.0, 8.0, 6.0, 12.0), cell, 3, ink);
        assert_eq!(g.kind, KIND_IMAGE);
        assert_eq!(g.uv, cell);
        assert_eq!(g.slot, 3);
        // The atlas is white, so the prim's color is what reaches the eye.
        assert_eq!(g.color, ink);
        // Glyphs are upright and square-cornered: the box is only a window.
        assert_eq!(g.angle, 0.0);
        assert_eq!(g.radius, 0.0);
    }

    #[test]
    fn flat_prims_map_onto_the_whole_texture() {
        // Slot 0 is the 1x1 white stand-in: a partial map would sample
        // outside it and the clamp would hide the mistake.
        assert_eq!(Prim::rect(sr(0.0, 0.0, 1.0, 1.0), WHITE).uv, WHOLE);
        assert_eq!(Prim::segment((0.0, 0.0), (1.0, 1.0), 1.0, WHITE).uv, WHOLE);
    }

    #[test]
    fn image_whose_texture_is_not_loaded_yet_paints_a_placeholder() {
        // Decoding happens off the frame path; until the texture lands the
        // element still has to occupy its box.
        let v = view(0.0, 0.0, 1.0);
        let doc = doc_with(vec![image(0.0, 0.0, 20.0, 10.0, 0.0)], &v);
        let got = document_prims(&doc, &v, &ImageSlots::new(), &no_sheet(), EDGE, None).prims;
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].kind, KIND_BOX);
        assert_eq!(got[0].color, PLACEHOLDER_COLOR);
        assert_close4(got[0].geom, [50.0, 50.0, 20.0, 10.0]);
    }

    fn img_prim(slot: u32) -> Prim {
        Prim::image(sr(0.0, 0.0, 1.0, 1.0), (0.5, 0.5), 0.0, slot)
    }

    #[test]
    fn prims_without_an_image_are_one_run() {
        let prims = [Prim::rect(sr(0.0, 0.0, 1.0, 1.0), WHITE); 3];
        assert_eq!(
            runs(&prims),
            vec![Run {
                start: 0,
                end: 3,
                slot: 0
            }]
        );
    }

    #[test]
    fn nothing_to_draw_is_no_runs() {
        assert_eq!(runs(&[]), vec![]);
    }

    #[test]
    fn plain_prims_ride_along_with_the_image_around_them() {
        // Only images sample, so a run breaks on a second texture, never
        // on the flat prims between.
        let flat = Prim::rect(sr(0.0, 0.0, 1.0, 1.0), WHITE);
        let prims = [flat, img_prim(4), flat, img_prim(4), flat];
        assert_eq!(
            runs(&prims),
            vec![Run {
                start: 0,
                end: 5,
                slot: 4
            }]
        );
    }

    #[test]
    fn a_papered_dab_names_the_sheet_it_reads_however_plain_it_is() {
        // A round dab is a flat box until it is dragged over a paper,
        // and then it reads the sheet like any glyph: a run of nothing
        // but those has to bind the sheet, or the paper is sampled from
        // whatever texture happened to be there and never bites.
        let sheet = one_paper("canvas");
        let v = view(0.0, 0.0, 1.0);
        let tip = dragged(20.0, ROUND, 400.0, 1.0);
        let dabs = stroke_prims(&[[0.0, 0.0], [60.0, 0.0]], &tip, &no_pen(), WHITE, &v, &sheet);
        assert!(dabs.len() > 1, "a stroke of dabs");
        assert!(dabs.iter().all(|d| d.kind == KIND_BOX), "still plain boxes");
        assert_eq!(
            runs(&dabs),
            vec![Run {
                start: 0,
                end: dabs.len() as u32,
                slot: sheet.slot,
            }]
        );
    }

    #[test]
    fn a_second_texture_breaks_the_run_at_its_own_prim() {
        // Paint order is the document's; the cut lands exactly where the
        // texture changes, so what was drawn before stays underneath.
        let flat = Prim::rect(sr(0.0, 0.0, 1.0, 1.0), WHITE);
        let prims = [img_prim(1), flat, img_prim(2), flat, img_prim(1)];
        assert_eq!(
            runs(&prims),
            vec![
                Run {
                    start: 0,
                    end: 2,
                    slot: 1
                },
                Run {
                    start: 2,
                    end: 4,
                    slot: 2
                },
                Run {
                    start: 4,
                    end: 5,
                    slot: 1
                },
            ]
        );
    }

    #[test]
    fn to_hex_is_the_inverse_of_parse_color() {
        assert_eq!(to_hex([0.0, 0.0, 0.0, 1.0]), "#000000");
        assert_eq!(to_hex([1.0, 1.0, 1.0, 1.0]), "#ffffff");
        let c = parse_color("#3b82f6");
        assert_eq!(parse_color(&to_hex(c)), c, "a colour survives the round trip");
    }

    #[test]
    fn a_clipped_prim_is_only_painted_where_the_clip_lets_it() {
        let r = ScreenRect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
        let cut = ScreenRect { x: 0.0, y: 0.0, w: 40.0, h: 100.0 };
        let whole = Prim::rect(r, [0.0, 0.0, 0.0, 1.0]);
        let clipped = whole.clipped(cut);
        assert!(whole.painted_bounds().w > 100.0, "the ramp grows it");
        assert!(
            clipped.painted_bounds().w < whole.painted_bounds().w,
            "the cut shrinks it"
        );
        assert!(
            cut.contains_rect(&clipped.painted_bounds()),
            "and never reaches past the cut"
        );
    }

    #[test]
    fn a_prim_cut_away_entirely_paints_nothing() {
        let r = ScreenRect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 };
        let elsewhere = ScreenRect { x: 500.0, y: 500.0, w: 10.0, h: 10.0 };
        let p = Prim::rect(r, [0.0, 0.0, 0.0, 1.0]).clipped(elsewhere);
        let b = p.painted_bounds();
        assert_eq!((b.w, b.h), (0.0, 0.0));
    }

    #[test]
    fn an_uncut_prim_is_painted_as_it_always_was() {
        let r = ScreenRect { x: 3.0, y: 4.0, w: 10.0, h: 10.0 };
        let p = Prim::rect(r, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(p.clip, NO_CLIP);
        assert_eq!(p.painted_bounds(), r.inset(-1.5));
    }

    /// A frame spanning (0,0)–(100,100) in world units, with a rect on
    /// its inner layer running well past its right edge.
    fn framed_doc() -> Document {
        Document::from_json(
            r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "layers": [ { "id": "fl", "name": "Frame 1", "kind": "frame" } ],
            "elements": [
                { "id": "fr", "type": "frame", "layer": "fl",
                  "x": 0, "y": 0, "w": 100, "h": 100, "background": "#ff0000",
                  "layers": [ { "id": "in", "name": "Layer 1" } ] },
                { "id": "inside", "type": "rect", "layer": "in",
                  "x": 50, "y": 50, "w": 400, "h": 20,
                  "stroke": null, "fill": "#00ff00", "text": null }
            ]
        }"##,
        )
        .unwrap()
    }

    const EDGE: Rgba = [0.4, 0.4, 0.4, 1.0];

    #[test]
    fn a_frame_lays_its_ground_and_then_its_edge() {
        let doc = framed_doc();
        let v = view(0.0, 0.0, 1.0);
        let f = document_prims(&doc, &v, &ImageSlots::new(), &no_sheet(), EDGE, None);
        assert_eq!(
            f.prims[0].color,
            parse_color("#ff0000"),
            "the ground goes down first"
        );
        assert!(
            f.prims[1..5].iter().all(|p| p.color == EDGE),
            "then four hairline edges"
        );
    }

    /// A frame with no background is still there: the edge is what makes
    /// an empty area visible, and hittable.
    #[test]
    fn a_frame_with_no_ground_still_shows_its_edge() {
        let mut doc = framed_doc();
        let Element::Frame(fr) = &mut doc.elements[0] else {
            panic!("not a frame");
        };
        fr.background = None;
        let v = view(0.0, 0.0, 1.0);
        let f = document_prims(&doc, &v, &ImageSlots::new(), &no_sheet(), EDGE, None);
        assert_eq!(f.prims[0].color, EDGE);
    }

    #[test]
    fn what_a_frame_holds_is_cut_to_its_boundary() {
        let doc = framed_doc();
        let v = view(0.0, 0.0, 1.0);
        let f = document_prims(&doc, &v, &ImageSlots::new(), &no_sheet(), EDGE, None);
        let cut = frame_rect(doc.frame("fr").unwrap(), &v);
        let content = f
            .prims
            .iter()
            .find(|p| p.color == parse_color("#00ff00"))
            .expect("the rect inside is painted");
        assert_eq!(
            content.clip,
            [cut.x, cut.y, cut.w, cut.h],
            "the content carries the boundary as its clip"
        );
    }

    #[test]
    fn a_frames_own_prims_are_not_cut_by_itself() {
        let doc = framed_doc();
        let v = view(0.0, 0.0, 1.0);
        let f = document_prims(&doc, &v, &ImageSlots::new(), &no_sheet(), EDGE, None);
        assert!(
            f.prims[..5].iter().all(|p| p.clip == NO_CLIP),
            "a frame is not inside itself"
        );
    }

    #[test]
    fn what_is_not_in_a_frame_is_not_cut() {
        let v = view(0.0, 0.0, 1.0);
        let doc = doc_with(vec![rect(0.0, 0.0, 10.0, 10.0, None, Some("#000"))], &v);
        let f = document_prims(&doc, &v, &ImageSlots::new(), &no_sheet(), EDGE, None);
        assert!(
            f.prims.iter().all(|p| p.clip == NO_CLIP),
            "an element on the open board carries no cut"
        );
    }

    /// The stroke in progress wears the cut it is going to land under:
    /// its layer names the frame, so the live ink and the ink it becomes
    /// cannot be under different boundaries.
    #[test]
    fn the_live_stroke_is_cut_by_the_frame_its_layer_is_in() {
        let doc = framed_doc();
        let v = view(0.0, 0.0, 1.0);
        let tip = Tip::PENCIL;
        let blue = [0.0, 0.0, 1.0, 1.0];
        let prims = vec![Prim::rect(
            ScreenRect { x: 400.0, y: 400.0, w: 10.0, h: 10.0 },
            blue,
        )];
        let live = Live { layer: "in", prims, tip: &tip };
        let f = document_prims(
            &doc,
            &v,
            &ImageSlots::new(),
            &no_sheet(),
            EDGE,
            Some(live),
        );
        let cut = frame_rect(doc.frame("fr").unwrap(), &v);
        let drawn = f
            .prims
            .iter()
            .find(|p| p.color == blue)
            .expect("the live stroke is painted");
        assert_eq!(drawn.clip, [cut.x, cut.y, cut.w, cut.h]);
    }

    /// And a stroke on the open board is not cut at all.
    #[test]
    fn a_live_stroke_on_the_board_wears_no_cut() {
        let v = view(0.0, 0.0, 1.0);
        let doc = doc_with(Vec::new(), &v);
        let tip = Tip::PENCIL;
        let blue = [0.0, 0.0, 1.0, 1.0];
        let prims = vec![Prim::rect(
            ScreenRect { x: 4.0, y: 4.0, w: 10.0, h: 10.0 },
            blue,
        )];
        let layer = doc.layers[0].id.clone();
        let live = Live { layer: &layer, prims, tip: &tip };
        let f = document_prims(
            &doc,
            &v,
            &ImageSlots::new(),
            &no_sheet(),
            EDGE,
            Some(live),
        );
        assert_eq!(f.prims[0].clip, NO_CLIP);
    }

    #[test]
    fn a_frame_appended_keeps_its_groups_and_sheets_on_its_own_prims() {
        let r = |x: f32| ScreenRect { x, y: 0.0, w: 4.0, h: 4.0 };
        let mut a = Frame::new();
        a.group(vec![Prim::rect(r(0.0), [1.0; 4]), Prim::rect(r(8.0), [1.0; 4])], 0.5, Blend::Union, Blend::Over);
        let mut b = Frame::new();
        b.sheet(|f| f.group(vec![Prim::rect(r(20.0), [1.0; 4])], 0.5, Blend::Union, Blend::Over));
        a.append(b);
        assert_eq!(a.prims.len(), 3);
        let spans: Vec<(u32, u32)> = a.groups.iter().map(|g| (g.start, g.end)).collect();
        assert_eq!(spans, [(0, 2), (2, 3)]);
        assert_eq!(a.sheets.iter().map(|s| (s.start, s.end)).collect::<Vec<_>>(), [(2, 3)]);
        assert_eq!(a.prims[2].bounds(), r(20.0));
    }

    #[test]
    fn a_frame_cut_to_a_box_draws_nothing_outside_it() {
        let mut f = Frame::new();
        f.extend([Prim::rect(ScreenRect { x: 0.0, y: 0.0, w: 50.0, h: 50.0 }, [1.0; 4])]);
        let cell = ScreenRect { x: 10.0, y: 10.0, w: 20.0, h: 20.0 };
        f.cut(cell);
        assert_eq!(f.prims[0].clip, Prim::rect(cell, [1.0; 4]).clipped(cell).clip);
    }
}
