//! Layers panel: the dock's chrome in a column on the right, one card per
//! layer, top layer first. Sized in logical px, positioned in physical
//! px, floating over the canvas and swallowing whatever it catches, with
//! a handle beside it that opens and closes it. Pure — `app` asks where a
//! click landed and what to draw.

use std::collections::HashMap;

use crate::doc::Layer;
use crate::scene::{Prim, Rgba, ScreenRect, Viewport, icon_prims, mix};
use crate::text::Atlas;
use crate::theme::Theme;

// Logical px.
pub const WIDTH: f32 = 200.0;
/// From the strip above and the window's right edge.
pub const MARGIN: f32 = 12.0;
pub const HEADER: f32 = 34.0;
pub const ROW: f32 = 34.0;
pub const PADDING: f32 = 6.0;
/// The header buttons and the eye are this square.
pub const BUTTON: f32 = 24.0;
pub const RADIUS: f32 = 12.0;
const ROW_RADIUS: f32 = 6.0;
/// A row's card sits this far inside it, so the gap between two cards is
/// twice this and a click in the gap still lands on a row.
const CARD_INSET: f32 = 2.0;
const CARD_SHADOW_OFFSET: f32 = 1.0;
const CARD_SHADOW_FEATHER: f32 = 4.0;
/// A card the pointer is carrying is further off the panel: its shadow
/// has further to fall, and its outline is thick enough to read.
const LIFT_SHADOW_OFFSET: f32 = 4.0;
const LIFT_SHADOW_FEATHER: f32 = 12.0;
const LIFT_BORDER: f32 = 2.0;
/// And it is out of the stack: this much bigger, this many degrees
/// clockwise, and this many logical px toward the canvas.
const LIFT_SCALE: f32 = 0.05;
const LIFT_TILT: f32 = 2.0;
const LIFT_LEFT: f32 = 12.0;
/// How far past the band a card in flight is allowed to reach: the lean
/// toward the canvas, plus what the growth and the turn add to a corner.
/// The band lets go by this much as the card lifts, so the lean is not
/// cut off at the panel's own edge.
const LIFT_REACH: f32 = LIFT_LEFT + WIDTH * LIFT_SCALE;
/// The scrollbar's thumb, and the room it keeps from the cards.
const BAR_W: f32 = 4.0;
const BAR_GAP: f32 = 3.0;
const BAR_MIN: f32 = 24.0;
/// How long the lift takes to come on, and to go off again.
pub const LIFT_SECONDS: f32 = 0.14;
const BUTTON_GAP: f32 = 2.0;
/// Between an icon and the label it introduces: a row's eye and its name,
/// the handle's chevron and the word under it.
const LABEL_GAP: f32 = 6.0;
/// The clickable area around the eye, past the box itself.
const EYE_SLOP: f32 = 2.0;
/// The 24-unit icon grid maps onto a box this big, centered in its button.
const ICON_BOX: f32 = 16.0;
const ICON_STROKE: f32 = 1.5;
const PUPIL: f32 = 2.5;
const SHADOW_OFFSET: f32 = 3.0;
const SHADOW_FEATHER: f32 = 14.0;
/// The handle: a tab on the header's line, standing on end. This is its
/// short side — the long one is however much it carries.
const HANDLE_W: f32 = 28.0;
/// Inside the handle, above and below what it carries.
const HANDLE_PAD: f32 = 8.0;
/// Between the handle and the panel it opens.
const HANDLE_GAP: f32 = 8.0;
const TITLE: &str = "Layers";
/// The shortcut the handle teaches, on a key of its own under the word.
const KBD: &str = "Shift+L";
/// The key: this much across the tab, this much padding at either end of
/// the letters, and corners of its own.
const KBD_W: f32 = 18.0;
const KBD_PAD: f32 = 5.0;
const KBD_RADIUS: f32 = 4.0;
/// Between the word and the key under it.
const KBD_GAP: f32 = 8.0;
/// A quarter turn counter-clockwise: what the word and the key's letters
/// take, so they read up the tab instead of across the window, their tops
/// facing the canvas the panel comes out over.
const QUARTER: f32 = -std::f32::consts::FRAC_PI_2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelHit {
    /// Make this layer the active one. The index is into the document's
    /// layers, bottom to top.
    Select(usize),
    /// Show or hide this layer.
    Toggle(usize),
    Add,
    Remove,
    Up,
    Down,
    /// Panel chrome between controls: swallowed, never reaches the canvas.
    Panel,
}

/// The card the pointer is carrying, and how far into the lift it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lift {
    /// Into the document's layers.
    pub index: usize,
    /// Where the card's top edge is asked to be, in physical px: the
    /// pointer, less the grip the card was taken by. It follows the
    /// pointer rather than the row, so the card does not jump.
    pub y: f32,
    /// The lift, 0 (sitting in its row) to 1 (fully off the panel),
    /// before easing. It runs back down to 0 when the card is let go.
    pub t: f32,
}

/// Smoothstep: the lift comes on and goes off without a corner, and
/// reverses mid-flight the same way.
pub fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A value on its way to where it belongs: how far it still has to
/// come, and how much of that is left. Everything the panel eases — a
/// card making room, the list gliding to show a row — is one of these,
/// so it all moves at the one pace.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Coming {
    /// Where it was, less where it now belongs, in physical px.
    from: f32,
    /// 1 the moment it was sent, 0 once it has arrived.
    t: f32,
}

impl Coming {
    /// Sends it `distance` further than it has already come, so
    /// something that moves again mid-flight carries what is left of
    /// the old trip into the new one instead of jumping to a new start.
    pub fn send(&mut self, distance: f32) {
        *self = Coming {
            from: distance + self.offset(),
            t: 1.0,
        };
    }

    pub fn tick(&mut self, dt: f32) {
        self.t = (self.t - dt / LIFT_SECONDS).max(0.0);
    }

    /// How far from home it still is, in physical px.
    pub fn offset(&self) -> f32 {
        self.from * ease(self.t)
    }

    pub fn moving(&self) -> bool {
        self.t > 0.0
    }
}

/// The cards on their way between two orders of the stack. A layer that
/// changed rows travels from where it was at the lift's pace, so the
/// stack opens and closes around a carried card instead of jumping.
/// Pure — `app` owns one and tells it what the stack looks like and how
/// much time has passed.
#[derive(Debug, Default, Clone)]
pub struct Slides {
    on_the_way: HashMap<String, Coming>,
    /// The order the stack was in when it was last looked at, bottom to
    /// top. A stack it has never seen starts every card still, so a tab
    /// switch does not slide a whole panel.
    seen: Vec<String>,
}

impl Slides {
    /// Takes in the stack's order; anything that changed rows starts
    /// over from where it was. `row` is one row in physical px.
    pub fn restack(&mut self, layers: &[Layer], row: f32) {
        if self.seen.len() == layers.len()
            && self.seen.iter().zip(layers).all(|(id, l)| *id == l.id)
        {
            return;
        }
        let order: Vec<String> = layers.iter().map(|l| l.id.clone()).collect();
        let seen = std::mem::replace(&mut self.seen, order);
        // Row positions run the other way: the top layer is row 0.
        let was: HashMap<&str, usize> = seen
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), seen.len() - 1 - i))
            .collect();
        for (i, layer) in layers.iter().enumerate() {
            let Some(&then) = was.get(layer.id.as_str()) else {
                continue;
            };
            let now = layers.len() - 1 - i;
            // Where it is on screen this instant, measured from the row
            // it is about to belong to: a card that moves again while
            // it is still travelling does not jump to start over.
            let step = (then as f32 - now as f32) * row;
            let mut coming = self.on_the_way.remove(&layer.id).unwrap_or_default();
            coming.send(step);
            if coming.offset() != 0.0 {
                self.on_the_way.insert(layer.id.clone(), coming);
            }
        }
    }

    /// Ages every slide by `dt` seconds; the arrived are forgotten.
    pub fn tick(&mut self, dt: f32) {
        self.on_the_way.retain(|_, coming| {
            coming.tick(dt);
            coming.moving()
        });
    }

    /// How far from its row a card still is, in physical px.
    pub fn offset(&self, id: &str) -> f32 {
        self.on_the_way.get(id).map_or(0.0, Coming::offset)
    }

    /// Something is still travelling, so the next frame will differ.
    pub fn moving(&self) -> bool {
        !self.on_the_way.is_empty()
    }
}

