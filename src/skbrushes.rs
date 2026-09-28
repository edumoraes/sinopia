//! `sinopia brushes import`: Sketchbook's `.skbrushes` sets, read from a
//! copy the person downloaded, into a library the board can paint with.
//!
//! The binary carries none of Sketchbook's brushes. Their definitions,
//! icons, nibs and papers are Sketchbook's own, published for use with
//! its app, so they come in here — from the person's copy, into the
//! person's data directory — and never ship.
//!
//! A `.skbrushes` file is a zip of one `<brushPresets>` XML and a 40x40
//! icon per brush (plus an @2x copy). This reads the XML, maps every brush
//! onto the body `brush.rs` keeps, and writes, under `brushes/`:
//!
//!     library.json   the sets, their brushes, and the body
//!     icons.png      every icon @2x, one grid cell each
//!     shapes.png     every nib and every paper
//!
//! and keeps the sets it read in `brushes/sets/`, so that importing
//! another set later rebuilds the library from all of them.
//!
//! A brush with art of its own names a TIFF in the same zip, and the image
//! says its coverage in one of two places. The sets use both: one drawn
//! white on opaque black says it in its gray, and one drawn on
//! transparency says it in its alpha — the colour left there is whatever
//! it happened to be drawn in, and thirteen of the 114 nibs of the
//! standard sets are drawn in black, so reading the gray would take them
//! for empty. Either way what comes out is a cell of gray + alpha, the
//! gray full and the alpha the coverage, exactly as the glyph atlas is
//! read.
//!
//! Sketchbook has three kinds of art and one sheet carries all of them.
//! Two belong to the nib: a `shape` is a silhouette stamped in place of a
//! round dab, a `texture` is a grain worn over one, and the library says
//! which a brush names. The third is the `paperTexture`, and it belongs
//! to the canvas rather than to the nib — the nib is dragged over it, so
//! it stands still while the nib turns, and one tile of it covers
//! hundreds of world units instead of one dab. A dab wearing a nib and a
//! paper has one sheet to sample.
//!
//! The mapping: `size` is `maxRadius` doubled (the `metaParameter` only
//! the legacy brushes carry is not a source), `opacity` is
//! `maxStrokeOpacity`, `flow` is `maxOpacity`, and each `pressure` is how
//! far the light end sits below the heavy one. Randomness is not a
//! fraction in Sketchbook — it throws a property by an amount in that
//! property's own unit — and is carried as it stands.
//!
//! Three things in the files are read and deliberately not carried, so
//! that nobody has to derive them twice:
//!
//!   * `hardnessEdge`, off on 25 of the standard brushes and every one of
//!     them carrying art, says whether the Edge slider reaches the nib's
//!     own image. The flag is plain; what it *does* to a silhouette is
//!     stated nowhere, and the sets only hint — the brushes that turn it
//!     off park their Edge at one of two values instead of spreading it
//!     over the eight the rest use. Reading it wrong would change 96 of
//!     the 103 shape brushes on an invention.
//!   * `tiltFactor`, off 1.0 on 28 brushes, scales what the barrel's lean
//!     does to the nib. What this engine reads from a lean is its
//!     *direction* — which way the barrel points, which is what turns the
//!     nib — and a factor cannot scale a direction.
//!   * `metaParameter`, on the legacy brushes, is the band their size
//!     slider ran over. Brush Properties has one band for every brush; a
//!     per-brush one is a different control, not a missing number.
//!
//! And a paper's brightness and contrast: nothing says what the numbers
//! mean, and the one band they could plainly be — the -100..100 every
//! such control uses — turns eight of the 49 standard papers solid black,
//! one of them under a brush named Textured Pencil. A brush that paints
//! nothing is not what anybody ships, so that reading is wrong, and the
//! canvas does not guess. The invert is a flag and says exactly what it
//! means, so it is baked into the cell: the same TIFF used both ways is
//! two cells under two names.
//!
//! What arrives is somebody else's file, so every read is capped: a set,
//! an entry of it, an image's side, and how many sets one import takes.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use image::{DynamicImage, GrayAlphaImage, ImageReader, Limits, LumaA, RgbaImage};
use serde::Serialize;

use crate::brush::{Brush, Dynamics, Jitter, OWN_SET};
use crate::doc::Pressure;
use crate::store::{self, Store};

/// The order the standard sets stand in the palette, by the name of the
/// file Sketchbook publishes each in. Anything not named here comes after,
/// alphabetically.
const ORDER: [&str; 17] = [
    "Basic",
    "Legacy",
    "Markers",
    "Fine+Art",
    "Traditional",
    "Designer",
    "Artist",
    "Pastel",
    "Half+Tone",
    "Texture+Essentials",
    "Texture",
    "Shape",
    "Synthetic+Paint",
    "Splatter",
    "Glow",
    "Smudge",
    "Colorless",
];

/// The @2x icon; the 1x copy is 40x40.
pub const ICON_PX: u32 = 80;
/// The shapes ship 128 to 1024 px square. A nib is a soft mask, not line
/// art, and 128 is past what the widest brush anyone paints with resolves.
pub const SHAPE_PX: u32 = 128;
/// A paper is not a nib. One tile of it covers hundreds of world units
/// rather than one dab, so a stroke crosses only a handful of its texels
/// and a nib's 128 would read as blobs. It rides on the same sheet all
/// the same, in a band of its own under the nibs, at a size that has to
/// divide the sheet's width.
pub const PAPER_PX: u32 = 192;
/// The narrowest the nib sheet stands, and the step it widens by when a
/// library has more art than that width holds under `SHEET_MAX`: both
/// cell sizes divide it.
const SHEET_STEP: u32 = 384;
const SHEET_MIN: u32 = 4 * SHEET_STEP;
/// The side no sheet goes past: what every GPU the board runs on takes,
/// and the ceiling `bitmap::decode` holds an image to.
const SHEET_MAX: u32 = 8192;
/// How many icons stand across the icon sheet, unless there are so many
/// that it would run past `SHEET_MAX` tall.
const ICON_COLS: u32 = 16;
/// Where an image keeps its coverage. The two ways the sets are drawn do
/// not blur into each other: every TIFF they ship is either opaque to
/// within a hair — the alpha averages 0.998 and up, which is antialiasing
/// at the border and nothing else — or plainly transparent, at 0.54 and
/// down.
const OPAQUE: f64 = 0.99;

