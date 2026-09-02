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

use bytemuck::{Pod, Zeroable};

use crate::brush::Tip;
use crate::curve::{self, Cubic};
use crate::doc::{Camera, Document, Element};

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

fn srgb_to_linear(c: f32) -> f32 {
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

pub fn with_alpha(c: Rgba, alpha: f32) -> Rgba {
    [c[0], c[1], c[2], alpha]
}

/// Axis-aligned rectangle in screen px.
#[derive(Debug, Clone, Copy, PartialEq)]
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

/// Which texture slot the renderer has uploaded for each blob hash. An
/// image the renderer has not caught up with yet is missing from the map.
pub type ImageSlots = std::collections::HashMap<String, u32>;

/// Multiplies the sampled texel: an image passes through untouched.
const NO_TINT: Rgba = [1.0, 1.0, 1.0, 1.0];

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
    /// [`KIND_IMAGE`] only: which texture to sample. Read on the CPU, to
    /// pick the bind group — the shader never sees it.
    pub slot: u32,
}

/// A clip that cuts nothing: what every prim carries until it is put
/// inside something with an edge.
pub const NO_CLIP: [f32; 4] = [0.0, 0.0, 0.0, 0.0];

/// The whole texture: what anything that is not a glyph samples.
const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

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

    /// The painted area plus the edge ramp: what the shader rasterizes,
    /// and so what a wipe has to cover.
    pub fn painted_bounds(&self) -> ScreenRect {
        self.bounds().inset(-(self.feather.max(1.0) * 0.5 + 1.0))
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
        if p.kind != KIND_IMAGE {
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

/// Stroke in progress (a raw polyline in world units) → screen prims.
pub fn stroke_prims(points: &[[f64; 2]], tip: Tip, color: Rgba, view: &View) -> Vec<Prim> {
    let screen: Vec<(f32, f32)> = points
        .iter()
        .map(|[x, y]| {
            let (sx, sy) = view.world_to_screen(*x, *y);
            (sx as f32, sy as f32)
        })
        .collect();
    let (radius, feather) = soft_radius(tip.width, tip.hardness, view);
    soft_polyline_prims(&screen, radius, feather, color)
}

/// How far the flattened polyline may stray from the curve, in px.
const FLATTEN_TOLERANCE_PX: f64 = 0.25;

/// Committed `path` (cubics in world units) → screen prims. The control
/// points are projected first — Béziers are affine-invariant — so the
/// flattening tolerance is in pixels whatever the zoom.
pub fn path_prims(curves: &[Cubic], tip: Tip, color: Rgba, view: &View) -> Vec<Prim> {
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
    let (radius, feather) = soft_radius(tip.width, tip.hardness, view);
    soft_polyline_prims(&screen, radius, feather, color)
}

/// A stretch of a frame's prims that is composited as one shape: drawn
/// into the scratch texture as the union of their coverage, then laid on
/// the frame once at `opacity`. `bounds` is what the shader rasterizes
/// for them, ramp included.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Group {
    pub start: u32,
    pub end: u32,
    pub opacity: f32,
    pub bounds: ScreenRect,
}

/// Everything on screen: the prims in paint order, and which stretches
/// of them are composited as groups. Groups never overlap and come in
/// order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Frame {
    pub prims: Vec<Prim>,
    pub groups: Vec<Group>,
}

impl Frame {
    pub fn new() -> Frame {
        Frame::default()
    }

    /// Prims drawn straight onto the frame.
    pub fn extend(&mut self, prims: impl IntoIterator<Item = Prim>) {
        self.prims.extend(prims);
    }

    /// Prims composited as one shape at `opacity`. Nothing to draw makes
    /// no group.
    pub fn group(&mut self, prims: Vec<Prim>, opacity: f32) {
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
            bounds,
        });
    }

    /// A stroke's prims, direct or grouped as its tip demands.
    pub fn stroke(&mut self, prims: Vec<Prim>, tip: Tip) {
        if tip.is_direct() {
            self.extend(prims);
        } else {
            self.group(prims, tip.opacity as f32);
        }
    }

    /// `other` painted after everything here.
    pub fn append(&mut self, other: Frame) {
        let offset = self.prims.len() as u32;
        self.prims.extend(other.prims);
        self.groups.extend(other.groups.into_iter().map(|g| Group {
            start: g.start + offset,
            end: g.end + offset,
            ..g
        }));
    }
}

/// One render pass of a frame, over the prims [`passes`] hands back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// Onto the window: `composite` first, if there is one — the group
    /// the previous pass drew offscreen — then `start..end` in order.
    /// The first pass of a frame clears the window; the rest load it.
    Direct {
        composite: Option<u32>,
        start: u32,
        end: u32,
    },
    /// Onto the scratch texture: the `wipe` box first, which clears the
    /// group's bounds, then `start..end` with the union blend.
    Offscreen { wipe: u32, start: u32, end: u32 },
}

