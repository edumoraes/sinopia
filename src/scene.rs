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
}

pub const KIND_BOX: u32 = 0;
pub const KIND_SEGMENT: u32 = 1;

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
    pub _pad: u32,
}

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
            _pad: 0,
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
        Prim {
            geom: [a.0, a.1, b.0, b.1],
            color,
            radius: half_width,
            feather: 0.0,
            kind: KIND_SEGMENT,
            _pad: 0,
        }
    }

    /// Painted area, ignoring the antialiasing ramp. Test-only until
    /// hit-testing needs it.
    #[cfg(test)]
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
            ScreenRect {
                x: a,
                y: b,
                w: c,
                h: d,
            }
        }
    }
}

/// Polyline in screen px → one round-capped segment per span; overlapping
/// caps make the joins. Repeated points are skipped; a degenerate polyline
/// is still visible as a dot.
pub fn polyline_prims(points: &[(f32, f32)], half_width: f32, color: Rgba) -> Vec<Prim> {
    let Some(&first) = points.first() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut prev = first;
    for &p in &points[1..] {
        if p == prev {
            continue;
        }
        out.push(Prim::segment(prev, p, half_width, color));
        prev = p;
    }
    if out.is_empty() {
        out.push(Prim::circle(first.0, first.1, half_width, color));
    }
    out
}

/// Half of a pen width in screen px. Ink scales with zoom but never drops
/// below one pixel, so zoomed-out strokes stay visible.
fn half_width_px(width: f64, view: &View) -> f32 {
    ((width * view.px_per_world()) as f32 / 2.0).max(0.5)
}

/// Stroke in progress (a raw polyline in world units) → screen prims.
pub fn stroke_prims(points: &[[f64; 2]], width: f64, color: Rgba, view: &View) -> Vec<Prim> {
    let screen: Vec<(f32, f32)> = points
        .iter()
        .map(|[x, y]| {
            let (sx, sy) = view.world_to_screen(*x, *y);
            (sx as f32, sy as f32)
        })
        .collect();
    polyline_prims(&screen, half_width_px(width, view), color)
}

/// How far the flattened polyline may stray from the curve, in px.
const FLATTEN_TOLERANCE_PX: f64 = 0.25;

/// Committed `path` (cubics in world units) → screen prims. The control
/// points are projected first — Béziers are affine-invariant — so the
/// flattening tolerance is in pixels whatever the zoom.
pub fn path_prims(curves: &[Cubic], width: f64, color: Rgba, view: &View) -> Vec<Prim> {
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
    polyline_prims(&screen, half_width_px(width, view), color)
}

/// Flattens the document into prims in paint order. Rects paint fill first,
/// then the four outline edges (constant px thickness, aligned inwards);
/// paths become strokes.
pub fn document_prims(doc: &Document, view: &View) -> Vec<Prim> {
    let mut out = Vec::new();
    for element in &doc.elements {
        match element {
            Element::Rect(r) => {
                let (sx, sy) = view.world_to_screen(r.x, r.y);
                let (sx, sy) = (sx as f32, sy as f32);
                let sw = (r.w * view.px_per_world()) as f32;
                let sh = (r.h * view.px_per_world()) as f32;

                let fill = r.fill.as_deref().map(parse_color);
                let stroke = r.stroke.as_deref().map(parse_color);

                // An element with no color at all still has to show up.
                let fill = fill.or(if stroke.is_none() {
                    Some(FALLBACK_COLOR)
                } else {
                    None
                });
                if let Some(color) = fill {
                    out.push(Prim::rect(
                        ScreenRect {
                            x: sx,
                            y: sy,
                            w: sw,
                            h: sh,
                        },
                        color,
                    ));
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
                        out.push(Prim::rect(ScreenRect { x, y, w, h }, color));
                    }
                }
            }
            Element::Path(p) => {
                out.extend(path_prims(&p.curves, p.width, parse_color(&p.stroke), view));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Camera, Path, Rect};

    const VP: Viewport = Viewport { w: 100, h: 100 };
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
        d
    }

    fn rect(x: f64, y: f64, w: f64, h: f64, stroke: Option<&str>, fill: Option<&str>) -> Element {
        Element::Rect(Rect {
            id: "el".into(),
            x,
            y,
            w,
            h,
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

    #[test]
    fn fill_only_rect_becomes_one_box() {
        let v = view(0.0, 0.0, 1.0);
        let doc = doc_with(vec![rect(0.0, 0.0, 10.0, 10.0, None, Some("#fff"))], &v);
        assert_eq!(
            document_prims(&doc, &v),
            vec![Prim::rect(sr(50.0, 50.0, 10.0, 10.0), WHITE)]
        );
    }

    #[test]
    fn stroke_only_rect_becomes_four_inner_edges() {
        let v = view(0.0, 0.0, 1.0);
        let doc = doc_with(vec![rect(10.0, 10.0, 20.0, 20.0, Some("#fff"), None)], &v);
        let t = STROKE_PX;
        assert_eq!(
            document_prims(&doc, &v),
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
        let got = document_prims(&doc, &v);
        assert_eq!(got.len(), 5);
        assert_eq!(got[0].color, WHITE, "fill first");
        assert_eq!(got[1].color, [0.0, 0.0, 0.0, 1.0], "stroke after");
    }

    #[test]
    fn zoom_scales_rect_position_and_size() {
        let v = view(0.0, 0.0, 2.0);
        let doc = doc_with(vec![rect(1.0, 0.0, 5.0, 5.0, None, Some("#fff"))], &v);
        let got = document_prims(&doc, &v);
        assert_eq!(got[0].geom, [52.0, 50.0, 10.0, 10.0]);
    }

    #[test]
    fn rect_without_any_color_still_paints_with_fallback() {
        let v = view(0.0, 0.0, 1.0);
        let doc = doc_with(vec![rect(0.0, 0.0, 4.0, 4.0, None, None)], &v);
        let got = document_prims(&doc, &v);
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
                curves: vec![[[0.0, 0.0], [3.0, 0.0], [6.0, 0.0], [9.0, 0.0]]],
                stroke: "#000".into(),
                width: 2.0,
            })],
            &v,
        );
        // Width is in world units: 2 * zoom 2 = 4px wide → half-width 2.
        assert_eq!(
            document_prims(&doc, &v),
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
        let at_1x = path_prims(&[c], 2.0, WHITE, &view(0.0, 0.0, 1.0));
        assert!(at_1x.len() > 1, "a curve is more than one segment");
        let first = at_1x[0].geom;
        let last = at_1x[at_1x.len() - 1].geom;
        assert_eq!((first[0], first[1]), (50.0, 50.0));
        assert_eq!((last[2], last[3]), (150.0, 150.0));
        // Flattening tolerance is in pixels, so zooming in adds segments.
        let at_4x = path_prims(&[c], 2.0, WHITE, &view(0.0, 0.0, 4.0));
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
        let got = path_prims(&[a, b], 2.0, WHITE, &view(0.0, 0.0, 1.0));
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0].geom, [50.0, 50.0, 59.0, 50.0]);
        assert_eq!(got[1].geom, [59.0, 50.0, 68.0, 50.0]);
    }

    #[test]
    fn stroke_width_never_drops_below_one_pixel() {
        let v = view(0.0, 0.0, 0.1);
        let got = stroke_prims(&[[0.0, 0.0], [100.0, 0.0]], 2.0, WHITE, &v);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].radius, 0.5);
    }
}
