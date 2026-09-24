//! The layers panel's thumbnails: one sheet holding a picture of every
//! raster and vector layer whose row is on show, each in a cell of its
//! own, taken once the board is at rest. Pure — `app` says which layers
//! are on show, and draws the frame this lays out onto the sheet `gfx`
//! keeps.

use crate::doc::{BlendMode, Camera, Document, Layer};
use crate::geom::Point;
use crate::scene::{Prim, ScreenRect, View, Viewport, parse_color};

/// Cells to a row of the sheet.
const COLS: usize = 8;
/// How far in from its cell's edges a picture stands, in px.
const PAD: f64 = 2.0;
/// The checker's squares, in px, and its two greys: what shows through
/// where nothing is drawn, as every pixel editor shows it.
const SQUARE: f32 = 6.0;
const LIGHT: &str = "#ffffff";
const DARK: &str = "#cccccc";

/// Where the thumbnails are: a cell each, in rows of [`COLS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sheet {
    /// A cell's size in px.
    cell: (u32, u32),
    /// The layers pictured, a cell each, in this order.
    ids: Vec<String>,
}

impl Sheet {
    pub fn new(ids: Vec<String>, cell: (u32, u32)) -> Sheet {
        Sheet { cell, ids }
    }

    /// The layers pictured, a cell each, in this order.
    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    /// Its size in px: no wider than its cells need, and never nothing.
    pub fn size(&self) -> (u32, u32) {
        let n = self.ids.len();
        if n == 0 {
            return (1, 1);
        }
        let (cols, rows) = (n.min(COLS) as u32, n.div_ceil(COLS) as u32);
        (cols * self.cell.0.max(1), rows * self.cell.1.max(1))
    }

    /// Cell `i`, in the sheet's px.
    pub fn rect(&self, i: usize) -> ScreenRect {
        let (cw, ch) = (self.cell.0 as f32, self.cell.1 as f32);
        ScreenRect {
            x: (i % COLS) as f32 * cw,
            y: (i / COLS) as f32 * ch,
            w: cw,
            h: ch,
        }
    }

    /// Where layer `id`'s picture is on the sheet, as a prim samples it.
    pub fn uv(&self, id: &str) -> Option<[f32; 4]> {
        let i = self.ids.iter().position(|x| x == id)?;
        let r = self.rect(i);
        let (w, h) = self.size();
        let (w, h) = (w as f32, h as f32);
        Some([r.x / w, r.y / h, (r.x + r.w) / w, (r.y + r.h) / h])
    }

    /// The view that draws world box `content` into cell `i`, as large
    /// as fits a pad in from its edges and centred in it — on the whole
    /// sheet, whose other cells a cut keeps it out of.
    pub fn view(&self, i: usize, (lo, hi): (Point, Point)) -> View {
        let r = self.rect(i);
        let (w, h) = self.size();
        let span = |a: f64, b: f64| (b - a).max(f64::MIN_POSITIVE);
        let zoom = ((f64::from(r.w) - 2.0 * PAD) / span(lo[0], hi[0]))
            .min((f64::from(r.h) - 2.0 * PAD) / span(lo[1], hi[1]))
            .max(f64::MIN_POSITIVE);
        let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
        let cell = [f64::from(r.x + r.w / 2.0), f64::from(r.y + r.h / 2.0)];
        View {
            camera: Camera {
                x: centre[0] - (cell[0] - f64::from(w) / 2.0) / zoom,
                y: centre[1] - (cell[1] - f64::from(h) / 2.0) / zoom,
                zoom,
            },
            viewport: Viewport { w, h },
            scale: 1.0,
        }
    }

    /// What cell `i` shows under its picture: a checker, the squares at
    /// its far edges cut to it.
    pub fn checker(&self, i: usize) -> Vec<Prim> {
        let r = self.rect(i);
        let (light, dark) = (parse_color(LIGHT), parse_color(DARK));
        let mut out = Vec::new();
        let mut y = 0.0;
        let mut row = 0;
        while y < r.h {
            let mut x = 0.0;
            let mut col = 0;
            while x < r.w {
                let square = ScreenRect {
                    x: r.x + x,
                    y: r.y + y,
                    w: SQUARE.min(r.w - x),
                    h: SQUARE.min(r.h - y),
                };
                out.push(Prim::rect(square, if (row + col) % 2 == 0 { light } else { dark }));
                x += SQUARE;
                col += 1;
            }
            y += SQUARE;
            row += 1;
        }
        out
    }
}

