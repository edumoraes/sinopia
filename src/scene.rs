//! Cena → dados de render, sem tocar GPU (testável puro).
//!
//! Convenção de câmera: `camera.x/y` é o ponto do MUNDO que aparece no centro
//! da viewport; `zoom` multiplica mundo → pixels de tela.
//! `screen = (world - camera) * zoom + viewport/2`

use bytemuck::{Pod, Zeroable};

use crate::doc::{Camera, Document, Element};

/// Viewport em pixels físicos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    pub w: u32,
    pub h: u32,
}

/// Espessura de stroke no MVP, em pixels de tela (constante sob zoom).
pub const STROKE_PX: f32 = 2.0;

/// Cor de fallback (cinza) para cor ausente ou hex inválido — documento com
/// lixo não derruba o render.
pub const FALLBACK_COLOR: [f32; 4] = [0.5, 0.5, 0.5, 1.0];

/// Instância de retângulo sólido pronta para a GPU: origem e tamanho em
/// pixels de tela (origem no canto superior esquerdo), cor RGBA linear.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct RectInstance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub color: [f32; 4],
}

pub fn world_to_screen(cam: &Camera, vp: Viewport, wx: f64, wy: f64) -> (f64, f64) {
    (
        (wx - cam.x) * cam.zoom + f64::from(vp.w) / 2.0,
        (wy - cam.y) * cam.zoom + f64::from(vp.h) / 2.0,
    )
}

#[allow(dead_code)] // inverso usado por hit-test/pan quando houver input
pub fn screen_to_world(cam: &Camera, vp: Viewport, sx: f64, sy: f64) -> (f64, f64) {
    (
        (sx - f64::from(vp.w) / 2.0) / cam.zoom + cam.x,
        (sy - f64::from(vp.h) / 2.0) / cam.zoom + cam.y,
    )
}

