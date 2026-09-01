//! Dotted background: a grid fixed in world space (it pans and zooms with
//! the camera) drawn as screen-space dots of constant size. Spacing doubles
//! as the camera zooms out so the dots never crowd into noise.

use crate::scene::{Prim, Rgba, View};

/// Grid pitch in world units (logical px at zoom 1).
pub const BASE_SPACING: f64 = 24.0;
/// Below this on-screen pitch (logical px) the spacing doubles.
pub const MIN_SPACING_PX: f64 = 14.0;
/// Dot radius in logical px.
pub const DOT_RADIUS: f64 = 1.25;

/// World-space pitch for this view: `BASE_SPACING * 2^k`, the smallest k
/// that keeps the on-screen pitch at or above `MIN_SPACING_PX`.
pub fn spacing(view: &View) -> f64 {
    let ppw = view.px_per_world();
    let mut s = BASE_SPACING;
    if ppw <= 0.0 || !ppw.is_finite() {
        return s;
    }
    while s * ppw < MIN_SPACING_PX * view.scale {
        s *= 2.0;
    }
    s
}

/// Every dot that touches the viewport, row-major.
pub fn prims(view: &View, color: Rgba) -> Vec<Prim> {
    let ppw = view.px_per_world();
    if ppw <= 0.0 || !ppw.is_finite() {
        return Vec::new();
    }
    let s = spacing(view);
    let radius = DOT_RADIUS * view.scale;
    let (w, h) = (f64::from(view.viewport.w), f64::from(view.viewport.h));
    // Grid indices whose dots can still overlap the screen.
    let (wx0, wy0) = view.screen_to_world(-radius, -radius);
    let (wx1, wy1) = view.screen_to_world(w + radius, h + radius);
    let (i0, i1) = ((wx0 / s).ceil() as i64, (wx1 / s).floor() as i64);
    let (j0, j1) = ((wy0 / s).ceil() as i64, (wy1 / s).floor() as i64);
    let mut out = Vec::with_capacity(((i1 - i0 + 1) * (j1 - j0 + 1)).max(0) as usize);
    for j in j0..=j1 {
        for i in i0..=i1 {
            let (sx, sy) = view.world_to_screen(i as f64 * s, j as f64 * s);
            out.push(Prim::circle(sx as f32, sy as f32, radius as f32, color));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Camera;
    use crate::scene::{Prim, Viewport};

    const GRAY: [f32; 4] = [0.5, 0.5, 0.5, 1.0];

    fn view(x: f64, y: f64, zoom: f64, scale: f64) -> View {
        View {
            camera: Camera { x, y, zoom },
            viewport: Viewport { w: 100, h: 100 },
            scale,
        }
    }

    fn centers(prims: &[Prim]) -> Vec<(f32, f32)> {
        prims.iter().map(|p| p.bounds().center()).collect()
    }

    #[test]
    fn dots_sit_on_world_multiples_of_the_spacing() {
        let v = view(0.0, 0.0, 1.0, 1.0);
        let got = prims(&v, GRAY);
        // World -48..48 in steps of 24 → screen 2, 26, 50, 74, 98 on both axes.
        assert_eq!(got.len(), 25, "{:?}", centers(&got));
        assert!(centers(&got).contains(&(50.0, 50.0)));
        assert!(centers(&got).contains(&(2.0, 98.0)));
        assert!(got.iter().all(|p| p.radius == DOT_RADIUS as f32));
        assert!(got.iter().all(|p| p.color == GRAY));
    }

    #[test]
    fn grid_follows_the_camera() {
        let v = view(10.0, -4.0, 1.0, 1.0);
        let got = centers(&prims(&v, GRAY));
        // World origin now shows at (40, 54); the grid moves with it.
        assert!(got.contains(&(40.0, 54.0)), "{got:?}");
        assert!(got.contains(&(64.0, 30.0)), "{got:?}");
    }

    #[test]
    fn spacing_doubles_as_the_camera_zooms_out() {
        assert_eq!(spacing(&view(0.0, 0.0, 1.0, 1.0)), BASE_SPACING);
        assert_eq!(spacing(&view(0.0, 0.0, 0.5, 1.0)), BASE_SPACING * 2.0);
        assert_eq!(spacing(&view(0.0, 0.0, 0.25, 1.0)), BASE_SPACING * 4.0);
        assert_eq!(spacing(&view(0.0, 0.0, 4.0, 1.0)), BASE_SPACING);
    }

    #[test]
    fn scale_factor_keeps_dots_in_logical_pixels() {
        let v = view(0.0, 0.0, 1.0, 2.0);
        let got = prims(&v, GRAY);
        assert!(got.iter().all(|p| p.radius == (DOT_RADIUS * 2.0) as f32));
        // 24 world units are 48 physical px on a 2x display: 2..98 → 3 per axis.
        assert_eq!(got.len(), 9, "{:?}", centers(&got));
        assert_eq!(spacing(&v), BASE_SPACING);
    }

    #[test]
    fn degenerate_zoom_yields_no_dots() {
        assert!(prims(&view(0.0, 0.0, 0.0, 1.0), GRAY).is_empty());
        assert!(prims(&view(0.0, 0.0, f64::NAN, 1.0), GRAY).is_empty());
    }
}