/// A whole set, read into memory. The biggest Sketchbook publishes is six
/// megabytes.
const SET_MAX: u64 = 64 * 1024 * 1024;
/// One entry of a set, unpacked: a 1024 px RGBA TIFF is four megabytes.
const ENTRY_MAX: u64 = 64 * 1024 * 1024;
/// How many sets one import takes, the ones already imported included.
const SETS_MAX: usize = 512;
/// The longest a set's or a brush's name is kept.
const NAME_MAX: usize = 80;

/// What an import did, for the person who asked for it.
#[derive(Debug, Default)]
pub struct Report {
    /// The sets now in the library, with how many brushes each has.
    pub sets: Vec<(String, usize)>,
    pub brushes: usize,
    pub shapes: usize,
    pub grains: usize,
    pub papered: usize,
    /// Brushes whose art the set named and did not carry, or that did not
    /// fit on the sheet: they lay a plain round nib instead.
    pub missing: Vec<String>,
    /// Files handed over that were not sets, and why.
    pub skipped: Vec<String>,
}

/// Adds every set in `inputs` to the ones already imported and rebuilds
/// the library from all of them. An input is a `.skbrushes` file, a
/// directory holding some, or a zip of them — the way Sketchbook
/// publishes its Mega Set.
pub fn import(store: &Store, inputs: &[PathBuf]) -> anyhow::Result<Report> {
    let dir = store.imported_dir();
    let kept = dir.join("sets");
    store::create_private_dir(&dir)?;
    store::create_private_dir(&kept)?;

    let mut skipped = Vec::new();
    let mut found = Vec::new();
    for input in inputs {
        gather(input, &mut found, &mut skipped)?;
    }
    anyhow::ensure!(!found.is_empty(), "no Sketchbook brush set in {inputs:?}");
    for (stem, bytes) in &found {
        store::write_private_atomic(&kept.join(format!("{}.skbrushes", file_stem(stem))), bytes)?;
    }

    let mut sets = Vec::new();
    for entry in std::fs::read_dir(&kept).with_context(|| format!("reading {kept:?}"))? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "skbrushes")
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            sets.push((stem.to_owned(), read_capped(&path, SET_MAX)?));
        }
    }
    anyhow::ensure!(sets.len() <= SETS_MAX, "more than {SETS_MAX} sets");
    let built = build(sets)?;
    store::write_private_atomic(&dir.join(store::IMPORTED_ICONS), &built.icons)?;
    store::write_private_atomic(&dir.join(store::IMPORTED_SHAPES), &built.shapes)?;
    // The library last: it is what the board reads first, and it must not
    // name cells of a sheet that is still the one before.
    store::write_private_atomic(&dir.join(store::IMPORTED_LIBRARY), built.library.as_bytes())?;
    let mut report = built.report;
    report.skipped = skipped;
    Ok(report)
}

/// The sets `input` holds, by the name of the file each came in.
fn gather(
    input: &Path,
    found: &mut Vec<(String, Vec<u8>)>,
    skipped: &mut Vec<String>,
) -> anyhow::Result<()> {
    if input.is_dir() {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(input)
            .with_context(|| format!("reading {input:?}"))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "skbrushes"))
            .collect();
        paths.sort();
        for path in paths {
            gather(&path, found, skipped)?;
        }
        return Ok(());
    }
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("set")
        .to_owned();
    // Read from the file, not into memory: a zip of sets is as big as
    // everything in it — the Mega Set is 141 MB — and only a set itself
    // is held to `SET_MAX`.
    let file = std::fs::File::open(input).with_context(|| format!("opening {input:?}"))?;
    let mut zip = match zip::ZipArchive::new(file) {
        Ok(z) => z,
        Err(e) => {
            skipped.push(format!("{}: not a zip ({e})", input.display()));
            return Ok(());
        }
    };
    let names: Vec<String> = zip.file_names().map(str::to_owned).collect();
    if names.iter().any(|n| n.to_lowercase().ends_with(".xml")) {
        found.push((stem, read_capped(input, SET_MAX)?));
        return Ok(());
    }
    // A zip of sets: each one inside it, read out whole.
    let mut inner: Vec<&String> = names
        .iter()
        .filter(|n| n.to_lowercase().ends_with(".skbrushes"))
        .collect();
    inner.sort();
    if inner.is_empty() {
        skipped.push(format!("{}: holds no brush set", input.display()));
    }
    for name in inner {
        anyhow::ensure!(found.len() < SETS_MAX, "more than {SETS_MAX} sets");
        let set = entry(&mut zip, name, SET_MAX)?;
        let stem = Path::new(name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("set")
            .to_owned();
        found.push((stem, set));
    }
    Ok(())
}

/// A file, whole, as long as it is no longer than `cap`.
fn read_capped(path: &Path, cap: u64) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .with_context(|| format!("opening {path:?}"))?
        .take(cap + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {path:?}"))?;
    anyhow::ensure!(bytes.len() as u64 <= cap, "{path:?} is past {cap} bytes");
    Ok(bytes)
}

/// One entry of a zip, unpacked, as long as it is no longer than `cap` —
/// counted as it inflates, since the size a zip declares is its own word.
fn entry<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
    cap: u64,
) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    zip.by_name(name)
        .with_context(|| format!("no {name} in the set"))?
        .take(cap + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("unpacking {name}"))?;
    anyhow::ensure!(bytes.len() as u64 <= cap, "{name} is past {cap} bytes");
    Ok(bytes)
}