/// What a layer's thumbnail is taken of: the layer alone with what stands
/// on it, shown, normal and whole — a thumbnail says what the layer
/// holds, and how it is composited is the bar's to say.
pub fn subject(doc: &Document, id: &str) -> Document {
    let layers = doc
        .layer(id)
        .map(|l| Layer {
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            locked: false,
            layers: Vec::new(),
            ..l.clone()
        })
        .into_iter()
        .collect();
    let elements = doc
        .elements
        .iter()
        .filter(|el| el.layer() == id)
        .cloned()
        .collect();
    Document {
        layers,
        elements,
        ..Document::new(&doc.title)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{BlendMode, Document};
    use crate::scene::KIND_BOX;

    fn ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("id{i}")).collect()
    }

    #[test]
    fn a_sheet_lays_its_cells_in_rows_of_eight() {
        let sheet = Sheet::new(ids(10), (64, 48));
        assert_eq!(sheet.size(), (512, 96));
        assert_eq!(sheet.rect(9), ScreenRect { x: 64.0, y: 48.0, w: 64.0, h: 48.0 });
        assert_eq!(sheet.uv("id9"), Some([0.125, 0.5, 0.25, 1.0]));
        assert_eq!(sheet.uv("nope"), None);
        assert_eq!(Sheet::new(ids(3), (64, 48)).size(), (192, 48), "no wider than it needs");
        assert_eq!(Sheet::new(Vec::new(), (64, 48)).size(), (1, 1));
    }

    #[test]
    fn a_cell_fits_its_content_centred_a_pad_in() {
        let sheet = Sheet::new(ids(2), (64, 48));
        let content = ([0.0, 0.0], [100.0, 50.0]);
        let view = sheet.view(0, content);
        let near = |(x, y): (f64, f64), (ex, ey): (f64, f64)| (x - ex).abs() < 1e-9 && (y - ey).abs() < 1e-9;
        // 60 by 44 inside the pad: the content's width is what fits.
        assert!(near(view.world_to_screen(0.0, 0.0), (2.0, 9.0)));
        assert!(near(view.world_to_screen(100.0, 50.0), (62.0, 39.0)));
        let second = sheet.view(1, content);
        assert!(near(second.world_to_screen(0.0, 0.0), (66.0, 9.0)), "in its own cell");
        assert_eq!(second.viewport, view.viewport, "the sheet's own size");
    }

    #[test]
    fn the_checker_covers_its_cell_in_two_greys() {
        let sheet = Sheet::new(ids(2), (64, 48));
        let cell = sheet.rect(1);
        let squares = sheet.checker(1);
        let area: f32 = squares.iter().map(|q| q.bounds().w * q.bounds().h).sum();
        assert_eq!(area, cell.w * cell.h);
        assert!(squares.iter().all(|q| q.kind == KIND_BOX && cell.contains_rect(&q.bounds())));
        let mut greys: Vec<[u32; 4]> = squares.iter().map(|q| q.color.map(f32::to_bits)).collect();
        greys.sort_unstable();
        greys.dedup();
        assert_eq!(greys.len(), 2);
    }

    #[test]
    fn a_thumbnail_is_taken_of_the_layer_alone_and_as_it_is_drawn() {
        let mut doc = crate::tree::tests::nested();
        let b = doc.layer_mut("B").unwrap();
        b.visible = false;
        b.opacity = 0.3;
        b.blend = BlendMode::Screen;
        let sub = subject(&doc, "B");
        assert_eq!(sub.layers.len(), 1);
        let l = &sub.layers[0];
        assert_eq!((l.id.as_str(), l.visible, l.opacity, l.blend), ("B", true, 1.0, BlendMode::Normal));
        assert_eq!(sub.elements.iter().map(|e| e.id()).collect::<Vec<_>>(), ["b"]);
        assert!(subject(&Document::new("t"), "nope").elements.is_empty());
    }
}