/// Cor CSS-hex (`#rgb` ou `#rrggbb`) → RGBA linear. Inválida → fallback.
pub fn parse_color(hex: &str) -> [f32; 4] {
    let Some(s) = hex.strip_prefix('#') else {
        return FALLBACK_COLOR;
    };
    let expanded: Vec<u8> = match s.len() {
        3 => s.bytes().flat_map(|b| [b, b]).collect(),
        6 => s.bytes().collect(),
        _ => return FALLBACK_COLOR,
    };
    let mut rgb = [0.0f32; 3];
    for (i, pair) in expanded.chunks_exact(2).enumerate() {
        let Ok(text) = std::str::from_utf8(pair) else {
            return FALLBACK_COLOR;
        };
        let Ok(byte) = u8::from_str_radix(text, 16) else {
            return FALLBACK_COLOR;
        };
        rgb[i] = srgb_to_linear(f32::from(byte) / 255.0);
    }
    [rgb[0], rgb[1], rgb[2], 1.0]
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Achata o documento em instâncias de retângulo na ordem de pintura:
/// fill primeiro, depois as quatro arestas do stroke (espessura constante
/// em px de tela, alinhadas para dentro).
pub fn rect_instances(doc: &Document, vp: Viewport) -> Vec<RectInstance> {
    let mut out = Vec::new();
    for element in &doc.elements {
        let Element::Rect(r) = element;
        let (sx, sy) = world_to_screen(&doc.camera, vp, r.x, r.y);
        let (sx, sy) = (sx as f32, sy as f32);
        let sw = (r.w * doc.camera.zoom) as f32;
        let sh = (r.h * doc.camera.zoom) as f32;

        let fill = r.fill.as_deref().map(parse_color);
        let stroke = r.stroke.as_deref().map(parse_color);

        // Elemento sem cor nenhuma ainda precisa aparecer na tela.
        if let Some(color) = fill.or(if stroke.is_none() {
            Some(FALLBACK_COLOR)
        } else {
            None
        }) {
            out.push(RectInstance {
                pos: [sx, sy],
                size: [sw, sh],
                color,
            });
        }
        if let Some(color) = stroke {
            let t = STROKE_PX;
            let inner_h = (sh - 2.0 * t).max(0.0);
            out.push(RectInstance {
                pos: [sx, sy],
                size: [sw, t],
                color,
            });
            out.push(RectInstance {
                pos: [sx, sy + sh - t],
                size: [sw, t],
                color,
            });
            out.push(RectInstance {
                pos: [sx, sy + t],
                size: [t, inner_h],
                color,
            });
            out.push(RectInstance {
                pos: [sx + sw - t, sy + t],
                size: [t, inner_h],
                color,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Camera, Rect};

    const VP: Viewport = Viewport { w: 100, h: 100 };

    fn cam(x: f64, y: f64, zoom: f64) -> Camera {
        Camera { x, y, zoom }
    }

    fn doc_with(elements: Vec<Element>, camera: Camera) -> Document {
        let mut d = Document::new("t");
        d.camera = camera;
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

    #[test]
    fn world_origin_lands_on_viewport_center() {
        assert_eq!(
            world_to_screen(&cam(0.0, 0.0, 1.0), VP, 0.0, 0.0),
            (50.0, 50.0)
        );
    }

    #[test]
    fn camera_pan_and_zoom_transform_points() {
        // Câmera olhando (10, 20) com zoom 2: o próprio (10,20) fica no centro…
        let c = cam(10.0, 20.0, 2.0);
        assert_eq!(world_to_screen(&c, VP, 10.0, 20.0), (50.0, 50.0));
        // …e um ponto 5 unidades à direita aparece 10px à direita do centro.
        assert_eq!(world_to_screen(&c, VP, 15.0, 20.0), (60.0, 50.0));
    }

    #[test]
    fn screen_to_world_inverts_world_to_screen() {
        let c = cam(-3.5, 8.0, 2.5);
        for (wx, wy) in [(0.0, 0.0), (10.0, -4.0), (123.25, 7.5)] {
            let (sx, sy) = world_to_screen(&c, VP, wx, wy);
            let (bx, by) = screen_to_world(&c, VP, sx, sy);
            assert!(
                (bx - wx).abs() < 1e-9 && (by - wy).abs() < 1e-9,
                "({wx},{wy})"
            );
        }
    }

    #[test]
    fn parses_hex_colors_to_linear_rgba() {
        assert_eq!(parse_color("#000"), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(parse_color("#ffffff"), [1.0, 1.0, 1.0, 1.0]);
        // #222 em sRGB: 0x22/255 ≈ 0.1333 → linear ≈ 0.0159963.
        let c = parse_color("#222");
        assert!((c[0] - 0.0159963).abs() < 1e-4, "{c:?}");
        assert_eq!(c[0], c[1]);
        assert_eq!(c[1], c[2]);
        assert_eq!(c[3], 1.0);
        // #rgb expande por dígito duplicado: #7aa == #77aaaa.
        assert_eq!(parse_color("#7aa"), parse_color("#77aaaa"));
    }

    #[test]
    fn invalid_colors_fall_back_to_gray() {
        for bad in ["", "#", "#12", "#12345", "red", "#gggggg"] {
            assert_eq!(parse_color(bad), FALLBACK_COLOR, "cor {bad:?}");
        }
    }

    #[test]
    fn fill_only_rect_becomes_one_instance() {
        let doc = doc_with(
            vec![rect(0.0, 0.0, 10.0, 10.0, None, Some("#fff"))],
            cam(0.0, 0.0, 1.0),
        );
        let got = rect_instances(&doc, VP);
        assert_eq!(
            got,
            vec![RectInstance {
                pos: [50.0, 50.0],
                size: [10.0, 10.0],
                color: [1.0, 1.0, 1.0, 1.0]
            }]
        );
    }

    #[test]
    fn stroke_only_rect_becomes_four_inner_edges() {
        let doc = doc_with(
            vec![rect(10.0, 10.0, 20.0, 20.0, Some("#fff"), None)],
            cam(0.0, 0.0, 1.0),
        );
        let got = rect_instances(&doc, VP);
        let white = [1.0, 1.0, 1.0, 1.0];
        let t = STROKE_PX;
        assert_eq!(
            got,
            vec![
                // topo, base, esquerda, direita — alinhadas para dentro.
                RectInstance {
                    pos: [60.0, 60.0],
                    size: [20.0, t],
                    color: white
                },
                RectInstance {
                    pos: [60.0, 80.0 - t],
                    size: [20.0, t],
                    color: white
                },
                RectInstance {
                    pos: [60.0, 60.0 + t],
                    size: [t, 20.0 - 2.0 * t],
                    color: white
                },
                RectInstance {
                    pos: [80.0 - t, 60.0 + t],
                    size: [t, 20.0 - 2.0 * t],
                    color: white
                },
            ]
        );
    }

    #[test]
    fn fill_and_stroke_paint_fill_first() {
        let doc = doc_with(
            vec![rect(0.0, 0.0, 10.0, 10.0, Some("#000"), Some("#fff"))],
            cam(0.0, 0.0, 1.0),
        );
        let got = rect_instances(&doc, VP);
        assert_eq!(got.len(), 5);
        assert_eq!(got[0].color, [1.0, 1.0, 1.0, 1.0], "fill primeiro");
        assert_eq!(got[1].color, [0.0, 0.0, 0.0, 1.0], "stroke depois");
    }

    #[test]
    fn zoom_scales_position_and_size() {
        let doc = doc_with(
            vec![rect(1.0, 0.0, 5.0, 5.0, None, Some("#fff"))],
            cam(0.0, 0.0, 2.0),
        );
        let got = rect_instances(&doc, VP);
        assert_eq!(got[0].pos, [52.0, 50.0]);
        assert_eq!(got[0].size, [10.0, 10.0]);
    }

    #[test]
    fn rect_without_any_color_still_paints_with_fallback() {
        let doc = doc_with(
            vec![rect(0.0, 0.0, 4.0, 4.0, None, None)],
            cam(0.0, 0.0, 1.0),
        );
        let got = rect_instances(&doc, VP);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].color, FALLBACK_COLOR);
    }
}