/// What the panel shows beyond the layers themselves: which one is
/// active, which is in the pointer's hand, and what is still moving.
pub struct Showing<'a> {
    pub active: usize,
    pub lift: Option<Lift>,
    pub slides: &'a Slides,
}

/// One layer's row, with everything already measured.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Into the document's layers.
    pub index: usize,
    /// What the pointer hits: the card and the gap under it.
    pub rect: ScreenRect,
    /// What is drawn: the rect less the gap that separates two cards.
    pub card: ScreenRect,
    pub eye: ScreenRect,
    /// The name, cut down to what fits.
    pub label: String,
    /// Where the label's pen starts.
    pub label_x: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pub rect: ScreenRect,
    pub header: ScreenRect,
    /// Where the cards are shown and cut off: as much of the stack as
    /// the window has room for.
    pub band: ScreenRect,
    /// Top layer first, and only those the band reaches.
    pub rows: Vec<Row>,
    pub up: ScreenRect,
    pub down: ScreenRect,
    pub add: ScreenRect,
    pub remove: ScreenRect,
    /// The scrollbar's thumb, when there is more stack than band.
    pub bar: Option<ScreenRect>,
    /// The scroll actually used, in physical px — what was asked for,
    /// kept inside what there is to scroll.
    scroll: f32,
    /// Every row's height together, in physical px.
    content: f32,
    scale: f32,
}

impl Panel {
    /// `top` is where the strip ends, in physical px. Rows are laid out
    /// top layer first from `scroll` px above the band, which is as much
    /// of the stack as there is room for above the bottom margin. Only
    /// the rows the band reaches are laid out; a row it reaches part of
    /// is laid out whole and cut by [`Panel::band`] when it is drawn.
    pub fn layout(
        viewport: Viewport,
        scale: f64,
        top: f32,
        atlas: &Atlas,
        layers: &[Layer],
        scroll: f32,
    ) -> Panel {
        let s = scale as f32;
        let x = (viewport.w as f32 - (MARGIN + WIDTH) * s).round();
        let y = (top + MARGIN * s).round();
        let inner_x = x + PADDING * s;
        let inner_w = (WIDTH - 2.0 * PADDING) * s;
        let header = ScreenRect {
            x: inner_x,
            y: y + PADDING * s,
            w: inner_w,
            h: HEADER * s,
        };

        // Buttons, right-aligned in the header: up, down, add, remove.
        let side = BUTTON * s;
        let by = header.y + (header.h - side) / 2.0;
        let mut bx = header.x + header.w - side;
        let mut button = || {
            let r = ScreenRect {
                x: bx,
                y: by,
                w: side,
                h: side,
            };
            bx -= side + BUTTON_GAP * s;
            r
        };
        let remove = button();
        let add = button();
        let down = button();
        let up = button();

        let band_y = header.y + header.h;
        let room = (viewport.h as f32 - (MARGIN + PADDING) * s - band_y).max(0.0);
        let content = layers.len() as f32 * ROW * s;
        let band = ScreenRect {
            x: inner_x,
            y: band_y,
            w: inner_w,
            h: content.min(room),
        };
        let scroll = scroll.clamp(0.0, (content - band.h).max(0.0));

        let mut rows = Vec::new();
        for (pos, (index, layer)) in layers.iter().enumerate().rev().enumerate() {
            let ry = band.y + pos as f32 * ROW * s - scroll;
            if ry + ROW * s <= band.y {
                continue;
            }
            if ry >= band.y + band.h {
                break;
            }
            let rect = ScreenRect {
                x: inner_x,
                y: ry,
                w: inner_w,
                h: ROW * s,
            };
            let eye = ScreenRect {
                x: rect.x + PADDING * s,
                y: rect.y + (rect.h - side) / 2.0,
                w: side,
                h: side,
            };
            let label_x = (eye.x + eye.w + LABEL_GAP * s).round();
            let gutter = if content > band.h {
                (BAR_W + BAR_GAP) * s
            } else {
                0.0
            };
            let room = rect.x + rect.w - PADDING * s - gutter - label_x;
            let label = if room > 0.0 {
                atlas.truncate(&layer.name, room)
            } else {
                String::new()
            };
            rows.push(Row {
                index,
                rect,
                card: rect.inset(CARD_INSET * s),
                eye,
                label,
                label_x,
            });
        }

        // A thumb as tall a share of the band as the band is of the
        // stack, and only when there is stack it does not reach.
        let bar = (content > band.h).then(|| {
            let h = (band.h * band.h / content).max(BAR_MIN * s).min(band.h);
            let travel = (band.h - h) * scroll / (content - band.h);
            ScreenRect {
                x: band.x + band.w - BAR_W * s,
                y: band.y + travel,
                w: BAR_W * s,
                h,
            }
        });

        let rect = ScreenRect {
            x,
            y,
            w: WIDTH * s,
            h: (2.0 * PADDING + HEADER) * s + band.h,
        };
        Panel {
            rect,
            header,
            band,
            rows,
            up,
            down,
            add,
            remove,
            bar,
            scroll,
            content,
            scale: s,
        }
    }

    /// The scroll in use, in physical px.
    pub fn scroll(&self) -> f32 {
        self.scroll
    }

    /// How far the stack can be scrolled: nothing when it all fits.
    pub fn max_scroll(&self) -> f32 {
        (self.content - self.band.h).max(0.0)
    }

    /// The scroll that brings layer `index` into the band, moving as
    /// little as it can — what the panel does when a click on the canvas
    /// makes a layer active that is out of sight. Where it already is,
    /// the answer is the scroll it already has.
    pub fn scroll_showing(&self, index: usize, layers: usize) -> f32 {
        if index >= layers {
            return self.scroll;
        }
        let row = ROW * self.scale;
        let top = (layers - 1 - index) as f32 * row;
        let want = if top < self.scroll {
            top
        } else if top + row > self.scroll + self.band.h {
            top + row - self.band.h
        } else {
            self.scroll
        };
        want.clamp(0.0, self.max_scroll())
    }

    pub fn hit(&self, x: f64, y: f64) -> Option<PanelHit> {
        if !self.rect.contains(x, y) {
            return None;
        }
        let buttons = [
            (self.up, PanelHit::Up),
            (self.down, PanelHit::Down),
            (self.add, PanelHit::Add),
            (self.remove, PanelHit::Remove),
        ];
        if let Some((_, hit)) = buttons.iter().find(|(r, _)| r.contains(x, y)) {
            return Some(*hit);
        }
        // A row reaches past the band when it is only part shown; the
        // pointer never does.
        if self.band.contains(x, y) {
            for row in &self.rows {
                if !row.rect.contains(x, y) {
                    continue;
                }
                let on_eye = row.eye.inset(-EYE_SLOP * self.scale).contains(x, y);
                return Some(if on_eye {
                    PanelHit::Toggle(row.index)
                } else {
                    PanelHit::Select(row.index)
                });
            }
        }
        Some(PanelHit::Panel)
    }

