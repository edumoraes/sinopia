//! A frame an agent hands over, planted on a board (§8).
//!
//! What `read` writes is what `add` takes: a document that is **one
//! frame** plus what stands on that frame's own layers, and nothing
//! else — literally what [`crate::export::sub_document`] produces for a
//! [`crate::export::Scope::Frame`]. One shape, so the round trip is
//! exact and the schema's own parse is the whole of the validation.
//!
//! Everything here is pure. The bytes behind an image are the store's
//! business, and the store is `app`'s.

use anyhow::Context as _;

use crate::doc::{Document, Element, Frame, Kind, Layer, new_id};
use crate::geom::Affine;
use crate::select;

/// How far to the right of everything a planted frame stands, in world
/// units — far enough that the two do not read as one area.
pub const GUTTER: f64 = 80.0;

/// The one frame a fragment is, checked: exactly one, on a frame layer
/// of the fragment's own stack, with every other element standing on one
/// of *that frame's* layers. Anything else is refused naming what is
/// wrong — a fragment carries a frame and what is in it, and a loose
/// object travelling beside one would have nowhere to land.
pub fn only_frame(fragment: &Document) -> anyhow::Result<&Frame> {
    let mut found = fragment.elements.iter().filter_map(|el| match el {
        Element::Frame(f) => Some(f),
        _ => None,
    });
    let frame = found
        .next()
        .context("a fragment is one frame and what stands in it; this one holds none")?;
    if let Some(second) = found.next() {
        anyhow::bail!(
            "a fragment is one frame; this one holds {:?} and {:?}",
            frame.id,
            second.id
        );
    }
    anyhow::ensure!(
        fragment
            .layers
            .iter()
            .any(|l| l.id == frame.layer && l.kind == Kind::Frame),
        "frame {:?} names layer {:?}, which is not a frame layer of the fragment",
        frame.id,
        frame.layer
    );
    let held = ids_in(&frame.layers);
    for el in &fragment.elements {
        if matches!(el, Element::Frame(_)) {
            continue;
        }
        anyhow::ensure!(
            held.iter().any(|id| *id == el.layer()),
            "element {:?} is on layer {:?}, which is not one of frame {:?}'s: \
             a fragment carries what stands in its frame and nothing beside it",
            el.id(),
            el.layer(),
            frame.id
        );
    }
    Ok(frame)
}

/// The name a fragment's frame carries: its layer's, as a frame's name
/// is everywhere else.
pub fn name_of(fragment: &Document, frame: &Frame) -> String {
    fragment
        .layers
        .iter()
        .find(|l| l.id == frame.layer)
        .map(|l| l.name.clone())
        .unwrap_or_default()
}

/// Where the board puts a frame it is handed: to the right of everything
/// already on it, aligned with the top of it. An empty board takes one
/// at the origin, which is where its camera starts.
///
/// The board picks and not the agent: an agent cannot see the board, so
/// a spot it named would land on top of work at random. Whoever wants it
/// somewhere else drags it.
pub fn spot(board: &Document) -> [f64; 2] {
    let ids: Vec<String> = board.elements.iter().map(|e| e.id().to_owned()).collect();
    match select::frame_of(board, &ids) {
        None => [0.0, 0.0],
        Some(f) => {
            let (lo, hi) = f.aabb();
            [hi[0] + GUTTER, lo[1]]
        }
    }
}

/// A frame worked out but not yet on the board.
///
/// The split is what lets everything that can refuse a fragment run
/// before a byte of it is committed: the images an agent hands over go
/// into the store on the way in, and a graft that is then turned down
/// would leave bytes there that nothing on the board names — which the
/// store has no way to collect.
pub struct Planned {
    stem: Layer,
    elements: Vec<Element>,
    id: String,
    name: String,
}

/// The frame [`planned`] worked out, on the board. On top: a frame an
/// agent hands over arrives over the work that is already there, never
/// under it.
pub fn apply(board: &mut Document, planned: Planned) -> (String, String) {
    board.layers.push(planned.stem);
    board.elements.extend(planned.elements);
    (planned.id, planned.name)
}