/// The compositing plan for `frame`: its prims with one wipe and one
/// composite box appended per group, and the passes to draw them in. A
/// group whose bounds miss the viewport is dropped, prims and all — no
/// pass covers them. Always begins with a `Direct` pass, so there is
/// one to clear the window with.
pub fn passes(frame: &Frame, viewport: Viewport, scratch: u32) -> (Vec<Prim>, Vec<Pass>) {
    let window = ScreenRect {
        x: 0.0,
        y: 0.0,
        w: viewport.w as f32,
        h: viewport.h as f32,
    };
    let mut prims = frame.prims.clone();
    let mut passes = Vec::new();
    let mut cursor = 0u32;
    let mut pending: Option<u32> = None;
    let direct = |passes: &mut Vec<Pass>, composite: Option<u32>, start: u32, end: u32| {
        if passes.is_empty() || composite.is_some() || end > start {
            passes.push(Pass::Direct {
                composite,
                start,
                end,
            });
        }
    };
    for g in &frame.groups {
        direct(&mut passes, pending.take(), cursor, g.start);
        if let Some(bounds) = g.bounds.intersect(&window) {
            let bounds = bounds.snapped();
            let wipe = prims.len() as u32;
            prims.push(Prim::rect(bounds, [0.0; 4]));
            passes.push(Pass::Offscreen {
                wipe,
                start: g.start,
                end: g.end,
            });
            pending = Some(prims.len() as u32);
            prims.push(Prim::composite(bounds, viewport, scratch, g.opacity));
        }
        cursor = g.end;
    }
    direct(&mut passes, pending.take(), cursor, frame.prims.len() as u32);
    (prims, passes)
}