/// A set's file name as it is kept: what is not a letter, a digit, a
/// space, `+`, `-` or `_` becomes `_`, and it never starts with a dot.
fn file_stem(stem: &str) -> String {
    let kept: String = stem
        .chars()
        .take(NAME_MAX)
        .map(|c| {
            if c.is_alphanumeric() || " +-_".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    match kept.trim() {
        "" => "set".to_owned(),
        k => k.to_owned(),
    }
}

/// What an import writes: the library and its two sheets, encoded.
pub struct Built {
    pub library: String,
    pub icons: Vec<u8>,
    pub shapes: Vec<u8>,
    pub report: Report,
}

/// The library as `brush::Library::with_imported` reads it.
#[derive(Serialize)]
struct LibraryOut {
    icon_px: u32,
    icon_cols: u32,
    icons: usize,
    shape_px: u32,
    shape_cols: u32,
    shapes: Vec<String>,
    paper_px: u32,
    papers: Vec<String>,
    sets: Vec<SetOut>,
}

#[derive(Serialize)]
struct SetOut {
    name: String,
    brushes: Vec<BrushOut>,
}

#[derive(Serialize)]
struct BrushOut {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    icon: Option<usize>,
    brush: Brush,
    #[serde(skip_serializing_if = "Option::is_none")]
    factory: Option<Brush>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shape: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    grain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    paper: Option<PaperOut>,
}

#[derive(Serialize)]
struct PaperOut {
    name: String,
    period: f64,
}

/// The library `sets` make, each a set's file stem and its bytes. Pure:
/// the files are the caller's to read and to write.
pub fn build(mut sets: Vec<(String, Vec<u8>)>) -> anyhow::Result<Built> {
    let rank = |stem: &str| ORDER.iter().position(|o| *o == stem).unwrap_or(ORDER.len());
    sets.sort_by(|(a, _), (b, _)| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));

    let mut report = Report::default();
    let mut out = Vec::new();
    let mut icons: Vec<RgbaImage> = Vec::new();
    // Name to cell, in the order the art is met. A nib is shared by every
    // brush that names it, and the name is what a saved board keeps — an
    // index would move the day a set is added.
    let mut shapes: Vec<(String, GrayAlphaImage)> = Vec::new();
    // And the papers, keyed by the reading they were baked under, with
    // how wide the source of each is: what the brush's own scale
    // stretches.
    let mut papers: Vec<(String, GrayAlphaImage, f64)> = Vec::new();
    let mut taken: Vec<String> = vec![OWN_SET.to_owned()];

    for (stem, bytes) in sets {
        let mut zip = match zip::ZipArchive::new(Cursor::new(bytes)) {
            Ok(z) => z,
            Err(e) => {
                report.skipped.push(format!("{stem}: not a zip ({e})"));
                continue;
            }
        };
        let names: Vec<String> = zip.file_names().map(str::to_owned).collect();
        let Some(xml) = names.iter().find(|n| n.to_lowercase().ends_with(".xml")) else {
            report.skipped.push(format!("{stem}: no brush list"));
            continue;
        };
        let xml = String::from_utf8_lossy(&entry(&mut zip, xml, ENTRY_MAX)?).into_owned();
        let (title, brushes) = read_set(&xml);
        // The name the set gives itself, unless another set took it
        // first — many a set in the Mega Set calls itself what its
        // neighbours do — and then the name of the file it came in, and
        // only then a number.
        let own = clean(&title.unwrap_or_else(|| stem.clone())).replace('+', " ");
        let file = clean(&stem).replace(['+', '_'], " ");
        let title = if !own.is_empty() && !taken.contains(&own) {
            own
        } else if !file.is_empty() && !taken.contains(&file) {
            file
        } else {
            unique(&own, &taken)
        };
        taken.push(title.clone());

        // A TIFF by the stem a brush names it by, and only among the
        // TIFFs: one set ships a shape named after the set itself, and
        // its `.xml` would otherwise answer first.
        let tiff = |stem: &str| {
            names
                .iter()
                .find(|n| {
                    let lower = n.to_lowercase();
                    (lower.ends_with(".tif") || lower.ends_with(".tiff"))
                        && n.rsplit_once('.').map(|(s, _)| s) == Some(stem)
                })
                .cloned()
        };
        let mut kept = Vec::new();
        for raw in brushes {
            let mut b = BrushOut {
                name: clean(&raw.name),
                icon: None,
                brush: raw.brush,
                factory: raw.factory,
                shape: None,
                grain: None,
                paper: None,
            };
            let label = format!("{title}/{}", b.name);
            let icon = format!("{}@2x.png", raw.icon);
            if !raw.icon.is_empty() && names.contains(&icon) {
                match decode(&entry(&mut zip, &icon, ENTRY_MAX)?) {
                    Ok(img) => {
                        b.icon = Some(icons.len());
                        icons.push(icon_cell(&img));
                    }
                    Err(e) => report.missing.push(format!("{label}: icon ({e:#})")),
                }
            }
            if let Some(art) = raw.art {
                let known = shapes.iter().any(|(n, _)| *n == art.name);
                let found = known
                    || match tiff(&art.name) {
                        Some(file) => match decode(&entry(&mut zip, &file, ENTRY_MAX)?) {
                            Ok(img) => {
                                shapes.push((art.name.clone(), art_cell(&img, SHAPE_PX, false)));
                                true
                            }
                            Err(e) => {
                                report.missing.push(format!("{label}: nib ({e:#})"));
                                false
                            }
                        },
                        None => {
                            report.missing.push(format!("{label}: nib {}", art.name));
                            false
                        }
                    };
                if found {
                    match art.kind {
                        ArtKind::Shape => b.shape = Some(art.name),
                        ArtKind::Grain => b.grain = Some(art.name),
                    }
                }
            }
            if let Some(paper) = raw.paper {
                let key = paper.key();
                let width = match papers.iter().find(|(n, ..)| *n == key) {
                    Some((.., width)) => Some(*width),
                    None => match tiff(&paper.stem) {
                        Some(file) => match decode(&entry(&mut zip, &file, ENTRY_MAX)?) {
                            Ok(img) => {
                                let width = f64::from(img.width());
                                papers.push((key.clone(), art_cell(&img, PAPER_PX, paper.invert), width));
                                Some(width)
                            }
                            Err(e) => {
                                report.missing.push(format!("{label}: paper ({e:#})"));
                                None
                            }
                        },
                        None => {
                            report.missing.push(format!("{label}: paper {}", paper.stem));
                            None
                        }
                    },
                };
                // One tile in world units: the image's own size,
                // stretched by what the brush asks for.
                if let Some(width) = width {
                    let period = round(width * paper.scale, 2);
                    if period.is_finite() && period > 0.0 {
                        b.paper = Some(PaperOut { name: key, period });
                    }
                }
            }
            kept.push(b);
        }
        report.sets.push((title.clone(), kept.len()));
        if !kept.is_empty() {
            out.push(SetOut {
                name: title,
                brushes: kept,
            });
        }
    }
    anyhow::ensure!(!out.is_empty(), "no brush in any of the sets");

    // The art that fits under the ceiling stays; a brush naming art that
    // did not is left a plain round nib, and says so.
    let (sheet_w, shapes_kept, papers_kept) = stamp_layout(shapes.len(), papers.len());
    let dropped_shapes: Vec<String> = shapes.drain(shapes_kept..).map(|(n, _)| n).collect();
    let dropped_papers: Vec<String> = papers.drain(papers_kept..).map(|(n, ..)| n).collect();
    for set in &mut out {
        for b in &mut set.brushes {
            for art in [&mut b.shape, &mut b.grain] {
                if art.as_ref().is_some_and(|n| dropped_shapes.contains(n)) {
                    report.missing.push(format!("{}/{}: no room for its nib", set.name, b.name));
                    *art = None;
                }
            }
            if b.paper.as_ref().is_some_and(|p| dropped_papers.contains(&p.name)) {
                report.missing.push(format!("{}/{}: no room for its paper", set.name, b.name));
                b.paper = None;
            }
        }
    }

    let icon_cols = icon_cols(icons.len());
    let icons_png = encode_icons(&icons, icon_cols)?;
    let shapes_png = encode_stamps(&shapes, &papers, sheet_w)?;
    for set in &out {
        for b in &set.brushes {
            report.brushes += 1;
            report.shapes += usize::from(b.shape.is_some());
            report.grains += usize::from(b.grain.is_some());
            report.papered += usize::from(b.paper.is_some());
        }
    }
    let library = LibraryOut {
        icon_px: ICON_PX,
        icon_cols,
        icons: icons.len(),
        shape_px: SHAPE_PX,
        shape_cols: sheet_w / SHAPE_PX,
        shapes: shapes.into_iter().map(|(n, _)| n).collect(),
        paper_px: PAPER_PX,
        papers: papers.into_iter().map(|(n, ..)| n).collect(),
        sets: out,
    };
    Ok(Built {
        library: serde_json::to_string(&library)? + "\n",
        icons: icons_png,
        shapes: shapes_png,
        report,
    })
}

/// `name`, or `name 2`, `name 3`… — the first of those no set has taken.
fn unique(name: &str, taken: &[String]) -> String {
    let base = if name.is_empty() { "Imported" } else { name };
    (1..)
        .map(|n| if n == 1 { base.to_owned() } else { format!("{base} {n}") })
        .find(|n| !taken.contains(n))
        .expect("an unbounded range has a free name")
}

/// A name as the library shows it: no control characters, trimmed, and
/// no longer than `NAME_MAX`.
fn clean(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .take(NAME_MAX)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// How many icons stand across the sheet: sixteen, or as many as keep it
/// from running past the ceiling.
fn icon_cols(count: usize) -> u32 {
    let rows_max = (SHEET_MAX / ICON_PX) as usize;
    (count.div_ceil(rows_max) as u32).clamp(ICON_COLS, SHEET_MAX / ICON_PX)
}

/// How wide the nib sheet stands, and how many nibs and papers it has
/// room for: as narrow as holds them all, widening by a step at a time,
/// and at the widest, as many as fit — the nibs first, since a nib is
/// the dab itself and a paper only what it is dragged over.
fn stamp_layout(shapes: usize, papers: usize) -> (u32, usize, usize) {
    let tall = |w: u32, shapes: usize, papers: usize| {
        let nibs = shapes.div_ceil((w / SHAPE_PX) as usize) as u32 * SHAPE_PX;
        nibs + papers.div_ceil((w / PAPER_PX) as usize) as u32 * PAPER_PX
    };
    let mut w = SHEET_MIN;
    while w + SHEET_STEP <= SHEET_MAX && tall(w, shapes, papers) > SHEET_MAX {
        w += SHEET_STEP;
    }
    let shapes = shapes.min((SHEET_MAX / SHAPE_PX * (w / SHAPE_PX)) as usize);
    let mut papers = papers;
    while papers > 0 && tall(w, shapes, papers) > SHEET_MAX {
        papers -= 1;
    }
    (w, shapes, papers)
}

/// Every icon into a sheet of its own, row by row, as a PNG.
fn encode_icons(icons: &[RgbaImage], cols: u32) -> anyhow::Result<Vec<u8>> {
    let rows = (icons.len() as u32).div_ceil(cols).max(1);
    let mut sheet = RgbaImage::new(cols * ICON_PX, rows * ICON_PX);
    for (i, icon) in icons.iter().enumerate() {
        let i = i as u32;
        image::imageops::replace(
            &mut sheet,
            icon,
            i64::from(i % cols * ICON_PX),
            i64::from(i / cols * ICON_PX),
        );
    }
    png(DynamicImage::ImageRgba8(sheet))
}

/// The one sheet the canvas stamps from: the nibs in a `SHAPE_PX` grid,
/// and under them the papers in a band of their own, `PAPER_PX` a side.
/// Each is placed in the order it is given, so a name's cell is its place
/// in its own list and nothing else has to be recorded.
fn encode_stamps(
    shapes: &[(String, GrayAlphaImage)],
    papers: &[(String, GrayAlphaImage, f64)],
    w: u32,
) -> anyhow::Result<Vec<u8>> {
    let (cols, across) = (w / SHAPE_PX, w / PAPER_PX);
    let top = (shapes.len() as u32).div_ceil(cols) * SHAPE_PX;
    let h = (top + (papers.len() as u32).div_ceil(across) * PAPER_PX).max(1);
    let mut sheet = GrayAlphaImage::new(w, h);
    for (i, (_, art)) in shapes.iter().enumerate() {
        let i = i as u32;
        image::imageops::replace(
            &mut sheet,
            art,
            i64::from(i % cols * SHAPE_PX),
            i64::from(i / cols * SHAPE_PX),
        );
    }
    for (i, (_, art, _)) in papers.iter().enumerate() {
        let i = i as u32;
        image::imageops::replace(
            &mut sheet,
            art,
            i64::from(i % across * PAPER_PX),
            i64::from(top + i / across * PAPER_PX),
        );
    }
    png(DynamicImage::ImageLumaA8(sheet))
}

fn png(img: DynamicImage) -> anyhow::Result<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

/// An image out of a set: PNG or TIFF, by what its bytes say, held to the
/// sheet's ceiling before any texel is allocated.
fn decode(bytes: &[u8]) -> anyhow::Result<DynamicImage> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(SHEET_MAX);
    limits.max_image_height = Some(SHEET_MAX);
    limits.max_alloc = Some(ENTRY_MAX * 4);
    reader.limits(limits);
    Ok(reader.decode()?)
}

/// An icon at the sheet's cell size, whatever it arrived at.
fn icon_cell(img: &DynamicImage) -> RgbaImage {
    let rgba = img.to_rgba8();
    if rgba.dimensions() == (ICON_PX, ICON_PX) {
        return rgba;
    }
    image::imageops::resize(&rgba, ICON_PX, ICON_PX, image::imageops::FilterType::Lanczos3)
}

/// One image into a `px` square of gray + alpha: whichever channel it
/// keeps its coverage in goes into the alpha — inverted on the way when
/// the brush asks for it — and the gray stays full, which is what makes
/// `texel * ink` the mark, exactly as the glyph atlas works.
fn art_cell(img: &DynamicImage, px: u32, invert: bool) -> GrayAlphaImage {
    let coverage = coverage(img);
    let coverage = image::imageops::resize(&coverage, px, px, image::imageops::FilterType::Lanczos3);
    GrayAlphaImage::from_fn(px, px, |x, y| {
        let c = coverage.get_pixel(x, y).0[0];
        LumaA([255, if invert { 255 - c } else { c }])
    })
}

/// Where an image keeps its coverage: an opaque one in its gray, any
/// other in its alpha.
fn coverage(img: &DynamicImage) -> image::GrayImage {
    let rgba = img.to_rgba8();
    let texels = u64::from(rgba.width()) * u64::from(rgba.height());
    let alpha: u64 = rgba.pixels().map(|p| u64::from(p.0[3])).sum();
    let opaque = texels > 0 && alpha as f64 / (texels as f64 * 255.0) >= OPAQUE;
    if opaque {
        img.to_luma8()
    } else {
        image::GrayImage::from_fn(rgba.width(), rgba.height(), |x, y| {
            image::Luma([rgba.get_pixel(x, y).0[3]])
        })
    }
}

/// One brush as its set writes it, before its art is looked for.
#[derive(Debug, Clone, PartialEq)]
struct RawBrush {
    name: String,
    /// The icon's file name, less its `@2x.png`.
    icon: String,
    brush: Brush,
    /// What it shipped as, when that is not what it is.
    factory: Option<Brush>,
    art: Option<Art>,
    paper: Option<PaperRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArtKind {
    /// A silhouette stamped in place of a round dab.
    Shape,
    /// A grain worn over one.
    Grain,
}

/// The nib image a brush carries, by the TIFF's name without its
/// extension — which is what the sheet and a saved board call it.
#[derive(Debug, Clone, PartialEq)]
struct Art {
    kind: ArtKind,
    name: String,
}

/// The paper a brush drags its nib over: the TIFF it names, whether it is
/// used inverted, and how far one tile of it is stretched.
#[derive(Debug, Clone, PartialEq)]
struct PaperRef {
    stem: String,
    invert: bool,
    scale: f64,
}

impl PaperRef {
    /// What the sheet and a saved board call one paper: the TIFF's own
    /// name, and `~i` after it when the cell was baked inverted — so a
    /// name is one image and not one image under two readings.
    fn key(&self) -> String {
        if self.invert {
            format!("{}~i", self.stem)
        } else {
            self.stem.clone()
        }
    }
}

/// A set's title, and its brushes in the order it lists them.
fn read_set(xml: &str) -> (Option<String>, Vec<RawBrush>) {
    let title = tag(xml, "group").remove("name").filter(|n| !n.is_empty());
    let mut out = Vec::new();
    let mut rest = xml;
    const OPEN: &str = "<brush name=\"";
    while let Some(at) = rest.find(OPEN) {
        rest = &rest[at + OPEN.len()..];
        let Some(quote) = rest.find('"') else { break };
        let name = unescape(&rest[..quote]);
        let Some(body_at) = rest[quote..].strip_prefix("\">") else {
            continue;
        };
        let Some(end) = body_at.find("</brush>") else { break };
        let body = &body_at[..end];
        rest = &body_at[end..];
        // A `<brush>` carries its settings, then a `<default>` repeating
        // what it shipped as. Everything before `<default>` is the live
        // brush.
        let (live, factory) = match body.find("<default>") {
            Some(cut) => (&body[..cut], &body[cut..]),
            None => (body, body),
        };
        let person = tag(live, "personalized");
        let brush = read_brush(live);
        let shipped = read_brush(factory);
        out.push(RawBrush {
            name: person
                .get("name")
                .filter(|n| !n.is_empty())
                .cloned()
                .unwrap_or(name),
            icon: person.get("icon").cloned().unwrap_or_default(),
            brush,
            // Every set ships its brushes at their factory settings, so
            // the copy is only kept when one of them does not.
            factory: (shipped != brush).then_some(shipped),
            art: art_of(live),
            paper: paper_of(live),
        });
    }
    (title, out)
}

/// One brush's body, out of the tags that carry it.
fn read_brush(body: &str) -> Brush {
    let stroke = tag(body, "strokeParameters");
    let params = tag(body, "brushParameters");
    let paper = tag(body, "paperTexture");
    // The size is the radius the brush reaches at full pressure; the
    // document speaks diameters.
    let max_radius = f(&params, "maxRadius", 8.0);
    let textured = paper.get("paperTextureEnabled").is_some_and(|v| v == "true");
    Brush {
        size: round(max_radius * 2.0, 3),
        opacity: round(f(&stroke, "maxStrokeOpacity", 1.0), 4),
        flow: round(f(&params, "maxOpacity", 1.0), 4),
        spacing: round(f(&stroke, "spacingBias", 1.0), 3),
        roundness: round(f(&params, "squish", 1.0), 3),
        rotation: round(f(&params, "angle", 0.0), 2),
        dynamics: dynamics(&stroke),
        hardness: round(f(&stroke, "hardness", 1.0), 4),
        profile: named(stroke.get("profile").map_or("regularSolid", String::as_str)),
        // What one dab does to the ink already down. Only the erasers
        // are a thing the canvas can do without reading the sheet back;
        // the rest are carried so the library can say what it is not
        // painting.
        mark: named(stroke.get("stampBlendStyle").map_or("normal", String::as_str)),
        texture_depth: if textured {
            round(f(&paper, "paperTextureDepthMax", 0.0), 3)
        } else {
            0.0
        },
        // What a dab does with the paint under it. Nothing reads the
        // canvas back yet, so these are carried and not painted.
        strength: round(f(&params, "strength", 0.0), 4),
        blending: round(f(&params, "blending", 0.0), 4),
        dilution: round(f(&params, "dilution", 0.0), 4),
        jitter: Jitter {
            size: round(f(&stroke, "radiusJitter", 0.0), 3),
            opacity: round(f(&stroke, "opacityJitter", 0.0), 3),
            flow: round(f(&stroke, "strokeOpacityJitter", 0.0), 3),
            rotation: round(f(&stroke, "rotationJitter", 0.0), 2),
            spacing: round(f(&stroke, "spacingNoise", 0.0), 3),
        },
        pressure: Pressure {
            size: round(driven(f(&params, "minRadius", 0.0), max_radius), 4),
            opacity: round(
                driven(
                    f(&stroke, "minStrokeOpacity", 0.0),
                    f(&stroke, "maxStrokeOpacity", 1.0),
                ),
                4,
            ),
            flow: round(
                driven(f(&params, "minOpacity", 0.0), f(&params, "maxOpacity", 1.0)),
                4,
            ),
        },
    }
}

/// A profile or a mark by the name the files give it; one this build
/// does not know is the default rather than a set that will not import.
fn named<T: serde::de::DeserializeOwned + Default>(name: &str) -> T {
    serde_json::from_value(serde_json::Value::String(name.to_owned())).unwrap_or_default()
}

/// Sketchbook's Rotation Dynamics: one choice, spread over two
/// attributes. Following the stroke wins over the stylus's tilt, which
/// is the order its own menu offers them in.
fn dynamics(stroke: &HashMap<String, String>) -> Dynamics {
    if stroke.get("rotateToStroke").is_some_and(|v| v == "true") {
        return Dynamics::ToStroke;
    }
    match stroke.get("tiltType").map(String::as_str) {
        Some("1") => Dynamics::Tilt,
        Some("2") => Dynamics::TiltAndRoll,
        _ => Dynamics::None,
    }
}

/// How much of a property the pen's pressure drives: the gap between
/// what a light touch gives and what a heavy one does, over the heavy
/// one. A brush whose two ends agree is one pressure does not touch.
fn driven(lo: f64, hi: f64) -> f64 {
    if hi <= 0.0 {
        return 0.0;
    }
    ((hi - lo) / hi).clamp(0.0, 1.0)
}

/// The nib image a brush carries, or none.
fn art_of(body: &str) -> Option<Art> {
    let custom = tag(body, "customBrush");
    if custom.get("type").is_none_or(|t| t == "off") {
        return None;
    }
    let kind = match custom.get("textureImageType")?.as_str() {
        "shape" => ArtKind::Shape,
        "texture" => ArtKind::Grain,
        _ => return None,
    };
    let name = custom.get("name")?;
    let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
    (!stem.is_empty()).then(|| Art {
        kind,
        name: stem.to_owned(),
    })
}

/// The paper a brush drags its nib over, or none.
fn paper_of(body: &str) -> Option<PaperRef> {
    let paper = tag(body, "paperTexture");
    if paper.get("paperTextureEnabled").is_none_or(|v| v != "true") {
        return None;
    }
    let name = paper.get("name").filter(|n| !n.is_empty())?;
    Some(PaperRef {
        stem: name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s).to_owned(),
        invert: paper.get("paperTextureInvert").is_some_and(|v| v == "true"),
        scale: f(&paper, "paperTextureScale", 1.0),
    })
}

/// The attributes of the first `<name …>` tag in `body`, or none.
fn tag(body: &str, name: &str) -> HashMap<String, String> {
    let open = format!("<{name}");
    let mut from = 0;
    while let Some(at) = body[from..].find(&open) {
        let start = from + at + open.len();
        let next = body[start..].chars().next();
        if matches!(next, Some(c) if c.is_whitespace() || c == '/' || c == '>') {
            let end = body[start..].find('>').map_or(body.len(), |e| start + e);
            return attrs(&body[start..end]);
        }
        from = start;
    }
    HashMap::new()
}

/// Every `key="value"` in a tag, the values unescaped.
fn attrs(tag_body: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut rest = tag_body;
    while let Some(eq) = rest.find("=\"") {
        let key: String = rest[..eq]
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let value_at = eq + 2;
        let Some(len) = rest[value_at..].find('"') else { break };
        if !key.is_empty() {
            out.insert(key, unescape(&rest[value_at..value_at + len]));
        }
        rest = &rest[value_at + len + 1..];
    }
    out
}

/// XML's five entities and its character references; anything else is
/// left as it was written.
fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let named = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match named {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A number out of an attribute, or `default` when it is absent or is not
/// a finite one.
fn f(attrs: &HashMap<String, String>, key: &str, default: f64) -> f64 {
    attrs
        .get(key)
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(default)
}

fn round(x: f64, places: i32) -> f64 {
    let k = 10f64.powi(places);
    (x * k).round() / k
}

/// What the library holds, in lines for the person who imported it.
pub fn describe(report: &Report, dir: &Path) -> String {
    let mut out = String::new();
    for (name, count) in &report.sets {
        out.push_str(&format!("  {name:28} {count:4} brushes\n"));
    }
    out.push_str(&format!(
        "\n{} brushes in {} sets: {} stamp a nib, {} wear a grain, {} are dragged over a paper\n",
        report.brushes,
        report.sets.len(),
        report.shapes,
        report.grains,
        report.papered
    ));
    if !report.missing.is_empty() {
        out.push_str(&format!(
            "{} brushes lay a plain nib where their set's art was missing: {}\n",
            report.missing.len(),
            report.missing.iter().take(3).cloned().collect::<Vec<_>>().join("; ")
        ));
    }
    for line in &report.skipped {
        out.push_str(&format!("skipped {line}\n"));
    }
    out.push_str(&format!("written to {}\n", dir.display()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Mark, Profile};
    use std::io::Write as _;

    /// A set written the way Sketchbook writes one, every name in it made
    /// up here: a round brush, one stamping a shape, one wearing a grain
    /// and dragged over an inverted paper, and one whose art is missing.
    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<brushPresets><group name="Made &amp; Up"/>
<brush name="first">
  <personalized name="Round &amp; Soft" icon="round-icon"/>
  <brushParameters maxRadius="12.5" minRadius="5" maxOpacity="0.8" minOpacity="0.2" squish="0.5" angle="30"/>
  <strokeParameters maxStrokeOpacity="0.9" minStrokeOpacity="0.9" spacingBias="0.4" hardness="0.75" profile="airbrush" tiltType="2"/>
  <customBrush type="off"/>
  <default>
    <brushParameters maxRadius="10"/>
    <strokeParameters maxStrokeOpacity="0.9"/>
  </default>
</brush>
<brush name="second">
  <personalized name="Stamp" icon="stamp-icon"/>
  <brushParameters maxRadius="20"/>
  <strokeParameters rotateToStroke="true" tiltType="1" stampBlendStyle="eraser" radiusJitter="3" profile="nonsense"/>
  <customBrush type="on" textureImageType="shape" name="star.tif"/>
</brush>
<brush name="third">
  <personalized icon="grain-icon"/>
  <brushParameters maxRadius="30" strength="0.5"/>
  <strokeParameters/>
  <customBrush type="on" textureImageType="texture" name="sand.tif"/>
  <paperTexture paperTextureEnabled="true" name="canvas.tif" paperTextureInvert="true" paperTextureScale="2" paperTextureDepthMax="0.6"/>
</brush>
<brush name="fourth">
  <personalized name="Lost" icon="no-such-icon"/>
  <brushParameters maxRadius="4"/>
  <strokeParameters/>
  <customBrush type="on" textureImageType="shape" name="gone.tif"/>
</brush>
</brushPresets>"#;

    fn tiff(img: DynamicImage) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Tiff).expect("tiff");
        out.into_inner()
    }

    fn png_of(img: DynamicImage) -> Vec<u8> {
        png(img).expect("png")
    }

    /// A white disc on opaque black, the way half the nibs are drawn.
    fn opaque_disc(px: u32) -> DynamicImage {
        let r = px as f32 / 2.0;
        DynamicImage::ImageRgba8(RgbaImage::from_fn(px, px, |x, y| {
            let d = ((x as f32 + 0.5 - r).powi(2) + (y as f32 + 0.5 - r).powi(2)).sqrt();
            let v = if d < r * 0.6 { 255 } else { 0 };
            image::Rgba([v, v, v, 255])
        }))
    }

    /// A black mark on transparency, the other half.
    fn clear_mark(px: u32) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_fn(px, px, |x, _| {
            image::Rgba([0, 0, 0, if x < px / 2 { 255 } else { 0 }])
        }))
    }

    fn set(xml: &str, files: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("Made+Up.xml", opts).expect("xml");
        zip.write_all(xml.as_bytes()).expect("xml");
        for (name, bytes) in files {
            zip.start_file(*name, opts).expect("file");
            zip.write_all(bytes).expect("file");
        }
        zip.finish().expect("zip").into_inner()
    }

    fn made_up() -> Vec<u8> {
        let icon = png_of(DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            80,
            80,
            image::Rgba([200, 30, 30, 255]),
        )));
        set(XML, &[
            ("round-icon@2x.png", icon.clone()),
            ("stamp-icon@2x.png", icon.clone()),
            ("grain-icon@2x.png", icon),
            ("star.tif", tiff(opaque_disc(256))),
            ("sand.tif", tiff(clear_mark(64))),
            ("canvas.tif", tiff(opaque_disc(300))),
        ])
    }

    #[test]
    fn a_tag_is_read_by_its_whole_name_and_its_values_unescaped() {
        let body = r#"<brushes x="1"/><brush name="A &amp; B &#233;&#x41;" size='x' empty=""/>"#;
        let t = tag(body, "brush");
        assert_eq!(t.get("name").map(String::as_str), Some("A & B éA"));
        assert_eq!(t.get("empty").map(String::as_str), Some(""));
        assert!(!t.contains_key("x"), "<brushes> is another tag");
        assert!(tag(body, "nothing").is_empty());
        assert_eq!(unescape("fish &chips; &bogus; &lt;"), "fish &chips; &bogus; <");
    }

    #[test]
    fn pressure_drives_the_gap_between_the_two_ends() {
        assert_eq!(driven(5.0, 10.0), 0.5);
        assert_eq!(driven(10.0, 10.0), 0.0, "two ends that agree");
        assert_eq!(driven(12.0, 10.0), 0.0, "never below nothing");
        assert_eq!(driven(0.0, 0.0), 0.0);
    }

    #[test]
    fn a_set_is_read_brush_by_brush_as_its_file_writes_it() {
        let (title, brushes) = read_set(XML);
        assert_eq!(title.as_deref(), Some("Made & Up"));
        assert_eq!(brushes.len(), 4);

        let round_ = &brushes[0];
        assert_eq!(round_.name, "Round & Soft", "the personalized name wins");
        assert_eq!(round_.icon, "round-icon");
        let b = round_.brush;
        assert_eq!(b.size, 25.0, "the radius, doubled");
        assert_eq!((b.flow, b.opacity, b.spacing), (0.8, 0.9, 0.4));
        assert_eq!((b.roundness, b.rotation, b.hardness), (0.5, 30.0, 0.75));
        assert_eq!(b.profile, Profile::Airbrush);
        assert_eq!(b.dynamics, Dynamics::TiltAndRoll);
        assert_eq!(b.pressure.size, 0.6, "5 of 12.5 at the lightest touch");
        assert_eq!(b.pressure.flow, 0.75);
        assert_eq!(b.pressure.opacity, 0.0, "the two ends agree");
        assert_eq!(round_.factory.map(|f| f.size), Some(20.0), "it shipped otherwise");
        assert_eq!(round_.art, None);

        let stamp = &brushes[1];
        assert_eq!(stamp.brush.dynamics, Dynamics::ToStroke, "following the stroke wins");
        assert_eq!(stamp.brush.mark, Mark::Erase);
        assert_eq!(stamp.brush.jitter.size, 3.0);
        assert_eq!(stamp.brush.profile, Profile::RegularSolid, "a profile it does not know");
        assert_eq!(stamp.factory, None, "no <default>: it is what it shipped as");
        assert_eq!(
            stamp.art,
            Some(Art {
                kind: ArtKind::Shape,
                name: "star".into(),
            })
        );

        let grain = &brushes[2];
        assert_eq!(grain.name, "third", "no personalized name: the brush's own");
        assert_eq!(grain.art.as_ref().map(|a| a.kind), Some(ArtKind::Grain));
        let paper = grain.paper.clone().expect("a paper");
        assert_eq!((paper.key(), paper.scale), ("canvas~i".to_owned(), 2.0));
        assert_eq!(grain.brush.texture_depth, 0.6);
        assert_eq!(grain.brush.strength, 0.5);
    }

    #[test]
    fn coverage_is_read_where_the_image_keeps_it() {
        let opaque = art_cell(&opaque_disc(256), 32, false);
        assert_eq!(opaque.get_pixel(16, 16).0, [255, 255], "white on black: the gray");
        assert_eq!(opaque.get_pixel(0, 0).0, [255, 0]);
        let clear = art_cell(&clear_mark(64), 32, false);
        assert_eq!(clear.get_pixel(2, 16).0, [255, 255], "black on nothing: the alpha");
        assert_eq!(clear.get_pixel(30, 16).0, [255, 0]);
        let inverted = art_cell(&opaque_disc(256), 32, true);
        assert_eq!(inverted.get_pixel(16, 16).0, [255, 0], "and a paper can be baked inverted");
    }

    #[test]
    fn a_set_becomes_a_library_the_board_reads() {
        let built = build(vec![("Made+Up".into(), made_up())]).expect("it builds");
        let r = &built.report;
        assert_eq!(r.sets, vec![("Made & Up".to_owned(), 4)]);
        assert_eq!((r.brushes, r.shapes, r.grains, r.papered), (4, 1, 1, 1));
        assert_eq!(r.missing.len(), 1, "the brush whose nib was not in the set");

        let lib = crate::brush::Library::with_imported(Some(&built.library));
        let names: Vec<&str> = lib.sets().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, [OWN_SET, "Made & Up"]);
        let imported = &lib.sets()[1].presets;
        assert_eq!(imported[1].shape.as_deref(), Some("star"));
        assert_eq!(imported[2].grain.as_deref(), Some("sand"));
        let paper = imported[2].paper.clone().expect("a paper");
        assert_eq!((paper.name.as_str(), paper.period), ("canvas~i", 600.0), "300 px, stretched twice");
        assert_eq!(imported[3].shape, None, "a nib that never came is no nib");
        assert!(
            matches!(imported[3].icon, crate::brush::Icon::Drawn),
            "an icon that never came is drawn"
        );
        let sheet = lib.sheet(3);
        assert!(sheet.cell("star").is_some() && sheet.cell("sand").is_some());
        assert!(sheet.paper("canvas~i").is_some());

        // And the sheets are images the board can decode.
        let icons = crate::bitmap::decode(&built.icons).expect("icons");
        assert_eq!((icons.w, icons.h), (16 * ICON_PX, ICON_PX));
        let stamps = crate::bitmap::decode(&built.shapes).expect("stamps");
        assert_eq!(stamps.w, SHEET_MIN);
        assert_eq!(stamps.h, SHAPE_PX + PAPER_PX, "a row of nibs, a band of papers");
    }

    #[test]
    fn sets_stand_in_sketchbooks_order_and_never_share_a_name() {
        let one = set(r#"<group name="Twin"/><brush name="a"><brushParameters/></brush>"#, &[]);
        let built = build(vec![
            ("Zebra".into(), one.clone()),
            ("Legacy".into(), one.clone()),
            ("Basic".into(), one.clone()),
            ("Sinopia".into(), set(r#"<brush name="b"><brushParameters/></brush>"#, &[])),
        ])
        .expect("it builds");
        let names: Vec<&str> = built.report.sets.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            ["Twin", "Legacy", "Sinopia 2", "Zebra"],
            "a name taken falls back on the file's, then on a number"
        );
    }

    #[test]
    fn what_is_not_a_set_is_skipped_and_nothing_at_all_is_refused() {
        let built = build(vec![
            ("junk".into(), b"not a zip".to_vec()),
            ("Made+Up".into(), made_up()),
        ])
        .expect("one good set is enough");
        assert_eq!(built.report.skipped.len(), 1);
        assert!(build(vec![("junk".into(), b"not a zip".to_vec())]).is_err());
    }

    #[test]
    fn a_sheet_widens_before_it_drops_anything_and_nibs_outlast_papers() {
        assert_eq!(stamp_layout(114, 32), (SHEET_MIN, 114, 32), "the standard sets fit");
        let (w, shapes, papers) = stamp_layout(3000, 600);
        assert!(w > SHEET_MIN && w <= SHEET_MAX && w % SHEET_STEP == 0);
        assert_eq!(shapes, 3000, "every nib fits once it widens");
        assert!(papers < 600, "the papers give way first");
        let rows = (shapes as u32).div_ceil(w / SHAPE_PX) * SHAPE_PX
            + (papers as u32).div_ceil(w / PAPER_PX) * PAPER_PX;
        assert!(rows <= SHEET_MAX);
        assert_eq!(icon_cols(211), ICON_COLS);
        assert!(icon_cols(5000) * ICON_PX <= SHEET_MAX);
        assert!((5000u32).div_ceil(icon_cols(5000)) * ICON_PX <= SHEET_MAX);
    }

    #[test]
    fn a_kept_file_name_is_tame() {
        assert_eq!(file_stem("Fine+Art"), "Fine+Art");
        assert_eq!(file_stem("../../etc/passwd"), "______etc_passwd");
        assert_eq!(file_stem(".hidden"), "_hidden");
        assert_eq!(file_stem("   "), "set");
    }

    #[test]
    fn an_import_keeps_its_sets_and_a_later_one_adds_to_them() {
        let root = tempfile::tempdir().expect("tmp");
        let store = Store::open(root.path().join("sinopia")).expect("store");
        let src = root.path().join("Made+Up.skbrushes");
        std::fs::write(&src, made_up()).expect("write");
        let first = import(&store, std::slice::from_ref(&src)).expect("import");
        assert_eq!(first.sets.len(), 1);

        // A zip of sets, the way the Mega Set comes.
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("Mega/Other.skbrushes", opts).expect("entry");
        zip.write_all(&set(r#"<group name="Other"/><brush name="o"><brushParameters/></brush>"#, &[]))
            .expect("entry");
        let mega = root.path().join("Mega.zip");
        std::fs::write(&mega, zip.finish().expect("zip").into_inner()).expect("write");
        let second = import(&store, &[mega]).expect("import");
        let names: Vec<&str> = second.sets.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Made & Up", "Other"], "the first set is still there");

        let imported = store.imported_brushes().expect("written");
        assert!(imported.sheets.icons.is_some() && imported.sheets.shapes.is_some());
        let lib = crate::brush::Library::with_imported(Some(&imported.library));
        assert_eq!(lib.sets().len(), 3);
        use std::os::unix::fs::PermissionsExt;
        for file in [store::IMPORTED_LIBRARY, store::IMPORTED_ICONS, store::IMPORTED_SHAPES] {
            let mode = std::fs::metadata(store.imported_dir().join(file))
                .expect("file")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{file}");
        }
        assert!(import(&store, &[root.path().join("nothing-here")]).is_err());
    }
}