    /// Where a row dragged to `y` would land: the layer whose row is
    /// under the pointer, or the nearest one once the drag has left the
    /// list at either end, so overshooting still drops it where it was
    /// headed. `None` when no row is on show.
    pub fn drop_index(&self, y: f64) -> Option<usize> {
        let last = self.rows.last()?;
        let y = y as f32;
        let row = self.rows.iter().find(|r| y < r.rect.y + r.rect.h);
        Some(row.unwrap_or(last).index)
    }

    /// Where a carried card's top edge actually goes: what the pointer
    /// asks for, kept inside the band. It is the band that holds it, not
    /// the rows — a row can be part shown at either edge, and a card
    /// riding one out of the panel is not what a lift looks like.
    fn free_y(&self, y: f32) -> f32 {
        let inset = CARD_INSET * self.scale;
        let top = self.band.y + inset;
        let bottom = (self.band.y + self.band.h - inset - (ROW * self.scale - 2.0 * inset)).max(top);
        y.clamp(top, bottom)
    }

    /// Paint order: shadow, border, panel, the title and the buttons,
    /// then a card per row — and last, over the cards it is passing, the
    /// one `showing.lift` names.
    pub fn prims(
        &self,
        layers: &[Layer],
        showing: &Showing,
        atlas: &Atlas,
        slot: u32,
        theme: &Theme,
    ) -> Vec<Prim> {
        let s = self.scale;
        let mut out = vec![
            Prim::soft(
                self.rect.offset(0.0, SHADOW_OFFSET * s),
                RADIUS * s,
                SHADOW_FEATHER * s,
                theme.shadow,
            ),
            Prim::rounded(self.rect.inset(-s), RADIUS * s + s, theme.border),
            Prim::rounded(self.rect, RADIUS * s, theme.panel),
        ];
        let baseline = atlas.baseline_in(self.header);
        for g in atlas.layout(TITLE, self.header.x + PADDING * s, baseline) {
            out.push(Prim::glyph(g.rect, g.uv, slot, theme.ink));
        }
        for (rect, icon) in [
            (self.up, UP),
            (self.down, DOWN),
            (self.add, PLUS),
            (self.remove, TRASH),
        ] {
            out.extend(icon_prims(icon, rect, 24.0, ICON_BOX, ICON_STROKE, s, theme.icon));
        }
        let carried = showing.lift.map(|l| l.index);
        for row in self.rows.iter().filter(|r| Some(r.index) != carried) {
            let dy = layers
                .get(row.index)
                .map_or(0.0, |l| showing.slides.offset(&l.id));
            self.card_prims(row, layers, showing.active, None, dy, atlas, slot, theme, &mut out);
        }
        // A card in the hand is where the pointer put it, not where the
        // stack says: it takes no slide.
        if let Some(l) = showing.lift
            && let Some(row) = self.rows.iter().find(|r| r.index == l.index)
        {
            let a = showing.active;
            self.card_prims(row, layers, a, Some(l), 0.0, atlas, slot, theme, &mut out);
        }
        // The thumb sits over the cards, at the band's right edge: it
        // says how much of the stack is on show and where.
        if let Some(bar) = self.bar {
            out.push(Prim::rounded(bar, bar.w / 2.0, theme.muted));
        }
        out
    }

    /// One row's card: its shadow, its outline, its body, then the eye
    /// and the name. A card in flight is the same card, every property
    /// carried `lift` of the way: further off the panel, bluer at the
    /// edge, and — once it is drawn — bigger, turned and leaning at the
    /// pointer, contents and all.
    #[allow(clippy::too_many_arguments)]
    fn card_prims(
        &self,
        row: &Row,
        layers: &[Layer],
        active: usize,
        lift: Option<Lift>,
        dy: f32,
        atlas: &Atlas,
        slot: u32,
        theme: &Theme,
        out: &mut Vec<Prim>,
    ) {
        let s = self.scale;
        let radius = ROW_RADIUS * s;
        let e = lift.map_or(0.0, |l| ease(l.t));
        let at = |rest: f32, flight: f32| rest + (flight - rest) * e;
        let (drop, feather, edge) = (
            at(CARD_SHADOW_OFFSET, LIFT_SHADOW_OFFSET),
            at(CARD_SHADOW_FEATHER, LIFT_SHADOW_FEATHER),
            at(1.0, LIFT_BORDER),
        );
        let outline = mix(theme.border, theme.lifted, e);
        let is_active = row.index == active;
        let start = out.len();
        out.push(Prim::soft(
            row.card.offset(0.0, drop * s),
            radius,
            feather * s,
            theme.shadow,
        ));
        out.push(Prim::rounded(
            row.card.inset(-edge * s),
            radius + edge * s,
            outline,
        ));
        out.push(Prim::rounded(
            row.card,
            radius,
            if is_active { theme.active_bg } else { theme.panel },
        ));
        let Some(layer) = layers.get(row.index) else {
            return;
        };
        let (eye, color) = if layer.visible {
            (EYE, theme.icon)
        } else {
            (EYE_HIDDEN, theme.muted)
        };
        out.extend(icon_prims(eye, row.eye, 24.0, ICON_BOX, ICON_STROKE, s, color));
        if layer.visible {
            let (cx, cy) = row.eye.center();
            out.push(Prim::circle(cx, cy, PUPIL / 24.0 * ICON_BOX * s, color));
        }
        if !row.label.is_empty() {
            let ink = if is_active { theme.ink } else { theme.icon };
            let baseline = atlas.baseline_in(row.rect);
            for g in atlas.layout(&row.label, row.label_x, baseline) {
                out.push(Prim::glyph(g.rect, g.uv, slot, ink));
            }
        }
        // Out of the stack in the pointer's hand, or still on its way to
        // the row it now belongs to. Either way the whole card moves,
        // contents and all.
        let (k, angle, by) = match lift {
            Some(l) if e > 0.0 => (
                1.0 + LIFT_SCALE * e,
                LIFT_TILT.to_radians() * e,
                (-LIFT_LEFT * s * e, (self.free_y(l.y) - row.card.y) * e),
            ),
            _ => (1.0, 0.0, (0.0, dy)),
        };
        let pivot = row.card.center();
        let moved = k != 1.0 || angle != 0.0 || by.0 != 0.0 || by.1 != 0.0;
        // The band is where the stack is shown, and the rest of a row
        // that reaches past it is not drawn — but a card in flight is
        // out of the stack, so the band lets go of it as it lifts.
        let cut = self.band.inset(-LIFT_REACH * s * e);
        for prim in &mut out[start..] {
            if moved {
                *prim = prim.transformed(pivot, k, angle, by);
            }
            *prim = prim.clipped(cut);
        }
    }
}

/// The panel's handle: a tab on the header's line that pulls the panel
/// out and puts it back. Closed it hangs off the window's right edge,
/// standing on end: the chevron on the header's line, and under it the
/// panel's word turned a quarter turn counter-clockwise so it reads up
/// the tab, then the shortcut on a key of its own. The word makes the panel
/// findable without knowing `Shift+L`; the key is how it stops being
/// needed. Open it steps aside to the panel's left, a chevron alone — the
/// header behind it already says "Layers", and a shortcut is only worth
/// teaching for the door that is shut.
#[derive(Debug, Clone, PartialEq)]
pub struct Handle {
    pub rect: ScreenRect,
    /// True while the panel is up: the chevron points back.
    pub open: bool,
    chevron: ScreenRect,
    /// Where the word's pen starts, running down the tab. Absent while
    /// the panel is up, or the window too short to letter it.
    word_y: Option<f32>,
    /// The key under the word, when the window has room for it too.
    kbd: Option<ScreenRect>,
    scale: f32,
}