/// Flattens the document into a frame in paint order — the layers'
/// order, then document order within a layer, hidden layers left out.
/// Rects paint fill first, then the four outline edges (constant px
/// thickness, aligned inwards), all turned about the rect center by its
/// rotation; paths become strokes, direct or composited as their tip
/// demands; images become one textured box each, from `images`.
pub fn document_prims(doc: &Document, view: &View, images: &ImageSlots) -> Frame {
    let mut frame = Frame::new();
    for (_, element) in doc.painted() {
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
            Element::Path(p) => {
                let tip = Tip::of(p);
                frame.stroke(path_prims(&p.curves, tip, parse_color(&p.stroke), view), tip);
                continue;
            }
            // A paint is one object made of many strokes: each is drawn
            // with the ink it was laid with, and composited on its own —
            // painting twice over the same place darkens it, as pixels do.
            Element::Paint(p) => {
                for s in &p.strokes {
                    let tip = Tip::of_stroke(s);
                    frame.stroke(path_prims(&s.curves, tip, parse_color(&s.stroke), view), tip);
                }
                continue;
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
                out.push(match images.get(&i.blob) {
                    Some(&slot) => Prim::image(r, pivot, angle, slot),
                    // Decoding happens off the frame path; until the
                    // texture lands, the element still occupies its box.
                    None => Prim::turned(r, pivot, angle, PLACEHOLDER_COLOR),
                });
            }
        }
        frame.extend(out);
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::Tip;
    use crate::doc::{Camera, Kind, Layer, Paint, Path, Rect, Stroke};

    const VP: Viewport = Viewport { w: 100, h: 100 };

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
        assert_eq!(std::mem::size_of::<Prim>(), 84);
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
        let got = stroke_prims(&[[0.0, 0.0], [10.0, 0.0]], tip(8.0, 1.0, 0.5), WHITE, &v);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, KIND_SEGMENT);
        assert_eq!((got[0].radius, got[0].feather), (3.0, 2.0));
        // A tap is a dot, and a dot can be soft too.
        let dot = stroke_prims(&[[0.0, 0.0]], tip(8.0, 1.0, 0.0), WHITE, &v);
        assert_eq!(dot.len(), 1);
        assert_eq!(dot[0].kind, KIND_BOX);
        assert_eq!((dot[0].radius, dot[0].feather), (2.0, 4.0));
        assert_eq!(dot[0].geom, [48.0, 48.0, 4.0, 4.0]);
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
        })
    }

    fn laid(curves: Vec<Cubic>, tip: Tip) -> Stroke {
        Stroke {
            curves,
            stroke: "#000".into(),
            width: tip.width,
            opacity: tip.opacity,
            hardness: tip.hardness,
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
            strokes: vec![laid(a.clone(), Tip::PENCIL), laid(b.clone(), soft)],
            rotation: 0.0,
        });
        let together = document_prims(&doc_with(vec![paint], &v), &v, &none);
        // Two strokes in one paint draw what two paths draw: nothing
        // joins them, and the soft one is still composited on its own.
        let apart = document_prims(
            &doc_with(vec![path_of(a, Tip::PENCIL), path_of(b, soft)], &v),
            &v,
            &none,
        );
        assert_eq!(together.prims, apart.prims);
        assert_eq!(together.groups.len(), 1, "one group, for the soft stroke");
        assert_eq!(together.groups[0].opacity, 0.5);
    }

    #[test]
    fn a_hard_opaque_path_is_direct_and_a_soft_one_is_a_group() {
        let v = view(0.0, 0.0, 1.0);
        let none = ImageSlots::new();
        let direct = document_prims(&doc_with(vec![path_with(Tip::PENCIL)], &v), &v, &none);
        assert_eq!(direct.prims.len(), 1);
        assert!(direct.groups.is_empty(), "the pencil needs no compositing");

        let soft = document_prims(&doc_with(vec![path_with(tip(8.0, 1.0, 0.5))], &v), &v, &none);
        assert_eq!(soft.prims.len(), 1);
        assert_eq!(soft.groups.len(), 1);
        assert_eq!((soft.groups[0].start, soft.groups[0].end), (0, 1));
        assert_eq!(soft.groups[0].opacity, 1.0);

        let faint = document_prims(&doc_with(vec![path_with(tip(2.0, 0.5, 1.0))], &v), &v, &none);
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
        );
        assert_eq!(f.prims.len(), 3);
        assert_eq!(f.groups.len(), 1);
        let g = &f.groups[0];
        assert_eq!((g.start, g.end, g.opacity), (1, 3, 0.5));
        assert_eq!(g.bounds, sr(-5.0, -5.0, 20.0, 15.0));
        // Nothing to draw makes no group.
        f.group(vec![], 0.5);
        assert_eq!(f.groups.len(), 1);
    }

    #[test]
    fn frame_stroke_goes_direct_or_grouped_by_the_tip() {
        let prims = vec![Prim::segment((0.0, 0.0), (1.0, 0.0), 1.0, WHITE)];
        let mut f = Frame::new();
        f.stroke(prims.clone(), Tip::PENCIL);
        assert!(f.groups.is_empty());
        f.stroke(prims, tip(2.0, 0.25, 1.0));
        assert_eq!(f.groups.len(), 1);
        assert_eq!((f.groups[0].start, f.groups[0].end), (1, 2));
        assert_eq!(f.groups[0].opacity, 0.25);
    }

    #[test]
    fn frame_append_offsets_the_groups() {
        let mut a = Frame::new();
        a.extend([Prim::rect(sr(0.0, 0.0, 1.0, 1.0), WHITE); 3]);
        let mut b = Frame::new();
        b.group(vec![Prim::rect(sr(0.0, 0.0, 1.0, 1.0), WHITE)], 0.5);
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
        );
        f.extend([flat()]);
        f
    }

    #[test]
    fn passes_split_around_a_group() {
        let f = framed(20.0, 0.5);
        let (prims, plan) = passes(&f, VP, 9);
        assert_eq!(
            plan,
            vec![
                Pass::Direct {
                    composite: None,
                    start: 0,
                    end: 2
                },
                Pass::Offscreen {
                    wipe: 5,
                    start: 2,
                    end: 4
                },
                Pass::Direct {
                    composite: Some(6),
                    start: 4,
                    end: 5
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
        let (prims, plan) = passes(&f, VP, 9);
        assert_eq!(prims.len(), 5, "no wipe, no composite");
        assert_eq!(
            plan,
            vec![
                Pass::Direct {
                    composite: None,
                    start: 0,
                    end: 2
                },
                Pass::Direct {
                    composite: None,
                    start: 4,
                    end: 5
                },
            ]
        );
    }

    #[test]
    fn a_group_half_outside_is_clipped_to_the_viewport() {
        let f = framed(95.0, 1.0);
        let (prims, plan) = passes(&f, VP, 9);
        assert_eq!(plan.len(), 3);
        assert_eq!(prims[5].geom, [91.0, 16.0, 9.0, 18.0]);
        assert_eq!(prims[6].uv, [0.91, 0.16, 1.0, 0.34]);
    }

    #[test]
    fn a_frame_without_groups_is_one_direct_pass() {
        let mut f = Frame::new();
        f.extend([flat(), flat()]);
        let (prims, plan) = passes(&f, VP, 9);
        assert_eq!(prims.len(), 2);
        assert_eq!(
            plan,
            vec![Pass::Direct {
                composite: None,
                start: 0,
                end: 2
            }]
        );
        // Even an empty frame is one pass: it is what clears the window.
        let (prims, plan) = passes(&Frame::new(), VP, 9);
        assert!(prims.is_empty());
        assert_eq!(
            plan,
            vec![Pass::Direct {
                composite: None,
                start: 0,
                end: 0
            }]
        );
    }

    #[test]
    fn a_group_that_opens_the_frame_still_gets_a_clearing_pass_first() {
        let mut f = Frame::new();
        f.group(vec![Prim::segment((10.0, 10.0), (20.0, 10.0), 2.0, WHITE)], 0.5);
        let (_, plan) = passes(&f, VP, 9);
        assert_eq!(
            plan,
            vec![
                Pass::Direct {
                    composite: None,
                    start: 0,
                    end: 0
                },
                Pass::Offscreen {
                    wipe: 1,
                    start: 0,
                    end: 1
                },
                Pass::Direct {
                    composite: Some(2),
                    start: 1,
                    end: 1
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
            name: "Layer 2".into(),
            visible: true,
            kind: Kind::Raster,
        });
        // The white rect is first in `elements` but on the top layer.
        doc.elements[0].set_layer("top");
        let got = document_prims(&doc, &v, &ImageSlots::new()).prims;
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].color, [0.0, 0.0, 0.0, 1.0], "the lower layer paints first");
        assert_eq!(got[1].color, WHITE);
        doc.layers[1].visible = false;
        let got = document_prims(&doc, &v, &ImageSlots::new()).prims;
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
        let got = document_prims(&doc_with(vec![el], &v), &v, &ImageSlots::new()).prims;
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
            document_prims(&doc, &v, &ImageSlots::new()).prims,
            vec![Prim::rect(sr(50.0, 50.0, 10.0, 10.0), WHITE)]
        );
    }

    #[test]
    fn stroke_only_rect_becomes_four_inner_edges() {
        let v = view(0.0, 0.0, 1.0);
        let doc = doc_with(vec![rect(10.0, 10.0, 20.0, 20.0, Some("#fff"), None)], &v);
        let t = STROKE_PX;
        assert_eq!(
            document_prims(&doc, &v, &ImageSlots::new()).prims,
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
        let got = document_prims(&doc, &v, &ImageSlots::new()).prims;
        assert_eq!(got.len(), 5);
        assert_eq!(got[0].color, WHITE, "fill first");
        assert_eq!(got[1].color, [0.0, 0.0, 0.0, 1.0], "stroke after");
    }

    #[test]
    fn zoom_scales_rect_position_and_size() {
        let v = view(0.0, 0.0, 2.0);
        let doc = doc_with(vec![rect(1.0, 0.0, 5.0, 5.0, None, Some("#fff"))], &v);
        let got = document_prims(&doc, &v, &ImageSlots::new()).prims;
        assert_eq!(got[0].geom, [52.0, 50.0, 10.0, 10.0]);
    }

    #[test]
    fn rect_without_any_color_still_paints_with_fallback() {
        let v = view(0.0, 0.0, 1.0);
        let doc = doc_with(vec![rect(0.0, 0.0, 4.0, 4.0, None, None)], &v);
        let got = document_prims(&doc, &v, &ImageSlots::new()).prims;
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
            })],
            &v,
        );
        // Width is in world units: 2 * zoom 2 = 4px wide → half-width 2.
        assert_eq!(
            document_prims(&doc, &v, &ImageSlots::new()).prims,
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
        let at_1x = path_prims(&[c], Tip::PENCIL, WHITE, &view(0.0, 0.0, 1.0));
        assert!(at_1x.len() > 1, "a curve is more than one segment");
        let first = at_1x[0].geom;
        let last = at_1x[at_1x.len() - 1].geom;
        assert_eq!((first[0], first[1]), (50.0, 50.0));
        assert_eq!((last[2], last[3]), (150.0, 150.0));
        // Flattening tolerance is in pixels, so zooming in adds segments.
        let at_4x = path_prims(&[c], Tip::PENCIL, WHITE, &view(0.0, 0.0, 4.0));
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
        let got = path_prims(&[a, b], Tip::PENCIL, WHITE, &view(0.0, 0.0, 1.0));
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0].geom, [50.0, 50.0, 59.0, 50.0]);
        assert_eq!(got[1].geom, [59.0, 50.0, 68.0, 50.0]);
    }

    #[test]
    fn stroke_width_never_drops_below_one_pixel() {
        let v = view(0.0, 0.0, 0.1);
        let got = stroke_prims(&[[0.0, 0.0], [100.0, 0.0]], Tip::PENCIL, WHITE, &v);
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
        let got = document_prims(&doc, &v, &slots).prims;
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
        let got = document_prims(&doc, &v, &ImageSlots::new()).prims;
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
}
