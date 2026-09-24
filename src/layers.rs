//! Layers panel: the dock's chrome in a column on the right, the layer
//! tree one card per row, top layer first. A group or a frame opens in
//! place, its layers under it and one step in. Sized in logical px,
//! positioned in physical px, floating over the canvas and swallowing
//! whatever it catches, with a handle beside it that opens and closes it.
//! Pure — `app` asks where a click landed and what to draw.

use std::collections::HashMap;

use crate::doc::{BlendMode, Kind, Tag};
use crate::editor::Command;
use crate::field::Field;
use crate::menu::Item;
use crate::scene::{Prim, Rgba, ScreenRect, Viewport, icon_prims, mix, parse_color};
use crate::text::Atlas;
use crate::theme::Theme;
use crate::tree::{self, Filter, Place};

// Logical px.
pub const WIDTH: f32 = 248.0;
/// From the strip above and the window's right edge.
pub const MARGIN: f32 = 12.0;
pub const HEADER: f32 = 34.0;
pub const ROW: f32 = 34.0;
/// The panel's foot, where its own buttons stand.
pub const FOOTER: f32 = 34.0;
/// The bar under the header: how the picked layers blend, how strong
/// they are, and whether they are locked.
pub const PROPS: f32 = 30.0;
/// The filter's bar, under the header while it is open: a row for the
/// name to look for, and one of toggles — the four kinds, then the seven
/// colours.
pub const FILTER: f32 = SEARCH_ROW + CHIPS_ROW;
const SEARCH_ROW: f32 = 30.0;
const CHIPS_ROW: f32 = 28.0;
const FIELD_H: f32 = 24.0;
/// A colour's toggle, its dot, and the room between the kinds and them.
const TAG_W: f32 = 18.0;
const TAG_DOT: f32 = 5.0;
const TAG_GAP: f32 = 8.0;
/// How much of a tag's colour the eye's cell takes.
const TAG_TINT: f32 = 0.45;
/// What an empty filter field says it is for.
const PLACEHOLDER: &str = "Filter by name";
/// The blend mode's button, the room the strength's number takes, and
/// the track's thickness.
const BLEND_W: f32 = 92.0;
const VALUE_W: f32 = 36.0;
const TRACK_H: f32 = 4.0;
const KNOB: f32 = 5.0;
pub const PADDING: f32 = 6.0;
/// The footer's buttons and the eye are this square.
pub const BUTTON: f32 = 24.0;
pub const RADIUS: f32 = 12.0;
/// A card's corner. The rename is drawn over a card and must round
/// the same way it does.
pub const ROW_RADIUS: f32 = 6.0;
/// A row's card sits this far inside it, so the gap between two cards is
/// twice this and a click in the gap still lands on a row.
const CARD_INSET: f32 = 2.0;
/// The column the eyes stand in, left of every card, so they line up
/// however deep a row stands.
const EYE_COL: f32 = BUTTON + PADDING;
/// How far a card steps in for every group or frame holding it.
pub const INDENT: f32 = 14.0;
/// The narrowest a card is let get, however deep it stands: past that
/// the steps stop rather than the name.
const CARD_MIN: f32 = 120.0;
/// A holder's chevron, and the room every card keeps for one so the
/// glyphs line up down a level.
const CHEVRON: f32 = 16.0;
/// Between the card's edge and the chevron, and the chevron and the glyph.
const CHEVRON_GAP: f32 = 4.0;
/// The glyph: what a row is, in a box a thumbnail can later fill.
pub const GLYPH_W: f32 = 32.0;
pub const GLYPH_H: f32 = 24.0;
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
/// The drop's line, and the outline round a holder a card would go into.
const DROP_LINE: f32 = 2.0;
/// How long the lift takes to come on, and to go off again.
pub const LIFT_SECONDS: f32 = 0.14;
const BUTTON_GAP: f32 = 2.0;
/// Between an icon and the label it introduces: a row's glyph and its
/// name, the handle's chevron and the word under it.
const LABEL_GAP: f32 = 6.0;
/// The clickable area around the eye and the chevron, past the box.
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

/// What a press on the panel asks for. A row is named by its layer's id,
/// wherever in the tree it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelHit {
    /// Pick this layer: the card's body.
    Pick(String),
    /// Show or hide it: its eye.
    Toggle(String),
    /// Open this group or frame in place, or shut it: its chevron.
    Open(String),
    /// Open the lock on this layer: the lock its row wears, when the lock
    /// is its own.
    Lock(String),
    /// A card asked to be renamed. `Panel::hit` never answers it: `app`
    /// turns a second press on a picked card into it.
    Rename(String),
    /// The footer's three: a new group, a new layer, and the bin.
    Group,
    Add,
    Remove,
    /// The bar's: the blend mode's menu, the strength — read off the
    /// pointer's x with [`Panel::opacity_at`] — and the lock.
    Blend,
    Opacity,
    LockPicked,
    /// The header's filter, which opens its bar or shuts it; and in the
    /// bar, the name's field and a kind's or a colour's toggle.
    Filter,
    Search,
    FilterKind(Kind),
    FilterTag(Tag),
    /// Panel chrome between controls: swallowed, never reaches the canvas.
    Panel,
}

/// The card the pointer is carrying, and how far into the lift it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Lift {
    /// The layer it is the card of.
    pub id: String,
    /// Where the card's top edge is asked to be, in physical px: the
    /// pointer, less the grip the card was taken by. It follows the
    /// pointer rather than the row, so the card does not jump.
    pub y: f32,
    /// The lift, 0 (sitting in its row) to 1 (fully off the panel),
    /// before easing. It runs back down to 0 when the card is let go.
    pub t: f32,
}

/// The blend modes' menu, and the mode on each of its lines: Photoshop's
/// order and runs — darkening, lightening, contrast, inversion and the
/// component modes, a rule before each — opening on Pass Through for a
/// group, which is a group's alone. `current` is checked.
pub fn blend_menu(current: BlendMode, group: bool) -> (Vec<Item>, Vec<BlendMode>) {
    let runs = [
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::Overlay,
        BlendMode::Difference,
        BlendMode::Hue,
    ];
    let mut items = Vec::new();
    let mut modes = Vec::new();
    let offered = BlendMode::ALL
        .iter()
        .filter(|m| **m != BlendMode::PassThrough);
    let passing = group.then_some(&BlendMode::PassThrough);
    for mode in passing.into_iter().chain(offered) {
        let mut item = Item::new(mode.name()).checked(*mode == current);
        if runs.contains(mode) {
            item = item.ruled();
        }
        items.push(item);
        modes.push(*mode);
    }
    (items, modes)
}

/// A tag's colour, fixed as the inks are: a tag is the document's, so it
/// reads the same under every theme — Photoshop's seven, toned to stand
/// out on a light panel and a dark one alike.
pub fn tag_color(tag: Tag) -> Option<Rgba> {
    let hex = match tag {
        Tag::None => return None,
        Tag::Red => "#e5534b",
        Tag::Orange => "#e0823d",
        Tag::Yellow => "#d4a72c",
        Tag::Green => "#46a758",
        Tag::Blue => "#3e8ed0",
        Tag::Violet => "#8e6ad8",
        Tag::Gray => "#8b949e",
    };
    Some(parse_color(hex))
}

/// What a line of a row's menu does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowLine {
    /// Opens the row's name to be typed.
    Rename,
    Run(Command),
    /// The clipboard's three, which are the window's: it holds the
    /// clipboard.
    Copy,
    Cut,
    Paste,
    Tag(Tag),
}

/// What a row's menu says of the picked layers: whether every one is
/// locked, and hidden — the toggles say what they would do — and the
/// row's own tag, checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowState {
    pub locked: bool,
    pub hidden: bool,
    pub tag: Tag,
}

/// A row's menu: its name, then what can be done to the picked layers
/// with the keys that do the same — each offered only where it would
/// change something: a command where `can` says so, a paste where there
/// is something to `paste` — then the colours, as in Photoshop.
pub fn row_menu(can: impl Fn(Command) -> bool, paste: bool, state: RowState) -> (Vec<Item>, Vec<RowLine>) {
    let lines_of = [
        ("Duplicate", "Ctrl+J", RowLine::Run(Command::Duplicate), false),
        ("Delete", "Del", RowLine::Run(Command::Remove), false),
        ("Copy", "Ctrl+C", RowLine::Copy, true),
        ("Cut", "Ctrl+X", RowLine::Cut, false),
        ("Paste", "Ctrl+V", RowLine::Paste, false),
        ("Group", "Ctrl+G", RowLine::Run(Command::Group), true),
        ("Ungroup", "Ctrl+Shift+G", RowLine::Run(Command::Ungroup), false),
        (
            if state.locked { "Unlock" } else { "Lock" },
            "Ctrl+/",
            RowLine::Run(Command::Lock),
            true,
        ),
        (
            if state.hidden { "Show" } else { "Hide" },
            "Ctrl+,",
            RowLine::Run(Command::Show),
            false,
        ),
    ];
    let mut items = vec![Item::new("Rename").hint("F2")];
    let mut lines = vec![RowLine::Rename];
    for (label, keys, line, rule) in lines_of {
        let enabled = match line {
            RowLine::Run(command) => can(command),
            RowLine::Paste => paste,
            _ => true,
        };
        let item = Item::new(label).hint(keys).enabled(enabled);
        items.push(if rule { item.ruled() } else { item });
        lines.push(line);
    }
    let (tags, marks) = tag_menu(state.tag);
    for (i, (item, tag)) in tags.into_iter().zip(marks).enumerate() {
        items.push(if i == 0 { item.ruled() } else { item });
        lines.push(RowLine::Tag(tag));
    }
    (items, lines)
}