impl Handle {
    /// `top` is where the strip ends, in physical px — the line the panel
    /// measures from, so the two stay level.
    pub fn layout(viewport: Viewport, scale: f64, top: f32, atlas: &Atlas, open: bool) -> Handle {
        let s = scale as f32;
        let icon = ICON_BOX * s;
        let w = HANDLE_W * s;
        let right = if open {
            (viewport.w as f32 - (MARGIN + WIDTH) * s).round() - HANDLE_GAP * s
        } else {
            (viewport.w as f32 - MARGIN * s).round()
        };
        let x = right - w;
        // The chevron is on the header's line whether the tab under it is
        // lettered or not, so a click that toggles the panel does not
        // slide the handle out from under the pointer that made it.
        let y = (top + (MARGIN + PADDING + HEADER / 2.0 - HANDLE_PAD - ICON_BOX / 2.0) * s).round();
        let chevron = ScreenRect {
            x: x + (w - icon) / 2.0,
            y: y + HANDLE_PAD * s,
            w: icon,
            h: icon,
        };
        // What the tab carries, measured down from the chevron — and what
        // the window has room for, down to the margin it keeps from its
        // own bottom edge.
        let word_y = chevron.y + icon + LABEL_GAP * s;
        let word = atlas.measure(TITLE);
        let kbd_len = 2.0 * KBD_PAD * s + atlas.measure(KBD);
        let kbd = ScreenRect {
            x: x + (w - KBD_W * s) / 2.0,
            y: word_y + word + KBD_GAP * s,
            w: KBD_W * s,
            h: kbd_len,
        };
        let end = |bottom: f32| bottom + HANDLE_PAD * s;
        let room = viewport.h as f32 - MARGIN * s;
        let (word_y, kbd, h) = if open {
            (None, None, 2.0 * HANDLE_PAD * s + icon)
        } else if end(kbd.y + kbd.h) <= room {
            (Some(word_y), Some(kbd), end(kbd.y + kbd.h) - y)
        } else if end(word_y + word) <= room {
            (Some(word_y), None, end(word_y + word) - y)
        } else {
            (None, None, 2.0 * HANDLE_PAD * s + icon)
        };
        Handle {
            rect: ScreenRect { x, y, w, h },
            open,
            chevron,
            word_y,
            kbd,
            scale: s,
        }
    }

    pub fn hit(&self, x: f64, y: f64) -> bool {
        self.rect.contains(x, y)
    }

    /// Paint order: shadow, border, body, the chevron, and — closed — the
    /// word it opens and the key that opens it.
    pub fn prims(&self, atlas: &Atlas, slot: u32, theme: &Theme) -> Vec<Prim> {
        let s = self.scale;
        let radius = self.rect.w.min(self.rect.h) / 2.0;
        let mut out = vec![
            Prim::soft(
                self.rect.offset(0.0, SHADOW_OFFSET * s),
                radius,
                SHADOW_FEATHER * s,
                theme.shadow,
            ),
            Prim::rounded(self.rect.inset(-s), radius + s, theme.border),
            Prim::rounded(self.rect, radius, theme.panel),
        ];
        let chevron = if self.open { CHEVRON_RIGHT } else { CHEVRON_LEFT };
        out.extend(icon_prims(
            chevron,
            self.chevron,
            24.0,
            ICON_BOX,
            ICON_STROKE,
            s,
            theme.icon,
        ));
        let across = (self.rect.x, self.rect.w);
        if let Some(top) = self.word_y {
            out.extend(reading_up(atlas, TITLE, across, top, slot, theme.ink));
        }
        if let Some(k) = self.kbd {
            out.push(Prim::rounded(k.inset(-s), KBD_RADIUS * s + s, theme.border));
            out.push(Prim::rounded(k, KBD_RADIUS * s, theme.active_bg));
            let top = k.y + KBD_PAD * s;
            out.extend(reading_up(atlas, KBD, (k.x, k.w), top, slot, theme.icon));
        }
        out
    }
}

/// `s` written up a tab: laid out flat, then turned [`QUARTER`] about the
/// origin and carried into place, so it reads bottom to top down a run
/// that starts at `top`, with its line centered across `across` — an
/// `(x, width)` span. The turn maps `(x, y)` to `(y, -x)`: the string's
/// own x becomes the distance back up the tab, which is why the run is
/// carried its whole measure past `top`, and the height it was laid out
/// in becomes the width it is centered across.
fn reading_up(
    atlas: &Atlas,
    s: &str,
    across: (f32, f32),
    top: f32,
    slot: u32,
    color: Rgba,
) -> Vec<Prim> {
    let flat = ScreenRect {
        x: 0.0,
        y: 0.0,
        w: atlas.measure(s),
        h: across.1,
    };
    let baseline = atlas.baseline_in(flat);
    atlas
        .layout(s, 0.0, baseline)
        .into_iter()
        .map(|g| {
            Prim::glyph(g.rect, g.uv, slot, color).transformed(
                (0.0, 0.0),
                1.0,
                QUARTER,
                (across.0, top + flat.w),
            )
        })
        .collect()
}