/// Everything [`plant`] does except touching the board.
pub fn planned(board: &Document, fragment: &Document) -> anyhow::Result<Planned> {
    let frame = only_frame(fragment)?;
    let name = name_of(fragment, frame);

    // Every id is minted anew: what arrives is a *new* frame, so handing
    // back the page just read plants a sibling rather than writing over
    // the frame it was read from. The id is all that is minted — the
    // rest of the layer is the fragment's own, so a frame staged hidden
    // arrives hidden, exactly as the layers inside it already do.
    let stem = Layer {
        id: new_id(),
        ..fragment
            .layers
            .iter()
            .find(|l| l.id == frame.layer)
            .cloned()
            .expect("only_frame checked the frame stands on a layer of the fragment")
    };
    let id = new_id();
    // Every layer of the frame's stack, however deep a group holds it.
    let inner: Vec<(String, String)> = ids_in(&frame.layers)
        .into_iter()
        .map(|was| (was.to_owned(), new_id()))
        .collect();

    let at = spot(board);
    let by = Affine::translate(at[0] - frame.x, at[1] - frame.y);

    // Read straight off `elements` and not through `painted()`: that one
    // skips a hidden layer, and hiding is a property that should travel
    // with a frame rather than quietly filter what it holds. Document
    // order within a layer is what `elements` already carries, and order
    // between layers is the board's own to derive.
    let mut planted: Vec<Element> = Vec::new();
    for el in &fragment.elements {
        let mut el = el.clone();
        match &mut el {
            Element::Frame(f) => {
                f.id = id.clone();
                f.layer = stem.id.clone();
                renamed(&mut f.layers, &inner);
            }
            other => {
                let to = minted(&inner, other.layer()).with_context(|| {
                    format!(
                        "element {:?} is on layer {:?}, which the frame does not have",
                        other.id(),
                        other.layer()
                    )
                })?;
                other.set_layer(&to);
                other.set_id(&new_id());
            }
        }
        select::transform(&mut el, &by);
        // The board picks the spot, so the agent's coordinates are added
        // to the board's: a fragment far enough out overflows to
        // infinity, `to_json` writes that as `null`, and the draft the
        // autosave then keeps never opens again. The check is on the
        // result because that is the invariant — what goes on the board
        // has to be able to come back off the disk. Nothing has been
        // pushed yet, so a refusal leaves the board as it was.
        anyhow::ensure!(
            placed(&el),
            "element {:?} lands outside the numbers a board can hold",
            el.id()
        );
        planted.push(el);
    }

    Ok(Planned {
        stem,
        elements: planted,
        id,
        name,
    })
}

/// Whether every number [`select::transform`] touched is still one.
/// Its field set is this one: what a map moves is what a map can move
/// out of the finite range.
pub(crate) fn placed(el: &Element) -> bool {
    let ok = |v: &f64| v.is_finite();
    match el {
        Element::Path(p) => p.curves.iter().flatten().flatten().all(ok) && ok(&p.rotation),
        Element::Paint(p) => {
            p.strokes
                .iter()
                .flat_map(|s| s.curves.iter().flatten().flatten())
                .all(ok)
                && ok(&p.rotation)
        }
        Element::Rect(r) => [r.x, r.y, r.w, r.h, r.rotation].iter().all(ok),
        Element::Image(i) => [i.x, i.y, i.w, i.h, i.rotation].iter().all(ok),
        Element::Frame(f) => [f.x, f.y, f.w, f.h].iter().all(ok),
        Element::Text(t) => [t.x, t.y, t.w, t.h, t.rotation, t.style.size].iter().all(ok),
        Element::Shape(s) => [s.x, s.y, s.w, s.h, s.rotation].iter().all(ok),
    }
}

/// Every layer id in `layers` and in the groups under them.
fn ids_in(layers: &[Layer]) -> Vec<&str> {
    layers
        .iter()
        .flat_map(|l| std::iter::once(l.id.as_str()).chain(ids_in(&l.layers)))
        .collect()
}

/// `layers`, and every group's under them, given the ids they were
/// minted as.
fn renamed(layers: &mut [Layer], pairs: &[(String, String)]) {
    for l in layers {
        l.id = minted(pairs, &l.id).expect("every inner layer was just minted");
        renamed(&mut l.layers, pairs);
    }
}