/// The colours' menu, and the tag on each of its lines: no colour, then
/// Photoshop's seven, each with its dot. `current` is checked.
pub fn tag_menu(current: Tag) -> (Vec<Item>, Vec<Tag>) {
    let tags: Vec<Tag> = std::iter::once(Tag::None).chain(Tag::COLORS).collect();
    let items = tags
        .iter()
        .map(|t| {
            let item = Item::new(t.name()).checked(*t == current);
            match tag_color(*t) {
                Some(c) => item.dot(c),
                None => item,
            }
        })
        .collect();
    (items, tags)
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

/// The cards on their way between two orders of the rows. A layer whose
/// row changed travels from where it was at the lift's pace, so the list
/// opens and closes around a carried card — or a group opening in place —
/// instead of jumping. Pure — `app` owns one and tells it what the rows
/// look like and how much time has passed.
#[derive(Debug, Default, Clone)]
pub struct Slides {
    on_the_way: HashMap<String, Coming>,
    /// The rows as they were when last looked at, top first. A list it
    /// has never seen starts every card still, so a tab switch does not
    /// slide a whole panel.
    seen: Vec<String>,
}

impl Slides {
    /// Takes in the rows' order, top first; anything that changed rows
    /// starts over from where it was. A row that was not on show before
    /// arrives where it belongs. `row` is one row in physical px.
    pub fn restack(&mut self, rows: &[&str], row: f32) {
        if self.seen.len() == rows.len() && self.seen.iter().zip(rows).all(|(a, b)| a == b) {
            return;
        }
        let order: Vec<String> = rows.iter().map(|id| (*id).to_owned()).collect();
        let seen = std::mem::replace(&mut self.seen, order);
        if seen.is_empty() {
            return;
        }
        let was: HashMap<&str, usize> = seen
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        for (now, id) in rows.iter().enumerate() {
            let Some(&then) = was.get(id) else {
                continue;
            };
            // Where it is on screen this instant, measured from the row
            // it is about to belong to: a card that moves again while
            // it is still travelling does not jump to start over.
            let step = (then as f32 - now as f32) * row;
            let mut coming = self.on_the_way.remove(*id).unwrap_or_default();
            coming.send(step);
            if coming.offset() != 0.0 {
                self.on_the_way.insert((*id).to_owned(), coming);
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
/// active, which are picked, which is in the pointer's hand and where it
/// would land, and what is still moving.
pub struct Showing<'a> {
    pub active: &'a str,
    pub picked: &'a [String],
    pub lift: Option<&'a Lift>,
    /// Where a carried card would land if let go of now.
    pub drop: Option<&'a Place>,
    pub slides: &'a Slides,
    /// What the bar says of the active layer: its blend mode's name, its
    /// strength, whether it is locked.
    pub blend: &'a str,
    pub opacity: f32,
    pub locked: bool,
    /// The filter, while its bar is open, and whether its field has the
    /// keyboard — then `app` draws the field, caret and all.
    pub filter: Option<&'a Filter>,
    pub searching: bool,
}

/// One row of the tree, with everything already measured.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The layer it is the row of.
    pub id: String,
    /// The layer holding its stack, none on the board's root.
    pub owner: Option<String>,
    pub depth: usize,
    pub kind: Kind,
    /// Its own eye.
    pub visible: bool,
    /// It and everything holding it on show: what is not is muted.
    pub shown: bool,
    /// A group or a frame shown open, its layers under it.
    pub open: bool,
    /// Its content cannot change: its own lock, or a holder's.
    pub locked: bool,
    /// The lock is its own, and so its row's to open.
    pub own_lock: bool,
    /// The colour it is tagged with, which tints its eye's cell.
    pub tag: Tag,
    /// What the pointer hits: the whole width and the gap under the card.
    pub rect: ScreenRect,
    /// What is drawn: stepped in by its depth, less the gap that
    /// separates two cards.
    pub card: ScreenRect,
    /// In the column left of every card, so the eyes line up.
    pub eye: ScreenRect,
    /// A group's or a frame's, which opens it in place. `None` for a
    /// layer that holds none.
    pub chevron: Option<ScreenRect>,
    /// What the row is: the kind's own icon, where a thumbnail can go.
    pub glyph: ScreenRect,
    /// The lock it wears at the card's far end, while it is locked.
    pub lock: Option<ScreenRect>,
    /// The name, cut down to what fits.
    pub label: String,
    /// Where the label's pen starts.
    pub label_x: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pub rect: ScreenRect,
    pub header: ScreenRect,
    /// The header's filter button.
    pub filter: ScreenRect,
    /// The filter's bar, while it is open: the name's field, then the
    /// kinds' toggles and the colours'.
    pub search: Option<ScreenRect>,
    pub kinds: Vec<(ScreenRect, Kind)>,
    pub tags: Vec<(ScreenRect, Tag)>,
    /// The bar under the header, and its three: the blend mode's button,
    /// the strength's slider and number, the lock.
    pub props: ScreenRect,
    pub blend: ScreenRect,
    pub opacity: ScreenRect,
    pub lock: ScreenRect,
    /// Where the cards are shown and cut off: as much of the tree as the
    /// window has room for.
    pub band: ScreenRect,
    /// Top first, and only those the band reaches.
    pub rows: Vec<Row>,
    /// The foot, and its three buttons: a new group, a new layer, the bin.
    pub footer: ScreenRect,
    pub group: ScreenRect,
    pub add: ScreenRect,
    pub remove: ScreenRect,
    /// The scrollbar's thumb, when there is more tree than band.
    pub bar: Option<ScreenRect>,
    /// The scroll actually used, in physical px — what was asked for,
    /// kept inside what there is to scroll.
    scroll: f32,
    /// Every row's height together, in physical px.
    content: f32,
    scale: f32,
}

impl Panel {
    /// `top` is where the strip ends, in physical px. `tree` is every row
    /// the tree has open, top first. Rows are laid out from `scroll` px
    /// above the band, which is as much of the tree as there is room for
    /// between the header and the footer; only the rows the band reaches
    /// are laid out, and a row it reaches part of is laid out whole and
    /// cut by [`Panel::band`] when it is drawn. `filtering` opens the
    /// filter's bar between the header and the properties' bar.
    pub fn layout(
        viewport: Viewport,
        scale: f64,
        top: f32,
        atlas: &Atlas,
        tree: &[tree::Row],
        scroll: f32,
        filtering: bool,
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
        let side = BUTTON * s;
        let filter = ScreenRect {
            x: header.x + header.w - side,
            y: header.y + (header.h - side) / 2.0,
            w: side,
            h: side,
        };
        let under = header.y + header.h;
        let (search, kinds, tags) = if filtering {
            Self::filter_bar(inner_x, inner_w, under, s)
        } else {
            (None, Vec::new(), Vec::new())
        };
        let bar_h = if filtering { FILTER * s } else { 0.0 };
        let props = ScreenRect {
            x: inner_x,
            y: under + bar_h,
            w: inner_w,
            h: PROPS * s,
        };
        let by = props.y + (props.h - side) / 2.0;
        let blend = ScreenRect {
            x: inner_x,
            y: by,
            w: BLEND_W * s,
            h: side,
        };
        let lock = ScreenRect {
            x: inner_x + inner_w - side,
            y: by,
            w: side,
            h: side,
        };
        let opacity = ScreenRect {
            x: blend.x + blend.w + LABEL_GAP * s,
            y: by,
            w: lock.x - LABEL_GAP * s - (blend.x + blend.w + LABEL_GAP * s),
            h: side,
        };

        let band_y = props.y + props.h;
        let room =
            (viewport.h as f32 - (MARGIN + PADDING + FOOTER) * s - band_y).max(0.0);
        let content = tree.len() as f32 * ROW * s;
        let band = ScreenRect {
            x: inner_x,
            y: band_y,
            w: inner_w,
            h: content.min(room),
        };
        let scroll = scroll.clamp(0.0, (content - band.h).max(0.0));
        let gutter = if content > band.h {
            (BAR_W + BAR_GAP) * s
        } else {
            0.0
        };

        let mut rows = Vec::new();
        for (pos, r) in tree.iter().enumerate() {
            let ry = band.y + pos as f32 * ROW * s - scroll;
            if ry + ROW * s <= band.y {
                continue;
            }
            if ry >= band.y + band.h {
                break;
            }
            rows.push(Self::row(r, inner_x, inner_w, gutter, ry, atlas, s));
        }

        // A thumb as tall a share of the band as the band is of the
        // tree, and only when there is tree it does not reach.
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

        // The foot, and its buttons right-aligned in it: the bin at the
        // end, as everywhere else.
        let footer = ScreenRect {
            x: inner_x,
            y: band.y + band.h,
            w: inner_w,
            h: FOOTER * s,
        };
        let by = footer.y + (footer.h - side) / 2.0;
        let mut bx = footer.x + footer.w - side;
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
        let group = button();

        let rect = ScreenRect {
            x,
            y,
            w: WIDTH * s,
            h: (2.0 * PADDING + HEADER + PROPS + FOOTER) * s + bar_h + band.h,
        };
        Panel {
            rect,
            header,
            filter,
            search,
            kinds,
            tags,
            props,
            blend,
            opacity,
            lock,
            band,
            rows,
            footer,
            group,
            add,
            remove,
            bar,
            scroll,
            content,
            scale: s,
        }
    }

    /// The filter's bar under `y`: the name's field across the panel, and
    /// under it the four kinds' toggles and then the seven colours'.
    #[allow(clippy::type_complexity)]
    fn filter_bar(
        x: f32,
        w: f32,
        y: f32,
        s: f32,
    ) -> (Option<ScreenRect>, Vec<(ScreenRect, Kind)>, Vec<(ScreenRect, Tag)>) {
        let side = BUTTON * s;
        let search = ScreenRect {
            x,
            y: y + (SEARCH_ROW - FIELD_H) / 2.0 * s,
            w,
            h: FIELD_H * s,
        };
        let cy = y + SEARCH_ROW * s + (CHIPS_ROW - BUTTON) / 2.0 * s;
        let kinds: Vec<(ScreenRect, Kind)> = [Kind::Raster, Kind::Vector, Kind::Group, Kind::Frame]
            .into_iter()
            .enumerate()
            .map(|(i, k)| {
                let r = ScreenRect {
                    x: x + i as f32 * (side + BUTTON_GAP * s),
                    y: cy,
                    w: side,
                    h: side,
                };
                (r, k)
            })
            .collect();
        let start = kinds.last().map_or(x, |(r, _)| r.x + r.w) + TAG_GAP * s;
        let tags = Tag::COLORS
            .into_iter()
            .enumerate()
            .map(|(i, t)| {
                let r = ScreenRect {
                    x: start + i as f32 * TAG_W * s,
                    y: cy,
                    w: TAG_W * s,
                    h: side,
                };
                (r, t)
            })
            .collect();
        (Some(search), kinds, tags)
    }

    /// Where the name's letters go in the filter's field: past the glass.
    pub fn search_text(&self) -> Option<ScreenRect> {
        let r = self.search?;
        let glass = (PADDING / 2.0 + ICON_BOX) * self.scale;
        Some(ScreenRect {
            x: r.x + glass,
            w: (r.w - glass).max(0.0),
            ..r
        })
    }

    /// One row, measured: the eye in its column, then the card stepped in
    /// by the row's depth — never so far that it is narrower than
    /// [`CARD_MIN`] — holding the chevron's room, the glyph and the name.
    fn row(
        r: &tree::Row,
        inner_x: f32,
        inner_w: f32,
        gutter: f32,
        y: f32,
        atlas: &Atlas,
        s: f32,
    ) -> Row {
        let side = BUTTON * s;
        let rect = ScreenRect {
            x: inner_x,
            y,
            w: inner_w,
            h: ROW * s,
        };
        let eye = ScreenRect {
            x: inner_x + (EYE_COL - BUTTON) / 2.0 * s,
            y: y + (rect.h - side) / 2.0,
            w: side,
            h: side,
        };
        let right = inner_x + inner_w - gutter;
        let deepest = (right - inner_x - (EYE_COL + CARD_MIN) * s).max(0.0);
        let indent = (r.depth as f32 * INDENT * s).min(deepest);
        let left = inner_x + EYE_COL * s + indent;
        let card = ScreenRect {
            x: left,
            y: y + CARD_INSET * s,
            w: right - left,
            h: (ROW - 2.0 * CARD_INSET) * s,
        };
        let cy = y + rect.h / 2.0;
        let holds = matches!(r.layer.kind, Kind::Group | Kind::Frame);
        let chevron = holds.then(|| ScreenRect {
            x: card.x + CHEVRON_GAP * s,
            y: cy - CHEVRON / 2.0 * s,
            w: CHEVRON * s,
            h: CHEVRON * s,
        });
        let glyph = ScreenRect {
            x: card.x + (2.0 * CHEVRON_GAP + CHEVRON) * s,
            y: cy - GLYPH_H / 2.0 * s,
            w: GLYPH_W * s,
            h: GLYPH_H * s,
        };
        let label_x = (glyph.x + glyph.w + LABEL_GAP * s).round();
        let lock = r.locked.then(|| ScreenRect {
            x: card.x + card.w - PADDING * s - ICON_BOX * s,
            y: cy - ICON_BOX / 2.0 * s,
            w: ICON_BOX * s,
            h: ICON_BOX * s,
        });
        let end = lock.map_or(card.x + card.w - PADDING * s, |l| l.x - LABEL_GAP * s);
        let room = end - label_x;
        let label = if room > 0.0 {
            atlas.truncate(&r.layer.name, room)
        } else {
            String::new()
        };
        Row {
            id: r.layer.id.clone(),
            owner: r.owner.map(str::to_owned),
            depth: r.depth,
            kind: r.layer.kind,
            visible: r.layer.visible,
            shown: r.shown,
            open: r.open,
            locked: r.locked,
            own_lock: r.layer.locked,
            tag: r.layer.color,
            rect,
            card,
            eye,
            chevron,
            glyph,
            lock,
            label,
            label_x,
        }
    }

    /// The scroll in use, in physical px.
    pub fn scroll(&self) -> f32 {
        self.scroll
    }

    /// How far the tree can be scrolled: nothing when it all fits.
    pub fn max_scroll(&self) -> f32 {
        (self.content - self.band.h).max(0.0)
    }

    /// The scroll that brings row `pos` — its place in the tree as listed,
    /// top first — into the band, moving as little as it can: what the
    /// panel does when a click on the canvas makes a layer active that is
    /// out of sight. Where it already is, the answer is the scroll it
    /// already has.
    pub fn scroll_showing(&self, pos: usize, rows: usize) -> f32 {
        if pos >= rows {
            return self.scroll;
        }
        let row = ROW * self.scale;
        let top = pos as f32 * row;
        let want = if top < self.scroll {
            top
        } else if top + row > self.scroll + self.band.h {
            top + row - self.band.h
        } else {
            self.scroll
        };
        want.clamp(0.0, self.max_scroll())
    }

    /// The slider's track, left of the strength's number.
    pub fn track(&self) -> ScreenRect {
        let s = self.scale;
        let o = self.opacity;
        ScreenRect {
            x: o.x + KNOB * s,
            y: o.y + (o.h - TRACK_H * s) / 2.0,
            w: (o.w - VALUE_W * s - 2.0 * KNOB * s).max(0.0),
            h: TRACK_H * s,
        }
    }

    /// The strength a press or a drag at `x` asks for, 0 to 1 along the
    /// track — held past either end, that end.
    pub fn opacity_at(&self, x: f64) -> f32 {
        let t = self.track();
        if t.w <= 0.0 {
            return 1.0;
        }
        ((x as f32 - t.x) / t.w).clamp(0.0, 1.0)
    }

    pub fn hit(&self, x: f64, y: f64) -> Option<PanelHit> {
        if !self.rect.contains(x, y) {
            return None;
        }
        let buttons = [
            (self.group, PanelHit::Group),
            (self.add, PanelHit::Add),
            (self.remove, PanelHit::Remove),
            (self.blend, PanelHit::Blend),
            (self.opacity, PanelHit::Opacity),
            (self.lock, PanelHit::LockPicked),
            (self.filter, PanelHit::Filter),
        ];
        let bar = self
            .search
            .map(|r| (r, PanelHit::Search))
            .into_iter()
            .chain(self.kinds.iter().map(|(r, k)| (*r, PanelHit::FilterKind(*k))))
            .chain(self.tags.iter().map(|(r, t)| (*r, PanelHit::FilterTag(*t))));
        if let Some((_, hit)) = buttons.into_iter().chain(bar).find(|(r, _)| r.contains(x, y)) {
            return Some(hit);
        }
        // A row reaches past the band when it is only part shown; the
        // pointer never does.
        if self.band.contains(x, y) {
            let slop = -EYE_SLOP * self.scale;
            for row in &self.rows {
                if !row.rect.contains(x, y) {
                    continue;
                }
                if row.eye.inset(slop).contains(x, y) {
                    return Some(PanelHit::Toggle(row.id.clone()));
                }
                if let Some(chevron) = row.chevron
                    && chevron.inset(slop).contains(x, y)
                {
                    return Some(PanelHit::Open(row.id.clone()));
                }
                if let Some(lock) = row.lock.filter(|_| row.own_lock)
                    && lock.inset(slop).contains(x, y)
                {
                    return Some(PanelHit::Lock(row.id.clone()));
                }
                return Some(PanelHit::Pick(row.id.clone()));
            }
        }
        Some(PanelHit::Panel)
    }

    /// The row a card dragged to `y` would land on: the one under the
    /// pointer, or the nearest once the drag has left the list at either
    /// end, so overshooting still drops it where it was headed. `None`
    /// when no row is on show.
    pub fn drop_row(&self, y: f64) -> Option<&Row> {
        let last = self.rows.last()?;
        let y = y as f32;
        Some(self.rows.iter().find(|r| y < r.rect.y + r.rect.h).unwrap_or(last))
    }

    /// Where a card let go of at `y` lands, read off the row under the
    /// pointer — or the nearest, once the drag has left the list: the
    /// middle third of a group's or a frame's row is inside it; the half
    /// of a row nearer the top is above it and the other half under it —
    /// except under an open holder, which is the top of what it holds,
    /// since that is the row drawn there.
    pub fn aim(&self, y: f64) -> Option<Place> {
        let row = self.drop_row(y)?;
        let f = ((y as f32 - row.rect.y) / row.rect.h).clamp(0.0, 1.0);
        let holds = row.chevron.is_some();
        let id = row.id.clone();
        Some(if holds && (1.0 / 3.0..2.0 / 3.0).contains(&f) {
            Place::Into(id)
        } else if f < 0.5 {
            Place::Above(id)
        } else if holds && row.open {
            Place::Into(id)
        } else {
            Place::Below(id)
        })
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

    /// Paint order: shadow, border, panel, the title, a card per row —
    /// and, over the cards it is passing, the one `showing.lift` names —
    /// then the thumb and the footer's buttons.
    pub fn prims(&self, showing: &Showing, atlas: &Atlas, slot: u32, theme: &Theme) -> Vec<Prim> {
        let s = self.scale;
        let b = theme.edge(s);
        let corner = theme.corner(RADIUS, s);
        let mut out = vec![
            Prim::soft(
                self.rect.offset(0.0, SHADOW_OFFSET * s),
                corner,
                SHADOW_FEATHER * s,
                theme.shadow,
            ),
            Prim::rounded(self.rect.inset(-b), corner + b, theme.border),
            Prim::rounded(self.rect, corner, theme.panel),
        ];
        let baseline = atlas.baseline_in(self.header);
        for g in atlas.layout(TITLE, self.header.x + PADDING * s, baseline) {
            out.push(Prim::glyph(g.rect, g.uv, slot, theme.ink));
        }
        out.extend(self.filter_prims(showing, atlas, slot, theme));
        out.extend(self.bar_prims(showing, atlas, slot, theme));
        let carried = showing.lift.map(|l| l.id.as_str());
        for row in self.rows.iter().filter(|r| Some(r.id.as_str()) != carried) {
            let dy = showing.slides.offset(&row.id);
            self.card_prims(row, showing, None, dy, atlas, slot, theme, &mut out);
        }
        if let Some(place) = showing.drop {
            out.extend(self.drop_prims(place, theme));
        }
        // A card in the hand is where the pointer put it, not where the
        // tree says: it takes no slide.
        if let Some(l) = showing.lift
            && let Some(row) = self.rows.iter().find(|r| r.id == l.id)
        {
            self.card_prims(row, showing, Some(l), 0.0, atlas, slot, theme, &mut out);
        }
        // The thumb sits over the cards, at the band's right edge: it
        // says how much of the tree is on show and where.
        if let Some(bar) = self.bar {
            out.push(Prim::rounded(bar, bar.w / 2.0, theme.muted));
        }
        for (rect, icon) in [
            (self.group, FOLDER_PLUS),
            (self.add, PLUS),
            (self.remove, TRASH),
        ] {
            out.extend(icon_prims(icon, rect, 24.0, ICON_BOX, ICON_STROKE, s, theme.icon));
        }
        out
    }

    /// The header's filter button — lit while its bar is open — and the
    /// bar: the field, bordered as the blend button is, the glass and the
    /// name or what the field is for; the kinds' icons and the colours'
    /// dots, each lit while it narrows the tree.
    fn filter_prims(&self, showing: &Showing, atlas: &Atlas, slot: u32, theme: &Theme) -> Vec<Prim> {
        let s = self.scale;
        let b = theme.edge(s);
        let corner = theme.corner(ROW_RADIUS, s);
        let mut out = Vec::new();
        let lit = |r: ScreenRect| Prim::rounded(r, corner, theme.active_bg);
        if self.search.is_some() {
            out.push(lit(self.filter));
        }
        out.extend(icon_prims(FUNNEL, self.filter, 24.0, ICON_BOX, ICON_STROKE, s, theme.icon));
        let (Some(search), Some(text)) = (self.search, self.search_text()) else {
            return out;
        };
        out.push(Prim::rounded(search.inset(-b), corner + b, theme.border));
        out.push(Prim::rounded(search, corner, theme.panel));
        let glass = ScreenRect {
            x: search.x + PADDING / 2.0 * s,
            y: search.y + (search.h - ICON_BOX * s) / 2.0,
            w: ICON_BOX * s,
            h: ICON_BOX * s,
        };
        out.extend(icon_prims(GLASS, glass, 24.0, ICON_BOX, ICON_STROKE, s, theme.muted));
        let name = showing.filter.map_or("", |f| f.name.as_str());
        if !showing.searching {
            if name.is_empty() {
                let baseline = atlas.baseline_in(text);
                for g in atlas.layout(PLACEHOLDER, text.x + crate::field::PADDING, baseline) {
                    out.push(Prim::glyph(g.rect, g.uv, slot, theme.muted).clipped(text));
                }
            } else {
                out.extend(Field::new(name).prims(text, atlas, slot, theme, false));
            }
        }
        let filter = showing.filter;
        for (r, kind) in &self.kinds {
            let on = filter.is_some_and(|f| f.kinds.contains(kind));
            if on {
                out.push(lit(*r));
            }
            let icon = match kind {
                Kind::Raster => PIXELS,
                Kind::Vector => CURVE,
                Kind::Group => FOLDER,
                Kind::Frame => FRAME,
            };
            let color = if on { theme.ink } else { theme.icon };
            out.extend(icon_prims(icon, *r, 24.0, ICON_BOX, ICON_STROKE, s, color));
        }
        for (r, tag) in &self.tags {
            if filter.is_some_and(|f| f.tags.contains(tag)) {
                out.push(lit(*r));
            }
            if let Some(c) = tag_color(*tag) {
                let (cx, cy) = r.center();
                out.push(Prim::circle(cx, cy, TAG_DOT * s, c));
            }
        }
        out
    }

    /// The bar: the blend mode's button — its name and a chevron — the
    /// strength as a track filled as far as it goes with its number
    /// beside it, and the lock, shut or open.
    fn bar_prims(&self, showing: &Showing, atlas: &Atlas, slot: u32, theme: &Theme) -> Vec<Prim> {
        let s = self.scale;
        let b = theme.edge(s);
        let corner = theme.corner(ROW_RADIUS, s);
        let mut out = vec![
            Prim::rounded(self.blend.inset(-b), corner + b, theme.border),
            Prim::rounded(self.blend, corner, theme.panel),
        ];
        let chevron = ScreenRect {
            x: self.blend.x + self.blend.w - (ICON_BOX + PADDING) * s,
            y: self.blend.y + (self.blend.h - ICON_BOX * s) / 2.0,
            w: ICON_BOX * s,
            h: ICON_BOX * s,
        };
        out.extend(icon_prims(CHEVRON_DOWN, chevron, 24.0, ICON_BOX, ICON_STROKE, s, theme.icon));
        let name_x = self.blend.x + PADDING * s;
        let name = atlas.truncate(showing.blend, (chevron.x - name_x).max(0.0));
        let baseline = atlas.baseline_in(self.blend);
        for g in atlas.layout(&name, name_x, baseline) {
            out.push(Prim::glyph(g.rect, g.uv, slot, theme.ink));
        }
        let t = self.track();
        let v = showing.opacity.clamp(0.0, 1.0);
        out.push(Prim::rounded(t, t.h / 2.0, theme.muted));
        let fill = ScreenRect { w: t.w * v, ..t };
        if fill.w > 0.0 {
            out.push(Prim::rounded(fill, t.h / 2.0, theme.ink));
        }
        out.push(Prim::circle(t.x + t.w * v, t.y + t.h / 2.0, KNOB * s, theme.ink));
        let number = format!("{}%", (v * 100.0).round() as u32);
        // Right-aligned a little way in: a glyph's ink can reach past its
        // advance, and the number is not to touch the lock.
        let right = self.opacity.x + self.opacity.w - PADDING / 2.0 * s;
        let baseline = atlas.baseline_in(self.opacity);
        for g in atlas.layout(&number, right - atlas.measure(&number), baseline) {
            out.push(Prim::glyph(g.rect, g.uv, slot, theme.ink));
        }
        let lock = if showing.locked { LOCK } else { UNLOCK };
        out.extend(icon_prims(lock, self.lock, 24.0, ICON_BOX, ICON_STROKE, s, theme.icon));
        out
    }

    /// Where a card would land, in the lift's own blue: a line on the
    /// boundary between two rows, as far in as the stack it lands in —
    /// or, into a group or a frame, its card outlined all round.
    fn drop_prims(&self, place: &Place, theme: &Theme) -> Vec<Prim> {
        let s = self.scale;
        let t = DROP_LINE * s;
        let find = |id: &str| self.rows.iter().find(|r| r.id == id);
        let line = |row: &Row, y: f32| {
            let r = ScreenRect {
                x: row.card.x,
                y: y - t / 2.0,
                w: row.card.w,
                h: t,
            };
            vec![Prim::rounded(r, t / 2.0, theme.lifted).clipped(self.band.inset(-t))]
        };
        match place {
            Place::Above(id) => find(id).map_or(Vec::new(), |r| line(r, r.rect.y)),
            Place::Below(id) => find(id).map_or(Vec::new(), |r| line(r, r.rect.y + r.rect.h)),
            Place::Into(id) => {
                let Some(r) = find(id) else {
                    return Vec::new();
                };
                let c = r.card.inset(-t / 2.0);
                [
                    (c.x, c.y, c.w, t),
                    (c.x, c.y + c.h - t, c.w, t),
                    (c.x, c.y + t, t, (c.h - 2.0 * t).max(0.0)),
                    (c.x + c.w - t, c.y + t, t, (c.h - 2.0 * t).max(0.0)),
                ]
                .into_iter()
                .map(|(x, y, w, h)| Prim::rect(ScreenRect { x, y, w, h }, theme.lifted).clipped(self.band))
                .collect()
            }
        }
    }

    /// One row: its card — shadow, outline, body — then the chevron, the
    /// glyph and the name, and the eye in its column. A card in flight is
    /// the same row, every property carried `lift` of the way: further
    /// off the panel, bluer at the edge, and — once it is drawn — bigger,
    /// turned and leaning at the pointer, contents and all.
    #[allow(clippy::too_many_arguments)]
    fn card_prims(
        &self,
        row: &Row,
        showing: &Showing,
        lift: Option<&Lift>,
        dy: f32,
        atlas: &Atlas,
        slot: u32,
        theme: &Theme,
        out: &mut Vec<Prim>,
    ) {
        let s = self.scale;
        let radius = theme.corner(ROW_RADIUS, s);
        let e = lift.map_or(0.0, |l| ease(l.t));
        let at = |rest: f32, flight: f32| rest + (flight - rest) * e;
        let (drop, feather, edge) = (
            at(CARD_SHADOW_OFFSET, LIFT_SHADOW_OFFSET),
            at(CARD_SHADOW_FEATHER, LIFT_SHADOW_FEATHER),
            at(theme.border_px, LIFT_BORDER.max(theme.border_px)),
        );
        let outline = mix(theme.border, theme.lifted, e);
        let picked = showing.picked.contains(&row.id) || showing.active == row.id;
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
            if picked { theme.active_bg } else { theme.panel },
        ));
        // A tag is read off the eye's cell, as in Photoshop.
        if let Some(c) = tag_color(row.tag) {
            out.push(Prim::rounded(
                row.eye,
                theme.corner(ROW_RADIUS, s),
                mix(theme.panel, c, TAG_TINT),
            ));
        }
        // What is not on show is read as such: muted, whatever its own
        // eye says — hiding a group hides what it holds.
        let tint = |on: Rgba| if row.shown { on } else { theme.muted };
        let (eye, color) = if row.visible {
            (EYE, tint(theme.icon))
        } else {
            (EYE_HIDDEN, theme.muted)
        };
        out.extend(icon_prims(eye, row.eye, 24.0, ICON_BOX, ICON_STROKE, s, color));
        if row.visible {
            let (cx, cy) = row.eye.center();
            out.push(Prim::circle(cx, cy, PUPIL / 24.0 * ICON_BOX * s, color));
        }
        if let Some(chevron) = row.chevron {
            let icon = if row.open { CHEVRON_DOWN } else { CHEVRON_RIGHT };
            out.extend(icon_prims(icon, chevron, 24.0, CHEVRON, ICON_STROKE, s, theme.icon));
        }
        // What the row is, read and never clicked: pixels, a curve, a
        // folder — open when it is — or a frame.
        let glyph = match (row.kind, row.open) {
            (Kind::Raster, _) => PIXELS,
            (Kind::Vector, _) => CURVE,
            (Kind::Group, false) => FOLDER,
            (Kind::Group, true) => FOLDER_OPEN,
            (Kind::Frame, _) => FRAME,
        };
        let square = ScreenRect {
            x: row.glyph.x + (row.glyph.w - row.glyph.h) / 2.0,
            w: row.glyph.h,
            ..row.glyph
        };
        out.extend(icon_prims(glyph, square, 24.0, ICON_BOX, ICON_STROKE, s, tint(theme.icon)));
        // A lock of its own is the row's to open; one worn for a holder
        // is read, muted, since it is the holder's.
        if let Some(lock) = row.lock {
            let color = if row.own_lock { theme.icon } else { theme.muted };
            out.extend(icon_prims(LOCK, lock, 24.0, ICON_BOX, ICON_STROKE, s, color));
        }
        if !row.label.is_empty() {
            let ink = tint(if picked { theme.ink } else { theme.icon });
            let baseline = atlas.baseline_in(row.rect);
            for g in atlas.layout(&row.label, row.label_x, baseline) {
                out.push(Prim::glyph(g.rect, g.uv, slot, ink));
            }
        }
        // Out of the tree in the pointer's hand, or still on its way to
        // the row it now belongs to. Either way the whole row moves,
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
        // The band is where the tree is shown, and the rest of a row that
        // reaches past it is not drawn — but a card in flight is out of
        // the tree, so the band lets go of it as it lifts.
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
        let b = theme.edge(s);
        // The tab is a surface, painted like the panel it opens, so the
        // desktop's corner reaches it as well — never past a capsule,
        // which is as round as a tab this narrow goes.
        let capsule = self.rect.w.min(self.rect.h) / 2.0;
        let radius = theme.corner(HANDLE_W / 2.0, s).min(capsule);
        let mut out = vec![
            Prim::soft(
                self.rect.offset(0.0, SHADOW_OFFSET * s),
                radius,
                SHADOW_FEATHER * s,
                theme.shadow,
            ),
            Prim::rounded(self.rect.inset(-b), radius + b, theme.border),
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
            let kbd = theme.corner(KBD_RADIUS, s);
            out.push(Prim::rounded(k.inset(-b), kbd + b, theme.border));
            out.push(Prim::rounded(k, kbd, theme.active_bg));
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
/// The handle's arrow: out the way the panel comes, and back — and a shut
/// holder's, pointing at the row it would open.
const CHEVRON_LEFT: &[&[(f32, f32)]] = &[&[(15.0, 6.0), (9.0, 12.0), (15.0, 18.0)]];
const CHEVRON_RIGHT: &[&[(f32, f32)]] = &[&[(9.0, 6.0), (15.0, 12.0), (9.0, 18.0)]];
/// An open holder's: pointing down at what it holds.
const CHEVRON_DOWN: &[&[(f32, f32)]] = &[&[(6.0, 9.0), (12.0, 15.0), (18.0, 9.0)]];
/// The filter: a funnel.
const FUNNEL: &[&[(f32, f32)]] = &[&[
    (4.0, 5.0),
    (20.0, 5.0),
    (14.0, 12.0),
    (14.0, 19.0),
    (10.0, 21.0),
    (10.0, 12.0),
    (4.0, 5.0),
]];
/// Looking for a name: a glass and its handle.
const GLASS: &[&[(f32, f32)]] = &[
    &[
        (16.5, 10.5),
        (16.04, 12.8),
        (14.74, 14.74),
        (12.8, 16.04),
        (10.5, 16.5),
        (8.2, 16.04),
        (6.26, 14.74),
        (4.96, 12.8),
        (4.5, 10.5),
        (4.96, 8.2),
        (6.26, 6.26),
        (8.2, 4.96),
        (10.5, 4.5),
        (12.8, 4.96),
        (14.74, 6.26),
        (16.04, 8.2),
        (16.5, 10.5),
    ],
    &[(14.74, 14.74), (20.0, 20.0)],
];
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

/// What a raster layer holds: a grid of pixels.
const PIXELS: &[&[(f32, f32)]] = &[
    &[(5.0, 5.0), (19.0, 5.0), (19.0, 19.0), (5.0, 19.0), (5.0, 5.0)],
    &[(12.0, 5.0), (12.0, 19.0)],
    &[(5.0, 12.0), (19.0, 12.0)],
];

/// What a vector layer holds: one curve, flattened the way the renderer
/// flattens the real ones.
const CURVE: &[&[(f32, f32)]] = &[&[
    (4.0, 18.0),
    (6.0, 13.5),
    (8.0, 10.5),
    (10.0, 9.5),
    (12.0, 11.0),
    (14.0, 14.0),
    (16.0, 15.0),
    (18.0, 13.0),
    (20.0, 8.0),
]];

/// A group, shut: a folder.
const FOLDER: &[&[(f32, f32)]] = &[&[
    (3.0, 6.0),
    (9.0, 6.0),
    (11.0, 9.0),
    (21.0, 9.0),
    (21.0, 19.0),
    (3.0, 19.0),
    (3.0, 6.0),
]];

/// A group, open: the folder's front leaf let down.
const FOLDER_OPEN: &[&[(f32, f32)]] = &[
    &[(3.0, 19.0), (3.0, 6.0), (9.0, 6.0), (11.0, 9.0), (19.0, 9.0), (19.0, 11.0)],
    &[(3.0, 19.0), (6.0, 11.0), (22.0, 11.0), (19.0, 19.0), (3.0, 19.0)],
];

/// The footer's new group: a folder with a plus on it.
const FOLDER_PLUS: &[&[(f32, f32)]] = &[
    &[
        (3.0, 6.0),
        (9.0, 6.0),
        (11.0, 9.0),
        (21.0, 9.0),
        (21.0, 19.0),
        (3.0, 19.0),
        (3.0, 6.0),
    ],
    &[(12.0, 11.0), (12.0, 17.0)],
    &[(9.0, 14.0), (15.0, 14.0)],
];

/// A padlock, shut: what a locked row wears.
const LOCK: &[&[(f32, f32)]] = &[
    &[(6.0, 11.0), (18.0, 11.0), (18.0, 20.0), (6.0, 20.0), (6.0, 11.0)],
    &[
        (8.0, 11.0),
        (8.0, 8.0),
        (9.0, 6.0),
        (12.0, 5.0),
        (15.0, 6.0),
        (16.0, 8.0),
        (16.0, 11.0),
    ],
];

/// A padlock, open: the bar's lock while the active layer is not locked.
const UNLOCK: &[&[(f32, f32)]] = &[
    &[(6.0, 11.0), (18.0, 11.0), (18.0, 20.0), (6.0, 20.0), (6.0, 11.0)],
    &[(8.0, 11.0), (8.0, 6.0), (9.0, 4.0), (12.0, 3.0), (15.0, 4.0), (16.0, 6.0), (16.0, 7.0)],
];

/// A frame: the crop marks of an area, as design tools draw one.
const FRAME: &[&[(f32, f32)]] = &[
    &[(8.0, 3.0), (8.0, 21.0)],
    &[(16.0, 3.0), (16.0, 21.0)],
    &[(3.0, 8.0), (21.0, 8.0)],
    &[(3.0, 16.0), (21.0, 16.0)],
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Document, Layer, Tag};
    use crate::scene::{KIND_BOX, KIND_IMAGE, KIND_SEGMENT};
    use crate::tree::Filter;
    use crate::tabs::Tabs;
    use crate::text::Font;

    fn atlas() -> Atlas {
        Atlas::build(&Font::bundled(), Tabs::label_px(crate::tabs::LABEL, 1.0))
    }

    /// A board of `n` raster layers on its root, `L1` at the bottom.
    fn flat(n: usize) -> Document {
        let mut doc = Document::new("t");
        doc.layers = (1..=n)
            .map(|i| Layer {
                id: format!("L{i}"),
                ..Layer::of(&format!("Layer {i}"), Kind::Raster)
            })
            .collect();
        doc
    }

    /// The panel over `doc` with the holders in `open` shown open.
    fn laid(doc: &Document, open: &[&str], viewport: Viewport, scale: f64, scroll: f32) -> Panel {
        let rows = doc.rows(|id| open.contains(&id));
        Panel::layout(viewport, scale, 34.0 * scale as f32, &atlas(), &rows, scroll, false)
    }

    fn panel(viewport: Viewport, scale: f64, n: usize) -> Panel {
        laid(&flat(n), &[], viewport, scale, 0.0)
    }

    const VP: Viewport = Viewport { w: 1200, h: 800 };

    fn sr(x: f32, y: f32, w: f32, h: f32) -> ScreenRect {
        ScreenRect { x, y, w, h }
    }

    fn ids(p: &Panel) -> Vec<&str> {
        p.rows.iter().map(|r| r.id.as_str()).collect()
    }

    fn row<'a>(p: &'a Panel, id: &str) -> &'a Row {
        p.rows.iter().find(|r| r.id == id).expect("a row by that id")
    }

    fn mid(r: ScreenRect) -> (f64, f64) {
        let (x, y) = r.center();
        (f64::from(x), f64::from(y))
    }

    #[test]
    fn panel_sits_below_the_strip_at_the_right_edge() {
        let p = panel(VP, 1.0, 2);
        // MARGIN from the right edge and from the strip's bottom; padding,
        // the header, one row per layer, the footer, padding.
        assert_eq!(
            p.rect,
            sr(
                1200.0 - MARGIN - WIDTH,
                34.0 + MARGIN,
                WIDTH,
                2.0 * PADDING + HEADER + PROPS + 2.0 * ROW + FOOTER
            )
        );
    }

    #[test]
    fn rows_list_the_top_layer_first() {
        let p = panel(VP, 1.0, 3);
        assert_eq!(ids(&p), ["L3", "L2", "L1"]);
        assert_eq!(p.rows[0].label, "Layer 3");
        assert_eq!(p.rows[0].rect.y, p.props.y + p.props.h, "under the bar");
        assert_eq!(p.rows[1].rect.y, p.rows[0].rect.y + ROW);
        assert_eq!(p.rows[0].rect.h, ROW);
        for r in &p.rows {
            assert!(r.rect.contains_rect(&r.eye), "the eye sits in its row");
            assert!(r.rect.contains_rect(&r.card), "and so does the card");
            assert!(r.card.contains_rect(&r.glyph), "which holds the glyph");
            assert!(r.label_x > r.glyph.x + r.glyph.w, "the name follows it");
            assert!(r.eye.x + r.eye.w <= r.card.x, "the eye is left of the card");
        }
    }

    #[test]
    fn a_group_opens_in_place_its_layers_one_step_in() {
        let doc = crate::tree::tests::nested();
        let shut = laid(&doc, &[], VP, 1.0, 0.0);
        assert_eq!(ids(&shut), ["F", "G", "A"]);
        let open = laid(&doc, &["G"], VP, 1.0, 0.0);
        assert_eq!(ids(&open), ["F", "G", "H", "B", "A"], "under its row, in place");
        let (g, h, a) = (row(&open, "G"), row(&open, "H"), row(&open, "A"));
        assert_eq!(h.depth, 1);
        assert_eq!(h.card.x, g.card.x + INDENT, "one step in");
        assert_eq!(a.card.x, g.card.x, "back out after it");
        assert_eq!(h.card.x + h.card.w, g.card.x + g.card.w, "every card ends at the band's edge");
        // The eyes stand in one column whatever the depth, so they line up.
        assert!(open.rows.iter().all(|r| r.eye.x == g.eye.x));
        assert!(g.open && !h.open);
    }

    #[test]
    fn a_card_never_steps_in_past_its_narrowest() {
        // A group forty deep: past a point the steps stop, not the name.
        let mut doc = Document::new("t");
        let mut layer = Layer {
            id: "leaf".into(),
            ..Layer::of("Leaf", Kind::Raster)
        };
        let mut open = Vec::new();
        for i in 0..40 {
            let id = format!("g{i}");
            layer = Layer {
                id: id.clone(),
                layers: vec![layer],
                ..Layer::of("Group", Kind::Group)
            };
            open.push(id);
        }
        doc.layers = vec![layer];
        let open: Vec<&str> = open.iter().map(String::as_str).collect();
        let p = laid(&doc, &open, Viewport { w: 1200, h: 4000 }, 1.0, 0.0);
        let leaf = row(&p, "leaf");
        assert_eq!(leaf.depth, 40);
        assert!(leaf.card.w >= CARD_MIN, "{}", leaf.card.w);
    }

    #[test]
    fn only_a_group_or_a_frame_has_a_chevron_and_every_glyph_lines_up() {
        let doc = crate::tree::tests::nested();
        let p = laid(&doc, &["G"], VP, 1.0, 0.0);
        assert!(row(&p, "F").chevron.is_some() && row(&p, "G").chevron.is_some());
        assert!(row(&p, "H").chevron.is_some());
        assert!(row(&p, "A").chevron.is_none() && row(&p, "B").chevron.is_none());
        // A layer keeps the chevron's room, so its glyph stands where a
        // holder's does at the same depth.
        assert_eq!(row(&p, "A").glyph.x, row(&p, "G").glyph.x);
        assert_eq!(row(&p, "B").glyph.x, row(&p, "H").glyph.x);
    }

    #[test]
    fn hit_names_the_eye_the_chevron_the_card_and_the_footer() {
        let doc = crate::tree::tests::nested();
        let p = laid(&doc, &["G"], VP, 1.0, 0.0);
        let hit = |r: ScreenRect| {
            let (x, y) = mid(r);
            p.hit(x, y)
        };
        let g = row(&p, "G");
        assert_eq!(hit(g.eye), Some(PanelHit::Toggle("G".into())));
        assert_eq!(hit(g.chevron.unwrap()), Some(PanelHit::Open("G".into())));
        assert_eq!(hit(g.glyph), Some(PanelHit::Pick("G".into())));
        let b = row(&p, "B");
        assert_eq!(p.hit(f64::from(b.label_x) + 2.0, mid(b.card).1), Some(PanelHit::Pick("B".into())));
        assert_eq!(hit(p.group), Some(PanelHit::Group));
        assert_eq!(hit(p.add), Some(PanelHit::Add));
        assert_eq!(hit(p.remove), Some(PanelHit::Remove));
        let (_, y) = mid(p.header);
        assert_eq!(
            p.hit(f64::from(p.header.x) + 10.0, y),
            Some(PanelHit::Panel),
            "the title swallows the click"
        );
        assert_eq!(p.hit(100.0, 100.0), None);
    }

    #[test]
    fn the_footer_holds_group_add_and_the_bin_at_the_foot() {
        let p = panel(VP, 1.0, 3);
        assert_eq!(p.footer.y, p.band.y + p.band.h, "right under the rows");
        assert_eq!(p.footer.y + p.footer.h + PADDING, p.rect.y + p.rect.h);
        for b in [p.group, p.add, p.remove] {
            assert!(p.footer.contains_rect(&b), "{b:?} is in the footer");
        }
        assert!(p.group.x < p.add.x && p.add.x < p.remove.x, "the bin at the end");
        assert_eq!(p.remove.x + p.remove.w, p.footer.x + p.footer.w);
    }

    #[test]
    fn the_band_is_as_much_of_the_tree_as_there_is_room_for() {
        // Room for the header, one row and the footer above the margin.
        let h = (34.0 + MARGIN + PADDING + HEADER + PROPS + ROW + FOOTER + PADDING + MARGIN) as u32;
        let p = panel(Viewport { w: 1200, h }, 1.0, 3);
        assert_eq!(p.band.h, ROW);
        assert_eq!(ids(&p), ["L3"], "the top layer is what the band starts on");
        assert_eq!(p.rect.h, 2.0 * PADDING + HEADER + PROPS + ROW + FOOTER);
        assert_eq!(p.max_scroll(), 2.0 * ROW, "two rows below the band");
        assert!(p.bar.is_some(), "and a thumb to say so");

        // A pixel short, and the band keeps the row, cut off. A scroll
        // area ends mid-card; it does not drop one.
        let p = panel(Viewport { w: 1200, h: h - 1 }, 1.0, 3);
        assert_eq!(p.band.h, ROW - 1.0);
        assert_eq!(p.rows.len(), 1);
        assert!(p.rows[0].rect.h > p.band.h);

        // Everything fits: no scroll, no thumb, and the band is the tree.
        let p = panel(VP, 1.0, 3);
        assert_eq!(p.band.h, 3.0 * ROW);
        assert_eq!(p.max_scroll(), 0.0);
        assert!(p.bar.is_none());
    }

    #[test]
    fn scrolling_slides_the_tree_under_the_band() {
        let h = (34.0 + MARGIN + PADDING + HEADER + PROPS + 2.0 * ROW + FOOTER + PADDING + MARGIN) as u32;
        let vp = Viewport { w: 1200, h };
        let doc = flat(4);
        let at = |scroll: f32| laid(&doc, &[], vp, 1.0, scroll);

        let top = at(0.0);
        assert_eq!(top.scroll(), 0.0);
        assert_eq!(ids(&top), ["L4", "L3"], "the top of the tree");

        // Half a row down: three rows are part shown.
        let half = at(ROW / 2.0);
        assert_eq!(ids(&half), ["L4", "L3", "L2"]);
        assert_eq!(half.rows[0].rect.y, half.band.y - ROW / 2.0, "cut at the top");

        // Past the end, and it stops with the last row against the
        // band's bottom.
        let end = at(10_000.0);
        assert_eq!(end.scroll(), 2.0 * ROW);
        assert_eq!(ids(&end), ["L2", "L1"]);
        let last = end.rows.last().unwrap();
        assert_eq!(last.rect.y + last.rect.h, end.band.y + end.band.h);

        // The thumb is the band's share of the tree, and travels the
        // whole way.
        let thumb = top.bar.unwrap();
        assert_eq!(thumb.h, top.band.h * top.band.h / (4.0 * ROW));
        assert_eq!(thumb.y, top.band.y);
        let thumb = end.bar.unwrap();
        assert_eq!(thumb.y + thumb.h, end.band.y + end.band.h);
    }

    #[test]
    fn scroll_showing_moves_as_little_as_it_can() {
        let h = (34.0 + MARGIN + PADDING + HEADER + PROPS + 2.0 * ROW + FOOTER + PADDING + MARGIN) as u32;
        let vp = Viewport { w: 1200, h };
        let doc = flat(4);
        let at = |scroll: f32| laid(&doc, &[], vp, 1.0, scroll);

        // Looking at the top two rows.
        let p = at(0.0);
        assert_eq!(p.scroll_showing(0, 4), 0.0, "already in the band");
        assert_eq!(p.scroll_showing(1, 4), 0.0);
        assert_eq!(p.scroll_showing(2, 4), ROW, "just far enough down");
        assert_eq!(p.scroll_showing(3, 4), 2.0 * ROW);

        // Looking at the bottom two: coming back up stops as soon as the
        // row is in.
        let p = at(2.0 * ROW);
        assert_eq!(p.scroll_showing(3, 4), 2.0 * ROW);
        assert_eq!(p.scroll_showing(1, 4), ROW);
        assert_eq!(p.scroll_showing(0, 4), 0.0);
        assert_eq!(p.scroll_showing(9, 4), p.scroll(), "no such row");
    }

    #[test]
    fn layout_scales_with_the_display() {
        let p = panel(VP, 2.0, 2);
        assert_eq!(p.rect.w, 2.0 * WIDTH);
        assert_eq!(p.rect.x, 1200.0 - 2.0 * (MARGIN + WIDTH));
        assert_eq!(p.rows[0].rect.h, 2.0 * ROW);
        assert_eq!(p.add.w, 2.0 * BUTTON);
        assert_eq!(p.footer.h, 2.0 * FOOTER);
    }

    #[test]
    fn drop_row_names_the_row_under_the_pointer() {
        let p = panel(VP, 1.0, 3);
        let at = |y: f64| p.drop_row(y).map(|r| r.id.as_str());
        let y = |i: usize| mid(p.rows[i].rect).1;
        assert_eq!(at(y(0)), Some("L3"));
        assert_eq!(at(y(2)), Some("L1"));
        // Past either end it is the nearest row, so a drag that
        // overshoots the list still lands where it was headed.
        assert_eq!(at(0.0), Some("L3"));
        assert_eq!(at(10_000.0), Some("L1"));
        let h = (34.0 + MARGIN + PADDING + HEADER + PROPS + FOOTER + PADDING + MARGIN) as u32;
        let p = panel(Viewport { w: 1200, h }, 1.0, 3);
        assert!(p.rows.is_empty());
        assert!(p.drop_row(100.0).is_none(), "nothing on show, nowhere to drop");
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

    /// The tab is a surface, not a knob: it stands beside the panel and
    /// is painted like it, so the desktop's own corner has to reach it.
    /// At Omarchy's `decoration:rounding` of 0 the panel and the dock go
    /// square, and a pill left standing between them reads as broken.
    #[test]
    fn the_desktops_corner_reaches_the_handle() {
        let mut theme = Theme::light();
        let a = atlas();
        let body = |h: &Handle, theme: &Theme| {
            h.prims(&a, 7, theme)
                .into_iter()
                .find(|q| q.color == theme.panel && q.bounds() == h.rect)
                .expect("the handle's body")
        };

        // As the chrome was drawn: the tab is a capsule, rounded to half
        // its own short side, open and closed alike.
        let closed = handle(VP, 1.0, false);
        let open = handle(VP, 1.0, true);
        assert_eq!(body(&closed, &theme).radius, HANDLE_W / 2.0);
        assert_eq!(body(&open, &theme).radius, HANDLE_W / 2.0);

        theme.rounding = 0.0;
        assert_eq!(
            body(&closed, &theme).radius,
            0.0,
            "square, like every window out there"
        );
        assert_eq!(body(&open, &theme).radius, 0.0);

        // And it never rounds past a capsule, however far the desktop
        // rounds its own windows.
        theme.rounding = 4.0;
        assert_eq!(body(&closed, &theme).radius, HANDLE_W / 2.0);
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
        let p = panel(VP, 1.0, 3);
        let body = |prims: &[Prim], card: ScreenRect| {
            prims.iter().position(|q| q.bounds() == card && q.feather == 0.0)
        };

        // At rest: a card inside every row, on a hairline of the border
        // color, over a shadow of its own.
        let prims = p.prims(&showing("L2", None), &a, 7, &theme);
        for row in &p.rows {
            assert!(row.rect.contains_rect(&row.card), "the card sits in its row");
            let at = body(&prims, row.card);
            assert!(at.is_some(), "row {} has a body", row.id);
            assert_eq!(
                prims[at.unwrap()].clip,
                [p.band.x, p.band.y, p.band.w, p.band.h],
                "row {} is cut to the band",
                row.id
            );
            assert!(
                prims
                    .iter()
                    .any(|q| q.color == theme.border && q.bounds() == row.card.inset(-1.0)),
                "row {} is outlined",
                row.id
            );
            let shadow: Vec<&Prim> = prims
                .iter()
                .filter(|q| {
                    q.color == theme.shadow && q.bounds() == row.card.offset(0.0, CARD_SHADOW_OFFSET)
                })
                .collect();
            assert_eq!(shadow.len(), 1, "row {} casts one shadow", row.id);
            assert_eq!(shadow[0].feather, CARD_SHADOW_FEATHER);
        }
        assert!(!prims.iter().any(|q| q.color == theme.lifted), "nothing is in flight");

        // Two cards are a gap apart, and the gap belongs to a row.
        assert_eq!(
            p.rows[1].card.y - (p.rows[0].card.y + p.rows[0].card.h),
            2.0 * CARD_INSET
        );

        // Carried: the middle card takes the blue, a shadow with further
        // to fall, and goes last — over the cards it is passing.
        let card = row(&p, "L2").card;
        let l = lift("L2", card.y, 1.0);
        let prims = p.prims(&showing("L2", Some(&l)), &a, 7, &theme);
        let blue: Vec<&Prim> = prims.iter().filter(|q| q.color == theme.lifted).collect();
        assert_eq!(blue.len(), 1, "one card is in flight");
        assert!(prims[0].feather > 0.0 && prims[0].color == theme.shadow);
        let long: Vec<&Prim> = prims[1..]
            .iter()
            .filter(|q| q.color == theme.shadow && q.feather > CARD_SHADOW_FEATHER)
            .collect();
        assert_eq!(long.len(), 1, "one card's shadow has further to fall");
        let carried = carried_body(&prims, &theme);
        assert!(carried > body(&prims, row(&p, "L3").card).unwrap());
        assert!(carried > body(&prims, row(&p, "L1").card).unwrap());
    }

    fn lift(id: &str, y: f32, t: f32) -> Lift {
        Lift {
            id: id.into(),
            y,
            t,
        }
    }

    /// Nothing on the move: what the panel shows at rest.
    fn showing<'a>(active: &'a str, lift: Option<&'a Lift>) -> Showing<'a> {
        static STILL: std::sync::LazyLock<Slides> = std::sync::LazyLock::new(Slides::default);
        Showing {
            active,
            picked: &[],
            lift,
            drop: None,
            slides: &STILL,
            blend: "Normal",
            opacity: 1.0,
            locked: false,
            filter: None,
            searching: false,
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
        let p = panel(VP, 1.0, 3);
        let card = row(&p, "L2").card;
        let (was_x, was_y) = card.center();
        let body_at = |y: f32, t: f32| {
            let l = lift("L2", y, t);
            let prims = p.prims(&showing("L2", Some(&l)), &a, 7, &theme);
            prims[carried_body(&prims, &theme)]
        };

        // Fully lifted, asked to stay on its row's line.
        let full = body_at(card.y, 1.0);
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
        let half = body_at(card.y, 0.5);
        assert_eq!(half.angle, LIFT_TILT.to_radians() * 0.5);
        assert_eq!(half.geom[2], card.w * (1.0 + LIFT_SCALE * 0.5));
        assert_eq!(half.geom[0] + half.geom[2] / 2.0, was_x - LIFT_LEFT * 0.5);

        // The card is where the pointer put it, not on any row's line.
        let moved = body_at(card.y + 11.0, 1.0);
        assert_eq!(moved.geom[1] + moved.geom[3] / 2.0, was_y + 11.0);

        // Nothing of the lift is drawn at rest.
        let none = body_at(card.y + 11.0, 0.0);
        assert_eq!(none.bounds(), card);
        assert_eq!(none.angle, 0.0);
    }

    #[test]
    fn the_cards_that_make_room_slide_from_where_they_were() {
        let mut s = Slides::default();

        // A list it has never seen is already where it belongs.
        s.restack(&["L3", "L2", "L1"], ROW);
        assert!(!s.moving());
        assert_eq!(s.offset("L1"), 0.0);

        // The bottom layer goes to the top: it climbs two rows, and the
        // two it passed each drop one. Every card starts from where it
        // was, so the first frame after the move looks like the last
        // frame before it.
        s.restack(&["L1", "L3", "L2"], ROW);
        assert!(s.moving());
        assert_eq!(s.offset("L1"), 2.0 * ROW, "down two rows from the top");
        assert_eq!(s.offset("L2"), -ROW);
        assert_eq!(s.offset("L3"), -ROW);

        // Halfway there is halfway back.
        s.tick(LIFT_SECONDS / 2.0);
        assert_eq!(s.offset("L1"), 2.0 * ROW * ease(0.5));
        // A card that moves again while travelling carries what is left
        // of the old trip into the new one, instead of jumping.
        let carried = s.offset("L2");
        assert_eq!(carried, -ROW * ease(0.5));
        s.restack(&["L1", "L2", "L3"], ROW);
        assert_eq!(s.offset("L2"), ROW + carried);

        // And they all arrive.
        s.tick(LIFT_SECONDS);
        assert!(!s.moving());
        assert_eq!(s.offset("L2"), 0.0);
    }

    #[test]
    fn a_group_opening_in_place_slides_what_is_under_it_down() {
        let mut s = Slides::default();
        s.restack(&["F", "G", "A"], ROW);
        // G opens: its two rows arrive in place, and A makes room for
        // them by sliding from where it was.
        s.restack(&["F", "G", "H", "B", "A"], ROW);
        assert_eq!(s.offset("A"), -2.0 * ROW, "it was two rows higher");
        assert_eq!(s.offset("H"), 0.0, "a row that was not on show arrives where it belongs");
        assert_eq!(s.offset("G"), 0.0, "and what did not move does not slide");
    }

    #[test]
    fn a_sliding_card_is_drawn_off_its_row() {
        let theme = Theme::light();
        let a = atlas();
        let p = panel(VP, 1.0, 3);
        let mut s = Slides::default();
        s.restack(&["L3", "L2", "L1"], ROW);
        s.restack(&["L2", "L3", "L1"], ROW);
        let showing = Showing {
            active: "L1",
            picked: &[],
            lift: None,
            drop: None,
            slides: &s,
            blend: "Normal",
            opacity: 1.0,
            locked: false,
            filter: None,
            searching: false,
        };
        let prims = p.prims(&showing, &a, 7, &theme);
        for row in &p.rows {
            let card = row.card.offset(0.0, s.offset(&row.id));
            assert!(
                prims.iter().any(|q| q.bounds() == card && q.feather == 0.0),
                "row {} is drawn where it is coming from",
                row.id
            );
        }
    }

    #[test]
    fn the_band_lets_go_of_a_card_in_flight() {
        let theme = Theme::light();
        let a = atlas();
        let p = panel(VP, 1.0, 3);
        let card = row(&p, "L2").card;
        let band = [p.band.x, p.band.y, p.band.w, p.band.h];

        // At rest a card is cut to the band exactly.
        let prims = p.prims(&showing("L2", None), &a, 7, &theme);
        let at = prims.iter().position(|q| q.bounds() == card).unwrap();
        assert_eq!(prims[at].clip, band);

        // In flight it leans out of the panel, so the cut goes with it:
        // every piece of the row — its shadow, its outline, its body, its
        // eye, its glyph and its name — falls inside what it is cut to.
        let l = lift("L2", card.y, 1.0);
        let prims = p.prims(&showing("L2", Some(&l)), &a, 7, &theme);
        let body = prims[carried_body(&prims, &theme)];
        let [cx, cy, cw, ch] = body.clip;
        assert!(cx < p.band.x - LIFT_LEFT, "the cut clears the lean");
        assert_ne!(body.clip, band);
        let cut = sr(cx, cy, cw, ch);
        let carried: Vec<&Prim> = prims.iter().filter(|q| q.clip == body.clip).collect();
        assert!(carried.len() > 5, "a row is more than its body");
        for q in carried {
            assert!(cut.contains_rect(&q.bounds()), "{:?} is cut short by {cut:?}", q.bounds());
        }

        // And the cards it left behind are cut to the band as before.
        let still = prims.iter().position(|q| q.bounds() == row(&p, "L3").card).unwrap();
        assert_eq!(prims[still].clip, band);
    }

    #[test]
    fn a_carried_card_stays_within_the_rows_on_show() {
        let theme = Theme::light();
        let a = atlas();
        let p = panel(VP, 1.0, 3);
        let body_of = |y: f32| {
            let l = lift("L3", y, 1.0);
            let prims = p.prims(&showing("L3", Some(&l)), &a, 7, &theme);
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
    fn prims_fill_the_picked_rows_and_mute_what_is_not_on_show() {
        let theme = Theme::light();
        let a = atlas();
        let mut doc = crate::tree::tests::nested();
        doc.layer_mut("G").unwrap().visible = false;
        let p = laid(&doc, &["G"], VP, 1.0, 0.0);
        let picked = vec!["A".to_owned()];
        let show = Showing {
            active: "B",
            picked: &picked,
            lift: None,
            drop: None,
            slides: &Slides::default(),
            blend: "Normal",
            opacity: 1.0,
            locked: false,
            filter: None,
            searching: false,
        };
        let prims = p.prims(&show, &a, 7, &theme);

        assert!(prims[0].feather > 0.0, "soft shadow goes first");
        let filled: Vec<ScreenRect> = prims
            .iter()
            .filter(|q| q.kind == KIND_BOX && q.color == theme.active_bg)
            .map(|q| q.bounds())
            .collect();
        assert_eq!(filled.len(), 2, "the active row and the picked one");
        assert!(filled.contains(&row(&p, "B").card) && filled.contains(&row(&p, "A").card));

        // Hiding a group hides what it holds: its rows are muted, though
        // their own eyes are open.
        let segs_in = |r: ScreenRect| -> Vec<&Prim> {
            prims
                .iter()
                .filter(|q| q.kind == KIND_SEGMENT && r.contains_rect(&q.bounds()))
                .collect()
        };
        for (id, muted) in [("G", true), ("H", true), ("B", true), ("A", false), ("F", false)] {
            let r = row(&p, id);
            let glyph = segs_in(r.glyph);
            assert!(!glyph.is_empty(), "{id} has a glyph");
            let want = if muted { theme.muted } else { theme.icon };
            assert!(glyph.iter().all(|q| q.color == want), "{id}'s glyph");
        }
        let eye = segs_in(row(&p, "B").eye);
        assert!(eye.iter().all(|q| q.color == theme.muted), "B's eye is muted with it");

        // The title and every row that fits are lettered.
        let glyphs = prims.iter().filter(|q| q.kind == KIND_IMAGE && q.slot == 7);
        assert!(glyphs.count() >= TITLE.len() + 5 * 5);
        // The footer's three buttons draw inside their boxes.
        for b in [p.group, p.add, p.remove] {
            assert!(!segs_in(b).is_empty(), "{b:?} has an icon");
        }
    }

    #[test]
    fn every_kind_reads_apart() {
        let theme = Theme::light();
        let a = atlas();
        let mut doc = crate::tree::tests::nested();
        doc.layer_mut("B").unwrap().kind = Kind::Vector;
        let p = laid(&doc, &["G"], VP, 1.0, 0.0);
        let prims = p.prims(&showing("A", None), &a, 7, &theme);
        // What is drawn inside a glyph box, relative to the box.
        let drawing = |id: &str, open: bool| -> Vec<[i32; 4]> {
            let r = row(&p, id);
            assert_eq!(r.open, open);
            prims
                .iter()
                .filter(|q| q.kind == KIND_SEGMENT && r.glyph.contains_rect(&q.bounds()))
                .map(|q| {
                    let b = q.bounds();
                    [(b.x - r.glyph.x) as i32, (b.y - r.glyph.y) as i32, b.w as i32, b.h as i32]
                })
                .collect()
        };
        let kinds = [
            drawing("A", false), // raster
            drawing("B", false), // vector
            drawing("H", false), // a shut group
            drawing("G", true),  // an open one
            drawing("F", false), // a frame
        ];
        for (i, one) in kinds.iter().enumerate() {
            assert!(!one.is_empty(), "kind {i} is drawn");
            for (j, other) in kinds.iter().enumerate().skip(i + 1) {
                assert_ne!(one, other, "kinds {i} and {j} read alike");
            }
        }
    }

    /// The x both strokes of a chevron touch — its tip — and the y.
    fn chevron_tip_in(prims: &[Prim], r: ScreenRect) -> (f32, f32) {
        let segs: Vec<&Prim> = prims
            .iter()
            .filter(|q| q.kind == KIND_SEGMENT && r.contains_rect(&q.bounds()))
            .collect();
        assert_eq!(segs.len(), 2, "two strokes make a chevron");
        (segs[0].geom[2], segs[0].geom[3])
    }

    #[test]
    fn a_shut_holders_chevron_points_at_it_and_an_open_ones_points_down() {
        let theme = Theme::light();
        let a = atlas();
        let doc = crate::tree::tests::nested();
        let p = laid(&doc, &["G"], VP, 1.0, 0.0);
        let prims = p.prims(&showing("A", None), &a, 7, &theme);
        let shut = row(&p, "F").chevron.unwrap();
        let (x, _) = chevron_tip_in(&prims, shut);
        assert!(x > shut.center().0, "a shut one points right");
        let open = row(&p, "G").chevron.unwrap();
        let (_, y) = chevron_tip_in(&prims, open);
        assert!(y > open.center().1, "an open one points down");
    }

    fn into(id: &str) -> Place {
        Place::Into(id.into())
    }

    fn above(id: &str) -> Place {
        Place::Above(id.into())
    }

    fn below(id: &str) -> Place {
        Place::Below(id.into())
    }

    #[test]
    fn the_middle_of_a_holders_row_drops_into_it_and_its_edges_beside_it() {
        let doc = crate::tree::tests::nested();
        let p = laid(&doc, &["G"], VP, 1.0, 0.0);
        let at = |id: &str, f: f32| {
            let r = row(&p, id);
            p.aim(f64::from(r.rect.y + r.rect.h * f))
        };
        assert_eq!(at("G", 0.5), Some(into("G")));
        assert_eq!(at("G", 0.1), Some(above("G")));
        // G is open: just under its row is the top of what it holds.
        assert_eq!(at("G", 0.9), Some(into("G")));
        // F is shut: under its row is under F.
        assert_eq!(at("F", 0.9), Some(below("F")));
        assert_eq!(at("F", 0.5), Some(into("F")));
        // A layer holds none: its row is cut in two.
        assert_eq!(at("A", 0.4), Some(above("A")));
        assert_eq!(at("A", 0.6), Some(below("A")));
        // Past either end, the nearest edge — so an overshoot still lands
        // where it was headed.
        assert_eq!(p.aim(0.0), Some(above("F")));
        assert_eq!(p.aim(10_000.0), Some(below("A")));
    }

    #[test]
    fn a_drop_is_a_line_at_the_depth_it_lands_or_an_outline_round_its_holder() {
        let theme = Theme::light();
        let a = atlas();
        let doc = crate::tree::tests::nested();
        let p = laid(&doc, &["G"], VP, 1.0, 0.0);
        let drawn = |place: &Place| -> Vec<ScreenRect> {
            let show = Showing {
                drop: Some(place),
                ..showing("A", None)
            };
            p.prims(&show, &a, 7, &theme)
                .iter()
                .filter(|q| q.color == theme.lifted)
                .map(Prim::bounds)
                .collect()
        };
        let nothing = p.prims(&showing("A", None), &a, 7, &theme);
        assert!(!nothing.iter().any(|q| q.color == theme.lifted));

        // Above B: one line on the boundary over B's row, starting where
        // B's card does — the depth it lands at.
        let b = row(&p, "B");
        let line = drawn(&above("B"));
        assert_eq!(line.len(), 1, "{line:?}");
        assert_eq!(line[0].x, b.card.x);
        assert!((line[0].center().1 - b.rect.y).abs() < 1.0);
        // Under B: on the boundary under it.
        let line = drawn(&below("B"));
        assert!((line[0].center().1 - (b.rect.y + b.rect.h)).abs() < 1.0);
        // Into H: its card, outlined all round.
        let h = row(&p, "H");
        let ring = drawn(&into("H"));
        assert_eq!(ring.len(), 4, "four edges");
        let around = ring.iter().fold(ring[0], |acc, r| acc.union(r));
        assert!(around.contains_rect(&h.card));
        assert!(h.card.inset(-4.0).contains_rect(&around), "and hugging it");
    }

    #[test]
    fn a_locked_row_wears_a_lock_and_its_own_lock_opens_it() {
        let theme = Theme::light();
        let a = atlas();
        let mut doc = crate::tree::tests::nested();
        doc.layer_mut("G").unwrap().locked = true;
        let p = laid(&doc, &["G"], VP, 1.0, 0.0);
        let (g, b) = (row(&p, "G"), row(&p, "B"));
        let (g_lock, b_lock) = (g.lock.expect("G is locked"), b.lock.expect("and so is B"));
        assert!(row(&p, "A").lock.is_none(), "A is not");
        assert!(g.card.contains_rect(&g_lock));
        assert!(g.label_x + a.measure(&g.label) <= g_lock.x, "the name stops short of it");
        let (x, y) = mid(g_lock);
        assert_eq!(p.hit(x, y), Some(PanelHit::Lock("G".into())));
        let (x, y) = mid(b_lock);
        assert_eq!(p.hit(x, y), Some(PanelHit::Pick("B".into())), "B's lock is G's to open");
        // Its own lock in ink; one worn for a holder, muted.
        let prims = p.prims(&showing("A", None), &a, 7, &theme);
        let segs = |r: ScreenRect| -> Vec<&Prim> {
            prims
                .iter()
                .filter(|q| q.kind == KIND_SEGMENT && r.contains_rect(&q.bounds()))
                .collect()
        };
        assert!(!segs(g_lock).is_empty() && segs(g_lock).iter().all(|q| q.color == theme.icon));
        assert!(!segs(b_lock).is_empty() && segs(b_lock).iter().all(|q| q.color == theme.muted));
    }

    #[test]
    fn the_properties_bar_stands_between_the_header_and_the_rows() {
        let p = panel(VP, 1.0, 2);
        assert_eq!(p.props.y, p.header.y + p.header.h);
        assert_eq!(p.props.h, PROPS);
        assert_eq!(p.band.y, p.props.y + p.props.h, "the rows start under it");
        for r in [p.blend, p.opacity, p.lock] {
            assert!(p.props.contains_rect(&r), "{r:?} is on the bar");
        }
        assert!(p.blend.x + p.blend.w <= p.opacity.x && p.opacity.x + p.opacity.w <= p.lock.x);
        assert_eq!(p.rect.h, 2.0 * PADDING + HEADER + PROPS + 2.0 * ROW + FOOTER);
    }

    #[test]
    fn a_press_on_the_opacity_asks_for_the_strength_under_it() {
        let p = panel(VP, 1.0, 2);
        let (x, y) = mid(p.opacity);
        assert_eq!(p.hit(x, y), Some(PanelHit::Opacity));
        let t = p.track();
        assert_eq!(p.opacity_at(f64::from(t.x)), 0.0);
        assert_eq!(p.opacity_at(f64::from(t.x + t.w)), 1.0);
        assert_eq!(p.opacity_at(f64::from(t.x + t.w / 2.0)), 0.5);
        assert_eq!(p.opacity_at(-100.0), 0.0, "held past either end, the end");
        assert_eq!(p.opacity_at(1e6), 1.0);
        let (x, y) = mid(p.lock);
        assert_eq!(p.hit(x, y), Some(PanelHit::LockPicked));
        let (x, y) = mid(p.blend);
        assert_eq!(p.hit(x, y), Some(PanelHit::Blend));
    }

    #[test]
    fn the_bar_shows_the_active_layers_strength_and_lock() {
        let theme = Theme::light();
        let a = atlas();
        let p = panel(VP, 1.0, 2);
        let at = |opacity: f32, locked: bool| {
            let show = Showing {
                opacity,
                locked,
                ..showing("L1", None)
            };
            p.prims(&show, &a, 7, &theme)
        };
        // The track is filled as far as the strength goes.
        let t = p.track();
        let fill = |prims: &[Prim]| -> f32 {
            prims
                .iter()
                .filter(|q| q.color == theme.ink && q.bounds().y >= t.y - 2.0 && q.bounds().y <= t.y + t.h)
                .map(|q| q.bounds().w)
                .fold(0.0, f32::max)
        };
        let half = fill(&at(0.5, false));
        let full = fill(&at(1.0, false));
        assert!((half - t.w / 2.0).abs() < 1.0, "{half}");
        assert!((full - t.w).abs() < 1.0, "{full}");
        // And the number is written.
        let glyphs_in = |prims: &[Prim], r: ScreenRect| {
            prims.iter().filter(|q| q.kind == KIND_IMAGE && q.slot == 7 && r.contains_rect(&q.bounds())).count()
        };
        assert_eq!(glyphs_in(&at(0.5, false), p.opacity), "50%".len());
        assert_eq!(glyphs_in(&at(1.0, false), p.opacity), "100%".len());
        // The lock reads as shut or open.
        let drawing = |prims: &[Prim]| -> Vec<[i32; 2]> {
            prims
                .iter()
                .filter(|q| q.kind == KIND_SEGMENT && p.lock.contains_rect(&q.bounds()))
                .map(|q| [q.bounds().x as i32, q.bounds().y as i32])
                .collect()
        };
        assert_ne!(drawing(&at(1.0, true)), drawing(&at(1.0, false)));
    }

    #[test]
    fn the_blend_menu_lists_photoshops_modes_in_its_runs() {
        let (items, modes) = blend_menu(BlendMode::Multiply, false);
        assert_eq!(items.len(), 27, "every mode but a group's own");
        assert_eq!(modes.len(), items.len());
        assert_eq!(modes[0], BlendMode::Normal);
        assert!(!modes.contains(&BlendMode::PassThrough));
        let checked: Vec<BlendMode> = modes
            .iter()
            .zip(&items)
            .filter(|(_, i)| i.checked)
            .map(|(m, _)| *m)
            .collect();
        assert_eq!(checked, [BlendMode::Multiply], "what is in force");
        // A run each: darken, lighten, contrast, inversion, component.
        let starts: Vec<BlendMode> = modes
            .iter()
            .zip(&items)
            .filter(|(_, i)| i.rule)
            .map(|(m, _)| *m)
            .collect();
        assert_eq!(
            starts,
            [
                BlendMode::Darken,
                BlendMode::Lighten,
                BlendMode::Overlay,
                BlendMode::Difference,
                BlendMode::Hue
            ]
        );
        assert_eq!(items[3].label, "Multiply");
        // A group's menu opens on Pass Through.
        let (items, modes) = blend_menu(BlendMode::PassThrough, true);
        assert_eq!(items.len(), 28);
        assert_eq!(modes[0], BlendMode::PassThrough);
        assert!(items[0].checked);
    }

    #[test]
    fn rename_names_a_row_by_its_layer_like_pick_does() {
        // The two travel together: app turns a Pick into a Rename on the
        // second press, so they must name a row the same way.
        let pick = PanelHit::Pick("L1".into());
        let PanelHit::Pick(id) = pick else { unreachable!() };
        assert_eq!(PanelHit::Rename(id.clone()), PanelHit::Rename("L1".into()));
    }

    /// The panel over `doc`, everything shut, with the filter's bar open.
    fn filtered(doc: &Document, scale: f64) -> Panel {
        let rows = doc.rows(|_| false);
        Panel::layout(VP, scale, 34.0 * scale as f32, &atlas(), &rows, 0.0, true)
    }

    #[test]
    fn the_filters_bar_opens_under_the_header_and_pushes_the_rest_down() {
        let doc = flat(2);
        let shut = laid(&doc, &[], VP, 1.0, 0.0);
        assert!(shut.search.is_none() && shut.kinds.is_empty() && shut.tags.is_empty());
        assert!(shut.header.contains_rect(&shut.filter));
        let (x, y) = mid(shut.filter);
        assert_eq!(shut.hit(x, y), Some(PanelHit::Filter));
        for scale in [1.0, 2.0] {
            let s = scale as f32;
            let shut = laid(&doc, &[], VP, scale, 0.0);
            let open = filtered(&doc, scale);
            assert_eq!(open.header, shut.header);
            assert_eq!(open.props.y - shut.props.y, FILTER * s);
            assert_eq!(open.rect.h - shut.rect.h, FILTER * s);
            let search = open.search.expect("a field to type a name into");
            assert!(search.y >= open.header.y + open.header.h);
            let (x, y) = mid(search);
            assert_eq!(open.hit(x, y), Some(PanelHit::Search));
            let kinds: Vec<Kind> = open.kinds.iter().map(|(_, k)| *k).collect();
            assert_eq!(kinds, [Kind::Raster, Kind::Vector, Kind::Group, Kind::Frame]);
            let tags: Vec<Tag> = open.tags.iter().map(|(_, t)| *t).collect();
            assert_eq!(tags, Tag::COLORS);
            let inner = open.rect.inset(PADDING * s);
            let mut all: Vec<ScreenRect> = open
                .kinds
                .iter()
                .map(|(r, _)| *r)
                .chain(open.tags.iter().map(|(r, _)| *r))
                .collect();
            for r in &all {
                assert!(inner.contains_rect(r), "{r:?} inside the panel");
                assert!(r.y >= search.y + search.h && r.y + r.h <= open.props.y);
            }
            all.sort_by(|a, b| a.x.total_cmp(&b.x));
            for w in all.windows(2) {
                assert!(w[0].x + w[0].w <= w[1].x, "side by side");
            }
            for (r, k) in &open.kinds {
                let (x, y) = mid(*r);
                assert_eq!(open.hit(x, y), Some(PanelHit::FilterKind(*k)));
            }
            for (r, t) in &open.tags {
                let (x, y) = mid(*r);
                assert_eq!(open.hit(x, y), Some(PanelHit::FilterTag(*t)));
            }
        }
    }

    #[test]
    fn the_colour_menu_offers_no_colour_and_the_seven_the_current_one_checked() {
        let (items, tags) = tag_menu(Tag::Green);
        assert_eq!(tags[0], Tag::None);
        assert_eq!(&tags[1..], Tag::COLORS);
        assert_eq!(items.len(), tags.len());
        assert_eq!(items[0].label, "No Color");
        assert!(items[0].dot.is_none());
        for (item, tag) in items.iter().zip(&tags).skip(1) {
            assert_eq!(item.label, tag.name());
            assert_eq!(item.dot, tag_color(*tag));
        }
        let checked: Vec<Tag> = items
            .iter()
            .zip(&tags)
            .filter(|(i, _)| i.checked)
            .map(|(_, t)| *t)
            .collect();
        assert_eq!(checked, [Tag::Green]);
    }

    #[test]
    fn a_rows_menu_offers_what_would_change_something_and_teaches_its_keys() {
        let state = RowState {
            locked: false,
            hidden: false,
            tag: Tag::Red,
        };
        let (items, lines) = row_menu(|c| c != Command::Ungroup, false, state);
        assert_eq!(items.len(), lines.len());
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels[..10],
            ["Rename", "Duplicate", "Delete", "Copy", "Cut", "Paste", "Group", "Ungroup", "Lock", "Hide"]
        );
        assert_eq!(&labels[10..], ["No Color", "Red", "Orange", "Yellow", "Green", "Blue", "Violet", "Gray"]);
        let line = |label: &str| labels.iter().position(|l| *l == label).unwrap();
        assert_eq!(lines[line("Rename")], RowLine::Rename);
        assert_eq!(lines[line("Ungroup")], RowLine::Run(Command::Ungroup));
        assert!(!items[line("Ungroup")].enabled, "nothing there to take apart");
        assert!(items[line("Group")].enabled);
        assert_eq!(items[line("Duplicate")].hint.as_deref(), Some("Ctrl+J"));
        assert_eq!(items[line("Rename")].hint.as_deref(), Some("F2"));
        assert_eq!(lines[line("Copy")], RowLine::Copy);
        assert_eq!(lines[line("Cut")], RowLine::Cut);
        assert_eq!(lines[line("Paste")], RowLine::Paste);
        assert!(!items[line("Paste")].enabled, "nothing to paste");
        assert!(items[line("Copy")].rule);
        assert_eq!(lines[line("Red")], RowLine::Tag(Tag::Red));
        assert!(items[line("Red")].checked);
        assert!(items[line("Group")].rule && items[line("Lock")].rule && items[line("No Color")].rule);
        // The toggles say what they would do.
        let (items, _) = row_menu(
            |_| true,
            true,
            RowState {
                locked: true,
                hidden: true,
                tag: Tag::None,
            },
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"Unlock") && labels.contains(&"Show"));
    }

    #[test]
    fn a_tag_tints_the_eyes_cell() {
        let theme = Theme::light();
        let a = atlas();
        let mut doc = flat(2);
        doc.layers[1].color = Tag::Red;
        let p = laid(&doc, &[], VP, 1.0, 0.0);
        let prims = p.prims(&showing("L1", None), &a, 7, &theme);
        let tint = mix(theme.panel, tag_color(Tag::Red).unwrap(), TAG_TINT);
        let tinted: Vec<ScreenRect> = prims.iter().filter(|q| q.color == tint).map(Prim::bounds).collect();
        assert_eq!(tinted, [row(&p, "L2").eye]);
        assert!(tag_color(Tag::None).is_none());
    }

    #[test]
    fn the_filters_toggles_read_on_and_its_field_says_what_it_is_for() {
        let theme = Theme::light();
        let a = atlas();
        let p = filtered(&flat(2), 1.0);
        let search = p.search.unwrap();
        let mut f = Filter::default();
        f.toggle_kind(Kind::Vector);
        f.toggle_tag(Tag::Green);
        let show = Showing {
            filter: Some(&f),
            ..showing("L1", None)
        };
        let prims = p.prims(&show, &a, 7, &theme);
        let on: Vec<ScreenRect> = prims
            .iter()
            .filter(|q| q.color == theme.active_bg)
            .map(Prim::bounds)
            .collect();
        let of_kind = |k: Kind| p.kinds.iter().find(|(_, x)| *x == k).unwrap().0;
        let of_tag = |t: Tag| p.tags.iter().find(|(_, x)| *x == t).unwrap().0;
        assert!(on.contains(&of_kind(Kind::Vector)) && on.contains(&of_tag(Tag::Green)));
        assert!(!on.contains(&of_kind(Kind::Raster)) && !on.contains(&of_tag(Tag::Red)));
        for (_, t) in &p.tags {
            let c = tag_color(*t).unwrap();
            assert!(prims.iter().any(|q| q.color == c), "every colour shows its dot");
        }
        // An empty field says what it is for, muted; a name is written
        // in ink; a field with the keyboard is `app`'s to draw.
        let glyphs = |prims: &[Prim], color: Rgba| {
            prims
                .iter()
                .filter(|q| q.kind == KIND_IMAGE && q.slot == 7 && q.color == color)
                .filter(|q| search.contains_rect(&q.bounds()))
                .count()
        };
        let letters = PLACEHOLDER.chars().filter(|c| !c.is_whitespace()).count();
        assert_eq!(glyphs(&prims, theme.muted), letters);
        f.name = "sky".into();
        let show = Showing {
            filter: Some(&f),
            ..showing("L1", None)
        };
        assert_eq!(glyphs(&p.prims(&show, &a, 7, &theme), theme.ink), 3);
        let show = Showing {
            filter: Some(&f),
            searching: true,
            ..showing("L1", None)
        };
        let prims = p.prims(&show, &a, 7, &theme);
        assert_eq!(glyphs(&prims, theme.ink) + glyphs(&prims, theme.muted), 0);
        let text = p.search_text().unwrap();
        assert!(search.contains_rect(&text) && text.x > search.x, "after the glass");
    }
}