// Icons as polylines on a 24×24 grid, like the dock's.
const UP: &[&[(f32, f32)]] = &[&[(6.0, 15.0), (12.0, 9.0), (18.0, 15.0)]];
/// The handle's arrow: out the way the panel comes, and back.
const CHEVRON_LEFT: &[&[(f32, f32)]] = &[&[(15.0, 6.0), (9.0, 12.0), (15.0, 18.0)]];
const CHEVRON_RIGHT: &[&[(f32, f32)]] = &[&[(9.0, 6.0), (15.0, 12.0), (9.0, 18.0)]];
const DOWN: &[&[(f32, f32)]] = &[&[(6.0, 9.0), (12.0, 15.0), (18.0, 9.0)]];
const PLUS: &[&[(f32, f32)]] = &[&[(12.0, 5.0), (12.0, 19.0)], &[(5.0, 12.0), (19.0, 12.0)]];
/// A bin: lid, handle, tapered body.
const TRASH: &[&[(f32, f32)]] = &[
    &[(4.0, 7.0), (20.0, 7.0)],
    &[(9.0, 7.0), (9.0, 4.0), (15.0, 4.0), (15.0, 7.0)],
    &[(6.0, 7.0), (7.0, 20.0), (17.0, 20.0), (18.0, 7.0)],
];
/// The almond of an open eye; the pupil is a circle drawn with it.
const ALMOND: &[(f32, f32)] = &[
    (2.0, 12.0),
    (5.0, 8.0),
    (9.0, 5.5),
    (12.0, 5.0),
    (15.0, 5.5),
    (19.0, 8.0),
    (22.0, 12.0),
    (19.0, 16.0),
    (15.0, 18.5),
    (12.0, 19.0),
    (9.0, 18.5),
    (5.0, 16.0),
    (2.0, 12.0),
];
const EYE: &[&[(f32, f32)]] = &[ALMOND];
/// The same almond, struck through.
const EYE_HIDDEN: &[&[(f32, f32)]] = &[ALMOND, &[(4.0, 20.0), (20.0, 4.0)]];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{KIND_BOX, KIND_IMAGE, KIND_SEGMENT};
    use crate::tabs::Tabs;
    use crate::text::Font;

    fn atlas() -> Atlas {
        Atlas::build(&Font::bundled(), Tabs::label_px(1.0))
    }

    fn layers(n: usize) -> Vec<Layer> {
        (1..=n)
            .map(|i| Layer {
                id: format!("L{i}"),
                name: format!("Layer {i}"),
                visible: true,
            })
            .collect()
    }

    fn panel(viewport: Viewport, scale: f64, n: usize) -> Panel {
        Panel::layout(viewport, scale, 34.0 * scale as f32, &atlas(), &layers(n), 0.0)
    }

    const VP: Viewport = Viewport { w: 1200, h: 800 };

    fn sr(x: f32, y: f32, w: f32, h: f32) -> ScreenRect {
        ScreenRect { x, y, w, h }
    }

    #[test]
    fn panel_sits_below_the_strip_at_the_right_edge() {
        let p = panel(VP, 1.0, 2);
        // MARGIN from the right edge and from the strip's bottom; padding,
        // the header, one row per layer, padding.
        assert_eq!(
            p.rect,
            sr(
                1200.0 - MARGIN - WIDTH,
                34.0 + MARGIN,
                WIDTH,
                2.0 * PADDING + HEADER + 2.0 * ROW
            )
        );
    }

    #[test]
    fn rows_list_the_top_layer_first() {
        let p = panel(VP, 1.0, 3);
        let indices: Vec<usize> = p.rows.iter().map(|r| r.index).collect();
        assert_eq!(indices, vec![2, 1, 0]);
        assert_eq!(p.rows[0].label, "Layer 3");
        assert_eq!(p.rows[0].rect.y, p.header.y + p.header.h);
        assert_eq!(p.rows[1].rect.y, p.rows[0].rect.y + ROW);
        assert_eq!(p.rows[0].rect.h, ROW);
        for r in &p.rows {
            assert!(r.rect.contains_rect(&r.eye), "the eye sits in its row");
            assert!(r.label_x > r.eye.x + r.eye.w, "the label follows the eye");
        }
    }

    #[test]
    fn the_band_is_as_much_of_the_stack_as_there_is_room_for() {
        // Room for the header and one row above the bottom margin.
        let h = (34.0 + MARGIN + PADDING + HEADER + ROW + PADDING + MARGIN) as u32;
        let p = panel(Viewport { w: 1200, h }, 1.0, 3);
        assert_eq!(p.band.h, ROW);
        assert_eq!(p.rows.len(), 1, "{:?}", p.rows);
        assert_eq!(p.rows[0].index, 2, "the top layer is what the band starts on");
        assert_eq!(p.rect.h, 2.0 * PADDING + HEADER + ROW);
        assert_eq!(p.max_scroll(), 2.0 * ROW, "two rows below the band");
        assert!(p.bar.is_some(), "and a thumb to say so");

        // A pixel short, and the band keeps the row, cut off. A scroll
        // area ends mid-card; it does not drop one.
        let p = panel(Viewport { w: 1200, h: h - 1 }, 1.0, 3);
        assert_eq!(p.band.h, ROW - 1.0);
        assert_eq!(p.rows.len(), 1);
        assert!(
            p.rows[0].rect.h > p.band.h,
            "the row reaches past the band and is cut when it is drawn"
        );

        // Everything fits: no scroll, no thumb, and the band is the
        // stack.
        let p = panel(VP, 1.0, 3);
        assert_eq!(p.band.h, 3.0 * ROW);
        assert_eq!(p.max_scroll(), 0.0);
        assert!(p.bar.is_none());
    }

    #[test]
    fn scrolling_slides_the_stack_under_the_band() {
        let h = (34.0 + MARGIN + PADDING + HEADER + 2.0 * ROW + PADDING + MARGIN) as u32;
        let vp = Viewport { w: 1200, h };
        let a = atlas();
        let ls = layers(4);
        let at = |scroll: f32| Panel::layout(vp, 1.0, 34.0, &a, &ls, scroll);

        let top = at(0.0);
        assert_eq!(top.scroll(), 0.0);
        assert_eq!(indices(&top), vec![3, 2], "the top of the stack");

        // Half a row down: three rows are part shown.
        let half = at(ROW / 2.0);
        assert_eq!(indices(&half), vec![3, 2, 1]);
        assert_eq!(half.rows[0].rect.y, half.band.y - ROW / 2.0, "cut at the top");

        // Past the end, and it stops with the last row against the
        // band's bottom.
        let end = at(10_000.0);
        assert_eq!(end.scroll(), 2.0 * ROW, "two rows of stack below a two-row band");
        assert_eq!(indices(&end), vec![1, 0]);
        let last = end.rows.last().unwrap();
        assert_eq!(last.rect.y + last.rect.h, end.band.y + end.band.h);

        // The thumb is the band's share of the stack, and travels the
        // whole way.
        let thumb = top.bar.unwrap();
        assert_eq!(thumb.h, top.band.h * top.band.h / (4.0 * ROW));
        assert_eq!(thumb.y, top.band.y);
        assert_eq!(thumb.x + thumb.w, top.band.x + top.band.w);
        let thumb = end.bar.unwrap();
        assert_eq!(thumb.y + thumb.h, end.band.y + end.band.h);
    }

    #[test]
    fn scroll_showing_moves_as_little_as_it_can() {
        let h = (34.0 + MARGIN + PADDING + HEADER + 2.0 * ROW + PADDING + MARGIN) as u32;
        let vp = Viewport { w: 1200, h };
        let a = atlas();
        let ls = layers(4);
        let at = |scroll: f32| Panel::layout(vp, 1.0, 34.0, &a, &ls, scroll);

        // Looking at the top two rows (layers 3 and 2, top first).
        let p = at(0.0);
        assert_eq!(p.scroll_showing(3, 4), 0.0, "already in the band");
        assert_eq!(p.scroll_showing(2, 4), 0.0);
        assert_eq!(p.scroll_showing(1, 4), ROW, "just far enough down");
        assert_eq!(p.scroll_showing(0, 4), 2.0 * ROW);

        // Looking at the bottom two: coming back up stops as soon as the
        // row is in.
        let p = at(2.0 * ROW);
        assert_eq!(p.scroll_showing(0, 4), 2.0 * ROW);
        assert_eq!(p.scroll_showing(2, 4), ROW);
        assert_eq!(p.scroll_showing(3, 4), 0.0);
        assert_eq!(p.scroll_showing(9, 4), p.scroll(), "no such layer");

        // Nothing to scroll: the answer is always where it already is.
        let p = panel(VP, 1.0, 4);
        assert_eq!(p.scroll_showing(0, 4), 0.0);
    }

    fn indices(p: &Panel) -> Vec<usize> {
        p.rows.iter().map(|r| r.index).collect()
    }

    #[test]
    fn layout_scales_with_the_display() {
        let p = panel(VP, 2.0, 2);
        assert_eq!(p.rect.w, 2.0 * WIDTH);
        assert_eq!(p.rect.x, 1200.0 - 2.0 * (MARGIN + WIDTH));
        assert_eq!(p.rows[0].rect.h, 2.0 * ROW);
        assert_eq!(p.add.w, 2.0 * BUTTON);
    }

    #[test]
    fn hit_names_the_row_the_eye_and_the_buttons() {
        let p = panel(VP, 1.0, 2);
        let mid = |r: ScreenRect| {
            let (x, y) = r.center();
            (f64::from(x), f64::from(y))
        };
        let (x, y) = mid(p.up);
        assert_eq!(p.hit(x, y), Some(PanelHit::Up));
        let (x, y) = mid(p.down);
        assert_eq!(p.hit(x, y), Some(PanelHit::Down));
        let (x, y) = mid(p.add);
        assert_eq!(p.hit(x, y), Some(PanelHit::Add));
        let (x, y) = mid(p.remove);
        assert_eq!(p.hit(x, y), Some(PanelHit::Remove));
        let (x, y) = mid(p.rows[0].eye);
        assert_eq!(p.hit(x, y), Some(PanelHit::Toggle(1)));
        let (x, y) = mid(p.rows[1].eye);
        assert_eq!(p.hit(x, y), Some(PanelHit::Toggle(0)));
        let (x, y) = mid(p.rows[1].rect);
        assert_eq!(p.hit(x, y), Some(PanelHit::Select(0)));
        let (_, y) = mid(p.header);
        let x = f64::from(p.header.x + 10.0);
        assert_eq!(p.hit(x, y), Some(PanelHit::Panel), "the title swallows the click");
        assert_eq!(p.hit(f64::from(p.rect.x) + 1.0, f64::from(p.rect.y) + 1.0), Some(PanelHit::Panel));
        assert_eq!(p.hit(100.0, 100.0), None);
        assert_eq!(p.hit(f64::from(p.rect.x) - 1.0, f64::from(p.rect.y) + 50.0), None);
    }

    #[test]
    fn drop_index_names_the_row_under_the_pointer() {
        let p = panel(VP, 1.0, 3);
        let mid = |r: ScreenRect| f64::from(r.center().1);
        assert_eq!(p.drop_index(mid(p.rows[0].rect)), Some(2));
        assert_eq!(p.drop_index(mid(p.rows[1].rect)), Some(1));
        assert_eq!(p.drop_index(mid(p.rows[2].rect)), Some(0));
        // Past either end it is the nearest row, so a drag that
        // overshoots the list still lands where it was headed.
        assert_eq!(p.drop_index(0.0), Some(2));
        assert_eq!(p.drop_index(10_000.0), Some(0));
        // Nothing on show, nowhere to drop.
        let h = (34.0 + MARGIN + PADDING + HEADER + PADDING + MARGIN) as u32;
        let p = panel(Viewport { w: 1200, h }, 1.0, 3);
        assert!(p.rows.is_empty());
        assert_eq!(p.drop_index(100.0), None);
    }

    fn handle(viewport: Viewport, scale: f64, open: bool) -> Handle {
        Handle::layout(viewport, scale, 34.0 * scale as f32, &atlas(), open)
    }

    #[test]
    fn the_handle_hangs_off_the_panel_open_and_off_the_edge_closed() {
        let closed = handle(VP, 1.0, false);
        let open = handle(VP, 1.0, true);
        let p = panel(VP, 1.0, 2);

        // Both start on the same line and the chevron sits at the same
        // place in each, so the toggle does not make the arrow jump out
        // from under the pointer that just clicked it.
        assert_eq!(closed.rect.y, open.rect.y);
        assert_eq!(closed.chevron, open.chevron.offset(closed.rect.x - open.rect.x, 0.0));
        let (_, cy) = closed.chevron.center();
        let (_, header_y) = p.header.center();
        assert!((cy - header_y).abs() < 1.0, "on the header's line");

        // The tab stands on end: the same short side either way, and the
        // long one is whatever it carries.
        assert_eq!(closed.rect.w, HANDLE_W);
        assert_eq!(open.rect.w, HANDLE_W);
        assert_eq!(open.rect.h, 2.0 * HANDLE_PAD + ICON_BOX);
        assert!(
            closed.rect.h > open.rect.h + atlas().measure(TITLE) + atlas().measure(KBD),
            "the closed one is taller by the word and the key it carries"
        );

        // Closed there is no panel, so it takes the panel's own margin
        // from the right edge; open it steps aside and leaves it clear.
        assert_eq!(closed.rect.x + closed.rect.w, 1200.0 - MARGIN);
        assert_eq!(open.rect.x + open.rect.w, p.rect.x - HANDLE_GAP);
    }

    #[test]
    fn handle_layout_scales_with_the_display() {
        let h = handle(VP, 2.0, false);
        assert_eq!(h.rect.w, 2.0 * HANDLE_W);
        assert_eq!(h.kbd.unwrap().w, 2.0 * KBD_W);
        assert_eq!(h.chevron.w, 2.0 * ICON_BOX);
        assert_eq!(h.rect.x + h.rect.w, 1200.0 - 2.0 * MARGIN);
    }

    #[test]
    fn a_window_too_short_gives_up_the_key_and_then_the_word() {
        let full = handle(VP, 1.0, false).rect.h;
        let top = handle(VP, 1.0, false).rect.y;
        let room = |h: f32| Viewport {
            w: 1200,
            h: (top + h + MARGIN).ceil() as u32,
        };

        // Room for everything, then for everything but the key, then for
        // neither: the tab gives up its cargo before it runs off the
        // window's bottom edge.
        let all = handle(room(full), 1.0, false);
        assert_eq!(all.rect.h, full);
        assert!(all.kbd.is_some() && all.word_y.is_some());

        let no_key = handle(room(full - 1.0), 1.0, false);
        assert!(no_key.kbd.is_none(), "the key goes first");
        assert!(no_key.word_y.is_some(), "the word still says what it opens");
        assert!(no_key.rect.h < full);

        let short = room(2.0 * HANDLE_PAD + ICON_BOX);
        let bare = handle(short, 1.0, false);
        assert_eq!(bare.word_y, None);
        assert_eq!(bare.rect.h, 2.0 * HANDLE_PAD + ICON_BOX, "the chevron alone");
        assert!(bare.rect.y + bare.rect.h + MARGIN <= short.h as f32);
    }

    #[test]
    fn the_handle_takes_a_click_and_nothing_beside_it() {
        let h = handle(VP, 1.0, false);
        let (cx, cy) = h.rect.center();
        assert!(h.hit(f64::from(cx), f64::from(cy)));
        assert!(!h.hit(f64::from(h.rect.x) - 2.0, f64::from(cy)));
        assert!(!h.hit(f64::from(cx), f64::from(h.rect.y) - 2.0));
    }

    /// The x both strokes of a chevron touch — its tip.
    fn chevron_tip(prims: &[Prim]) -> f32 {
        let segs: Vec<&Prim> = prims.iter().filter(|q| q.kind == KIND_SEGMENT).collect();
        assert_eq!(segs.len(), 2, "two strokes make a chevron");
        assert_eq!(
            (segs[0].geom[2], segs[0].geom[3]),
            (segs[1].geom[0], segs[1].geom[1]),
            "they meet at the tip"
        );
        segs[0].geom[2]
    }

    #[test]
    fn the_handle_points_out_when_closed_and_back_when_open() {
        let theme = Theme::light();
        let a = atlas();

        let closed = handle(VP, 1.0, false);
        let prims = closed.prims(&a, 7, &theme);
        assert!(prims[0].feather > 0.0, "soft shadow goes first");
        assert!(
            prims
                .iter()
                .any(|q| q.color == theme.panel && q.bounds() == closed.rect),
            "handle body"
        );
        let glyphs = prims
            .iter()
            .filter(|q| q.kind == KIND_IMAGE && q.slot == 7)
            .count();
        assert_eq!(
            glyphs,
            TITLE.chars().count() + KBD.chars().count(),
            "it says what it opens, and the key that opens it"
        );
        assert!(
            chevron_tip(&prims) < closed.chevron.center().0,
            "the arrow points left, the way the panel comes out"
        );

        let open = handle(VP, 1.0, true);
        let prims = open.prims(&a, 7, &theme);
        assert!(
            !prims.iter().any(|q| q.kind == KIND_IMAGE),
            "the panel's header already says 'Layers', and a shut door \
             is the only one worth a shortcut"
        );
        assert!(
            chevron_tip(&prims) > open.chevron.center().0,
            "the arrow points back, the way the panel goes"
        );
    }

    #[test]
    fn the_word_reads_up_the_tab_with_the_shortcut_under_it() {
        let theme = Theme::light();
        let a = atlas();
        let h = handle(VP, 1.0, false);
        let prims = h.prims(&a, 7, &theme);
        let glyphs: Vec<&Prim> = prims
            .iter()
            .filter(|q| q.kind == KIND_IMAGE && q.slot == 7)
            .collect();

        // Every letter has taken the same quarter turn, and none of them
        // is left lying across the window.
        assert!(glyphs.iter().all(|q| q.angle == QUARTER), "a quarter turn");

        // Reading order runs up the tab, not across it: each letter
        // starts above the one before, and the whole word stays inside
        // the tab's own width.
        let word = &glyphs[..TITLE.chars().count()];
        for pair in word.windows(2) {
            assert!(
                pair[1].geom[1] < pair[0].geom[1],
                "'{TITLE}' reads bottom to top"
            );
        }
        for q in &glyphs {
            let b = q.bounds();
            assert!(
                h.rect.contains_rect(&b),
                "the letters stay on the tab: {b:?} in {:?}",
                h.rect
            );
        }

        // The key is a card of its own under the word, and its letters
        // are the ones left over.
        let kbd = h.kbd.expect("the window has room for the key");
        assert!(kbd.y > h.word_y.unwrap() + a.measure(TITLE), "under the word");
        assert!(
            prims.iter().any(|q| q.bounds() == kbd && q.color == theme.active_bg),
            "the key's own face"
        );
        for q in &glyphs[TITLE.chars().count()..] {
            assert!(kbd.contains_rect(&q.bounds()), "the shortcut sits on its key");
        }
    }

    #[test]
    fn every_row_is_a_card_and_the_carried_one_rises_off_the_panel() {
        let theme = Theme::light();
        let a = atlas();
        let ls = layers(3);
        let p = Panel::layout(VP, 1.0, 34.0, &a, &ls, 0.0);
        let body = |prims: &[Prim], card: ScreenRect| {
            prims.iter().position(|q| q.bounds() == card && q.feather == 0.0)
        };

        // At rest: a card inside every row, on a hairline of the border
        // color, over a shadow of its own.
        let prims = p.prims(&ls, &showing(1, None), &a, 7, &theme);
        for row in &p.rows {
            assert!(row.rect.contains_rect(&row.card), "the card sits in its row");
            let at = body(&prims, row.card);
            assert!(at.is_some(), "row {} has a body", row.index);
            assert_eq!(
                prims[at.unwrap()].clip,
                [p.band.x, p.band.y, p.band.w, p.band.h],
                "row {} is cut to the band",
                row.index
            );
            assert!(
                prims
                    .iter()
                    .any(|q| q.color == theme.border && q.bounds() == row.card.inset(-1.0)),
                "row {} is outlined",
                row.index
            );
            let shadow: Vec<&Prim> = prims
                .iter()
                .filter(|q| {
                    q.color == theme.shadow && q.bounds() == row.card.offset(0.0, CARD_SHADOW_OFFSET)
                })
                .collect();
            assert_eq!(shadow.len(), 1, "row {} casts one shadow", row.index);
            assert_eq!(shadow[0].feather, CARD_SHADOW_FEATHER);
        }
        assert!(
            !prims.iter().any(|q| q.color == theme.lifted),
            "nothing is in flight"
        );

        // Two cards are a gap apart, and the gap belongs to a row.
        assert_eq!(
            p.rows[1].card.y - (p.rows[0].card.y + p.rows[0].card.h),
            2.0 * CARD_INSET
        );

        // Carried: the middle card takes the blue, a shadow with further
        // to fall, and goes last — over the cards it is passing.
        let card = p.rows[1].card;
        let prims = p.prims(&ls, &showing(1, Some(lift(1, card.y, 1.0))), &a, 7, &theme);
        let blue: Vec<&Prim> = prims.iter().filter(|q| q.color == theme.lifted).collect();
        assert_eq!(blue.len(), 1, "one card is in flight");
        // The panel's own shadow is the first prim; the rest are cards'.
        assert!(prims[0].feather > 0.0 && prims[0].color == theme.shadow);
        let long: Vec<&Prim> = prims[1..]
            .iter()
            .filter(|q| q.color == theme.shadow && q.feather > CARD_SHADOW_FEATHER)
            .collect();
        assert_eq!(long.len(), 1, "one card's shadow has further to fall");
        let carried = carried_body(&prims, &theme);
        assert!(carried > body(&prims, p.rows[0].card).unwrap());
        assert!(carried > body(&prims, p.rows[2].card).unwrap());
    }

    fn lift(index: usize, y: f32, t: f32) -> Lift {
        Lift { index, y, t }
    }

    /// Nothing on the move: what the panel shows at rest.
    fn showing(active: usize, lift: Option<Lift>) -> Showing<'static> {
        static STILL: std::sync::LazyLock<Slides> = std::sync::LazyLock::new(Slides::default);
        Showing {
            active,
            lift,
            slides: &STILL,
        }
    }

    /// Where the carried card's body sits in the paint order. It is the
    /// only one filled with the active color, and it is no longer at its
    /// row's rect, so it cannot be found by bounds.
    fn carried_body(prims: &[Prim], theme: &Theme) -> usize {
        let at: Vec<usize> = prims
            .iter()
            .enumerate()
            .filter(|(_, q)| q.color == theme.active_bg)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(at.len(), 1, "one card is filled");
        at[0]
    }

    #[test]
    fn the_lift_eases_in_and_out() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        assert_eq!(ease(0.5), 0.5);
        assert!(ease(0.25) < 0.25, "slow off the mark");
        assert!(ease(0.75) > 0.75, "and slow into the stop");
        assert_eq!(ease(-1.0), 0.0, "clamped, so a lift cannot overshoot");
        assert_eq!(ease(2.0), 1.0);
    }

    #[test]
    fn a_carried_card_grows_turns_and_leans_toward_the_canvas() {
        let theme = Theme::light();
        let a = atlas();
        let ls = layers(3);
        let p = Panel::layout(VP, 1.0, 34.0, &a, &ls, 0.0);
        let card = p.rows[1].card;
        let (was_x, was_y) = card.center();
        let body_of = |prims: &[Prim]| prims[carried_body(prims, &theme)];

        // Fully lifted, asked to stay on its row's line.
        let full = body_of(&p.prims(&ls, &showing(1, Some(lift(1, card.y, 1.0))), &a, 7, &theme));
        assert_eq!(full.geom[2], card.w * (1.0 + LIFT_SCALE), "grown");
        assert_eq!(full.geom[3], card.h * (1.0 + LIFT_SCALE));
        assert_eq!(full.angle, LIFT_TILT.to_radians(), "turned clockwise");
        let (cx, cy) = (
            full.geom[0] + full.geom[2] / 2.0,
            full.geom[1] + full.geom[3] / 2.0,
        );
        assert_eq!(cx, was_x - LIFT_LEFT, "leaning toward the canvas");
        assert_eq!(cy, was_y, "and nowhere in y that the pointer did not ask");

        // Half of it is half of everything: the transition has no step.
        let half = body_of(&p.prims(&ls, &showing(1, Some(lift(1, card.y, 0.5))), &a, 7, &theme));
        assert_eq!(half.angle, LIFT_TILT.to_radians() * 0.5);
        assert_eq!(half.geom[2], card.w * (1.0 + LIFT_SCALE * 0.5));
        assert_eq!(half.geom[0] + half.geom[2] / 2.0, was_x - LIFT_LEFT * 0.5);

        // The card is where the pointer put it, not on any row's line.
        let moved = body_of(&p.prims(&ls, &showing(1, Some(lift(1, card.y + 11.0, 1.0))), &a, 7, &theme));
        assert_eq!(moved.geom[1] + moved.geom[3] / 2.0, was_y + 11.0);

        // Nothing of the lift is drawn at rest.
        let none = body_of(&p.prims(&ls, &showing(1, Some(lift(1, card.y + 11.0, 0.0))), &a, 7, &theme));
        assert_eq!(none.bounds(), card);
        assert_eq!(none.angle, 0.0);
    }

    #[test]
    fn the_cards_that_make_room_slide_from_where_they_were() {
        let mut ls = layers(3);
        let mut s = Slides::default();

        // A stack it has never seen is already where it belongs.
        s.restack(&ls, ROW);
        assert!(!s.moving());
        assert_eq!(s.offset("L1"), 0.0);

        // The bottom layer goes to the top: it climbs two rows, and the
        // two it passed each drop one. Every card starts from where it
        // was, so the first frame after the move looks like the last
        // frame before it.
        ls.swap(0, 1);
        ls.swap(1, 2);
        s.restack(&ls, ROW);
        assert!(s.moving());
        assert_eq!(s.offset("L1"), 2.0 * ROW, "down two rows from the top");
        assert_eq!(s.offset("L2"), -ROW);
        assert_eq!(s.offset("L3"), -ROW);

        // Halfway there is halfway back.
        s.tick(LIFT_SECONDS / 2.0);
        assert_eq!(s.offset("L1"), 2.0 * ROW * ease(0.5));
        // A card that moves again while travelling carries what is left
        // of the old trip into the new one, instead of jumping: L2 was
        // 17px short of the bottom row and is now asked for the middle
        // one, a row higher — so it starts a row below it, less what it
        // had already come.
        let carried = s.offset("L2");
        assert_eq!(carried, -ROW * ease(0.5));
        ls.swap(0, 1);
        s.restack(&ls, ROW);
        assert_eq!(s.offset("L2"), ROW + carried);

        // And they all arrive.
        s.tick(LIFT_SECONDS);
        assert!(!s.moving());
        assert_eq!(s.offset("L2"), 0.0);
    }

    #[test]
    fn a_sliding_card_is_drawn_off_its_row() {
        let theme = Theme::light();
        let a = atlas();
        let mut ls = layers(3);
        let p = Panel::layout(VP, 1.0, 34.0, &a, &ls, 0.0);
        let mut s = Slides::default();
        s.restack(&ls, ROW);
        ls.swap(1, 2);
        s.restack(&ls, ROW);

        let showing = Showing {
            active: 0,
            lift: None,
            slides: &s,
        };
        let prims = p.prims(&ls, &showing, &a, 7, &theme);
        for row in &p.rows {
            let card = row.card.offset(0.0, s.offset(&ls[row.index].id));
            assert!(
                prims.iter().any(|q| q.bounds() == card && q.feather == 0.0),
                "row {} is drawn where it is coming from",
                row.index
            );
        }
    }

    #[test]
    fn the_band_lets_go_of_a_card_in_flight() {
        let theme = Theme::light();
        let a = atlas();
        let ls = layers(3);
        let p = Panel::layout(VP, 1.0, 34.0, &a, &ls, 0.0);
        let card = p.rows[1].card;
        let band = [p.band.x, p.band.y, p.band.w, p.band.h];

        // At rest a card is cut to the band exactly.
        let prims = p.prims(&ls, &showing(1, None), &a, 7, &theme);
        let at = prims.iter().position(|q| q.bounds() == card).unwrap();
        assert_eq!(prims[at].clip, band);

        // In flight it leans out of the panel, so the cut goes with it:
        // every piece of the card — its shadow, its outline, its body,
        // its eye and its name — falls inside what it is cut to.
        let prims = p.prims(&ls, &showing(1, Some(lift(1, card.y, 1.0))), &a, 7, &theme);
        let body = prims[carried_body(&prims, &theme)];
        let [cx, cy, cw, ch] = body.clip;
        assert!(cx < p.band.x - LIFT_LEFT, "the cut clears the lean");
        assert_ne!(body.clip, band);
        let cut = ScreenRect {
            x: cx,
            y: cy,
            w: cw,
            h: ch,
        };
        let carried: Vec<&Prim> = prims.iter().filter(|q| q.clip == body.clip).collect();
        assert!(carried.len() > 5, "a card is more than its body");
        for q in carried {
            assert!(
                cut.contains_rect(&q.bounds()),
                "{:?} is cut short by {cut:?}",
                q.bounds()
            );
        }

        // And the cards it left behind are cut to the band as before.
        let still = prims.iter().position(|q| q.bounds() == p.rows[0].card).unwrap();
        assert_eq!(prims[still].clip, band);
    }

    #[test]
    fn a_carried_card_stays_within_the_rows_on_show() {
        let theme = Theme::light();
        let a = atlas();
        let ls = layers(3);
        let p = Panel::layout(VP, 1.0, 34.0, &a, &ls, 0.0);
        let body_of = |y: f32| {
            let prims = p.prims(&ls, &showing(2, Some(lift(2, y, 1.0))), &a, 7, &theme);
            prims[carried_body(&prims, &theme)].bounds()
        };
        let top = p.rows[0].card;
        assert_eq!(body_of(-500.0).center().1, top.center().1, "no higher than the first row");
        let bottom = p.rows[2].card;
        assert_eq!(
            body_of(10_000.0).center().1,
            top.center().1 + (bottom.y - top.y),
            "no lower than the last"
        );
    }

    #[test]
    fn prims_highlight_the_active_row_and_dim_a_hidden_layer() {
        let theme = Theme::light();
        let a = atlas();
        let mut ls = layers(3);
        ls[0].visible = false;
        let p = Panel::layout(VP, 1.0, 34.0, &a, &ls, 0.0);
        let prims = p.prims(&ls, &showing(1, None), &a, 7, &theme);

        assert!(prims[0].feather > 0.0, "soft shadow goes first");
        assert!(
            prims
                .iter()
                .any(|q| q.color == theme.panel && q.bounds() == p.rect),
            "panel body"
        );
        let highlights: Vec<ScreenRect> = prims
            .iter()
            .filter(|q| q.kind == KIND_BOX && q.color == theme.active_bg)
            .map(|q| q.bounds())
            .collect();
        assert_eq!(highlights.len(), 1, "one active row");
        assert!(p.rows[1].rect.contains_rect(&highlights[0]), "on layer 1's row");

        // Labels are glyphs from the atlas slot; the title and every row
        // that fits have some.
        let glyphs = prims.iter().filter(|q| q.kind == KIND_IMAGE && q.slot == 7);
        assert!(glyphs.count() >= 4 + 3 * 5, "'Layers' and three 'Layer N'");

        // The hidden layer's eye is drawn dimmed, the others in the icon
        // color; every eye stays inside its box.
        for row in &p.rows {
            let expected = if ls[row.index].visible {
                theme.icon
            } else {
                theme.muted
            };
            let eye: Vec<&Prim> = prims
                .iter()
                .filter(|q| q.kind == KIND_SEGMENT && row.eye.contains_rect(&q.bounds()))
                .collect();
            assert!(!eye.is_empty(), "row {} has an eye", row.index);
            assert!(
                eye.iter().all(|q| q.color == expected),
                "row {}'s eye color",
                row.index
            );
        }
        // Buttons draw as strokes inside their boxes.
        for b in [p.up, p.down, p.add, p.remove] {
            assert!(
                prims
                    .iter()
                    .any(|q| q.kind == KIND_SEGMENT && b.contains_rect(&q.bounds())),
                "{b:?} has an icon"
            );
        }
    }
}