/// The id a layer of the fragment was minted as.
fn minted(pairs: &[(String, String)], was: &str) -> Option<String> {
    pairs
        .iter()
        .find(|(from, _)| from == was)
        .map(|(_, to)| to.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`planned`] and [`apply`] in one call. Production keeps them
    /// apart — the images go into the store between the two — but every
    /// test here is about what lands, not about when.
    fn plant(board: &mut Document, fragment: &Document) -> anyhow::Result<(String, String)> {
        let planned = planned(board, fragment)?;
        Ok(apply(board, planned))
    }
    use crate::doc::Rect;
    use crate::export::{self, Scope};

    fn rect(id: &str, layer: &str, x: f64, y: f64, w: f64, h: f64) -> Element {
        Element::Rect(Rect {
            id: id.into(),
            layer: layer.into(),
            x,
            y,
            w,
            h,
            rotation: 0.0,
            stroke: None,
            fill: Some("#eee".into()),
            text: None,
        })
    }

    /// A fragment: one frame 100×50 at the origin, named "Auth Flow",
    /// holding one rect 10 in from its top left.
    fn fragment() -> Document {
        let mut doc = Document::new("page");
        doc.layers = vec![Layer {
            id: "fl".into(),
            ..Layer::of("Auth Flow", Kind::Frame)
        }];
        doc.elements = vec![
            Element::Frame(Frame {
                id: "f1".into(),
                layer: "fl".into(),
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 50.0,
                background: Some("#ffffff".into()),
                layers: vec![Layer {
                    id: "in".into(),
                    ..Layer::of("Layer 1", Kind::Raster)
                }],
            }),
            rect("r1", "in", 10.0, 10.0, 20.0, 20.0),
        ];
        doc
    }

    /// A board with one rect spanning (0,0)–(200,100).
    fn board() -> Document {
        let mut doc = Document::new("board");
        doc.layers[0].id = "L1".into();
        doc.elements = vec![rect("b1", "L1", 0.0, 0.0, 200.0, 100.0)];
        doc
    }

    fn frame_of<'a>(doc: &'a Document, id: &str) -> &'a Frame {
        doc.frame(id).expect("a frame by that id")
    }

    /// [`fragment`] with a group in its frame's stack — `[in, g[deep]]` —
    /// and one more rect, on the layer inside the group.
    fn grouped_fragment() -> Document {
        let mut doc = fragment();
        let Element::Frame(f) = &mut doc.elements[0] else {
            unreachable!("the fragment opens with its frame")
        };
        f.layers.push(Layer {
            id: "g".into(),
            layers: vec![Layer {
                id: "deep".into(),
                ..Layer::of("Layer 2", Kind::Raster)
            }],
            ..Layer::of("Group 1", Kind::Group)
        });
        doc.elements.push(rect("r2", "deep", 40.0, 10.0, 10.0, 10.0));
        doc
    }

    #[test]
    fn what_stands_in_a_group_in_the_frame_stands_in_the_frame() {
        let doc = grouped_fragment();
        assert!(only_frame(&doc).is_ok());
    }

    #[test]
    fn every_layer_under_a_group_is_minted_anew_too() {
        let mut b = board();
        let frag = grouped_fragment();
        plant(&mut b, &frag).unwrap();
        plant(&mut b, &frag).unwrap();
        // No id is used twice, however deep it stands: the board is one
        // that parses.
        let back = Document::from_json(&b.to_json().unwrap()).unwrap();
        assert_eq!(back.elements.len(), b.elements.len());
        assert!(b.layer("deep").is_none() && b.layer("g").is_none());
        for el in &b.elements {
            assert!(b.layer(el.layer()).is_some(), "{} stands somewhere", el.id());
        }
        // And each planted group still holds its own layer.
        let groups: Vec<&Layer> = b
            .elements
            .iter()
            .filter_map(|el| match el {
                Element::Frame(f) => f.layers.iter().find(|l| l.kind == Kind::Group),
                _ => None,
            })
            .collect();
        assert_eq!(groups.len(), 2);
        assert_ne!(groups[0].layers[0].id, groups[1].layers[0].id);
    }

    #[test]
    fn a_fragment_holding_no_frame_is_refused() {
        let mut doc = Document::new("flat");
        doc.elements = vec![rect("r", "", 0.0, 0.0, 10.0, 10.0)];
        let e = only_frame(&doc).unwrap_err().to_string();
        assert!(e.contains("none"), "{e}");
    }

    #[test]
    fn a_fragment_holding_two_frames_is_refused_naming_both() {
        let mut doc = fragment();
        doc.layers.push(Layer {
            id: "fl2".into(),
            ..Layer::of("Other", Kind::Frame)
        });
        doc.elements.push(Element::Frame(Frame {
            id: "f2".into(),
            layer: "fl2".into(),
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            background: None,
            layers: vec![Layer::new("Layer 1")],
        }));
        let e = only_frame(&doc).unwrap_err().to_string();
        assert!(e.contains("f1") && e.contains("f2"), "{e}");
    }

    #[test]
    fn an_object_standing_outside_the_frame_is_refused_naming_it() {
        let mut doc = fragment();
        doc.layers.push(Layer::new("loose"));
        let loose = doc.layers.last().unwrap().id.clone();
        doc.elements.push(rect("stray", &loose, 0.0, 0.0, 5.0, 5.0));
        let e = only_frame(&doc).unwrap_err().to_string();
        assert!(e.contains("stray"), "{e}");
    }

    #[test]
    fn a_frame_on_a_layer_that_is_not_a_frame_layer_is_refused() {
        let mut doc = fragment();
        doc.layers[0].kind = Kind::Raster;
        assert!(only_frame(&doc).is_err());
    }

    #[test]
    fn a_planted_frame_keeps_its_name_and_its_size() {
        let mut board = board();
        let (id, name) = plant(&mut board, &fragment()).unwrap();
        assert_eq!(name, "Auth Flow");
        let f = frame_of(&board, &id);
        assert_eq!((f.w, f.h), (100.0, 50.0), "the size is the agent's");
        assert_eq!(
            board.layers.last().unwrap().name,
            "Auth Flow",
            "the frame's layer carries the name"
        );
    }

    #[test]
    fn a_planted_frame_stands_to_the_right_of_everything() {
        let mut board = board();
        let (id, _) = plant(&mut board, &fragment()).unwrap();
        let f = frame_of(&board, &id);
        assert_eq!(f.x, 200.0 + GUTTER, "past the right edge of the board");
        assert_eq!(f.y, 0.0, "aligned with the top of what is there");
    }

    #[test]
    fn an_empty_board_takes_a_frame_at_the_origin() {
        let mut board = Document::new("empty");
        let (id, _) = plant(&mut board, &fragment()).unwrap();
        let f = frame_of(&board, &id);
        assert_eq!((f.x, f.y), (0.0, 0.0));
    }

    #[test]
    fn what_the_frame_holds_moves_with_it() {
        let mut board = board();
        let (id, _) = plant(&mut board, &fragment()).unwrap();
        let f = frame_of(&board, &id).clone();
        let held: Vec<&Element> = board
            .painted()
            .filter(|p| p.within.is_some_and(|w| w.id == id))
            .map(|p| p.element)
            .collect();
        assert_eq!(held.len(), 1, "the rect came with it");
        let Element::Rect(r) = held[0] else {
            panic!("a rect")
        };
        assert_eq!(
            (r.x - f.x, r.y - f.y),
            (10.0, 10.0),
            "it keeps where it stood inside the frame"
        );
    }

    #[test]
    fn every_id_is_minted_anew_so_planting_twice_makes_two_frames() {
        let mut board = board();
        let frag = fragment();
        let (a, _) = plant(&mut board, &frag).unwrap();
        let (b, _) = plant(&mut board, &frag).unwrap();
        assert_ne!(a, b, "two frames, not one written over");
        assert_eq!(export::frames(&board).len(), 2);
        assert!(
            !board.elements.iter().any(|e| e.id() == "f1" || e.id() == "r1"),
            "nothing keeps the id it arrived with"
        );
    }

    #[test]
    fn a_planted_frame_lands_on_top() {
        let mut board = board();
        let (id, _) = plant(&mut board, &fragment()).unwrap();
        let last = board.painted().last().expect("something is painted");
        assert!(
            last.within.is_some_and(|w| w.id == id) || last.element.id() == id,
            "the frame an agent hands over arrives over what is there"
        );
    }

    #[test]
    fn a_board_that_took_a_graft_is_still_a_board_that_parses() {
        // The whole of the validation is the schema's own: unique layer
        // ids across both stacks, a frame layer with its frame on it, no
        // frame nested, every element naming a layer that exists.
        let mut board = board();
        plant(&mut board, &fragment()).unwrap();
        plant(&mut board, &fragment()).unwrap();
        let json = board.to_json().unwrap();
        Document::from_json(&json).expect("settle_layers accepts what was grafted");
    }

    #[test]
    fn what_read_writes_is_what_add_takes() {
        // The design's own claim, end to end: a frame exported off a
        // board is a fragment that board can be handed back.
        let mut board = board();
        let (first, _) = plant(&mut board, &fragment()).unwrap();
        let page = export::sub_document(&board, &Scope::Frame(first.clone()));
        let (second, name) = plant(&mut board, &page).unwrap();
        assert_ne!(first, second);
        assert_eq!(name, "Auth Flow", "the name survives the round trip");
        assert_eq!(export::frames(&board).len(), 2);
        assert!(
            board.frame(&first).is_some(),
            "the frame it was read from is untouched"
        );
    }

    #[test]
    fn a_hidden_layer_travels_hidden_rather_than_being_dropped() {
        // `painted()` would filter it away and the frame would arrive
        // empty with nothing said. A frame carries what it holds.
        let mut frag = fragment();
        let Element::Frame(f) = &mut frag.elements[0] else {
            panic!("a frame")
        };
        f.layers[0].visible = false;
        let mut board = board();
        let (id, _) = plant(&mut board, &frag).unwrap();
        let inner = &board.frame(&id).unwrap().layers[0];
        assert!(!inner.visible, "it is still hidden");
        assert!(
            board.elements.iter().any(|e| e.layer() == inner.id),
            "and what stood on it came along"
        );
    }

    #[test]
    fn the_spot_is_the_origin_when_there_is_nothing_on_the_board() {
        assert_eq!(spot(&Document::new("empty")), [0.0, 0.0]);
    }

    #[test]
    fn the_name_of_a_frame_is_its_layers() {
        let frag = fragment();
        let f = only_frame(&frag).unwrap();
        assert_eq!(name_of(&frag, f), "Auth Flow");
    }

    #[test]
    fn a_frame_staged_hidden_arrives_hidden() {
        // The layers *inside* a frame already travel with their
        // visibility; the frame's own layer was being rebuilt from
        // scratch, so it arrived drawn over the person's board.
        let mut frag = fragment();
        frag.layers[0].visible = false;
        let mut board = Document::new("board");
        plant(&mut board, &frag).unwrap();
        let stem = board.layers.last().unwrap();
        assert_eq!(stem.kind, Kind::Frame);
        assert_eq!(stem.name, "Auth Flow");
        assert!(!stem.visible, "hiding is the fragment's to say, not the graft's");
    }

    #[test]
    fn a_fragment_that_would_overflow_the_board_is_refused_whole() {
        // `to_json` writes a non-finite coordinate as `null`, and a
        // draft the autosave keeps that way never opens again — so the
        // refusal has to come before the board is touched at all.
        let mut frag = fragment();
        let Element::Frame(f) = &mut frag.elements[0] else {
            panic!("not a frame");
        };
        f.x = -1e308;
        frag.elements[1] = rect("far", "in", 1.5e308, 0.0, 10.0, 10.0);
        let mut board = Document::new("board");
        let err = plant(&mut board, &frag).unwrap_err().to_string();
        assert!(err.contains("outside the numbers"), "{err}");
        assert!(board.elements.is_empty(), "the board is as it was");
        assert_eq!(board.layers.len(), 1, "and so is its stack");
    }

    #[test]
    fn a_text_in_the_frame_is_grafted_with_it_moved_as_the_frame_is() {
        let mut b = board();
        let mut frag = fragment();
        if let Element::Frame(f) = &mut frag.elements[0] {
            f.layers.push(Layer {
                id: "tl".into(),
                ..Layer::of("Title", Kind::Text)
            });
        }
        frag.elements.push(Element::Text(crate::doc::Text {
            id: "t1".into(),
            layer: "tl".into(),
            x: 10.0,
            y: 5.0,
            w: 40.0,
            h: 20.0,
            rotation: 0.0,
            mode: crate::doc::TextMode::Artistic,
            text: "Label".into(),
            style: crate::doc::TextStyle::default(),
            runs: Vec::new(),
        }));
        plant(&mut b, &frag).unwrap();
        let (frame, text) = (
            b.elements.iter().find_map(|e| match e {
                Element::Frame(f) => Some(f.clone()),
                _ => None,
            }),
            b.elements.iter().find_map(|e| match e {
                Element::Text(t) => Some(t.clone()),
                _ => None,
            }),
        );
        let (frame, text) = (frame.unwrap(), text.unwrap());
        assert_ne!(text.id, "t1", "minted anew");
        assert_eq!((text.x - frame.x, text.y - frame.y), (10.0, 5.0), "where it stood in the frame");
        assert_eq!(b.layer(&text.layer).unwrap().kind, Kind::Text);
        assert!(Document::from_json(&b.to_json().unwrap()).is_ok());
    }
}
