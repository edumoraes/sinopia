//! Decoded bitmaps (ARCHITECTURE.md §7.1): clipboard or file bytes in,
//! RGBA8 out, with the size the board should give them.
//!
//! Pure — the blob store keeps the original bytes, `gfx` uploads the
//! texels, and neither belongs here.

use std::io::Cursor;

use anyhow::Context as _;

/// Widest and tallest an image may be, in pixels. Past this a paste is
/// refused instead of becoming a texture the GPU will not take: 8192 is
/// the `max_texture_dimension_2d` a desktop adapter is expected to have.
pub const MAX_SIDE: u32 = 8192;

/// A decoded image: tightly packed RGBA8, `w * h * 4` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// Decodes PNG, JPEG or WebP bytes. The format comes from the content,
/// never from what handed the bytes over: the clipboard may announce one
/// thing and deliver another. The size is checked before any texel is
/// allocated, so a few KiB cannot become gigabytes.
pub fn decode(bytes: &[u8]) -> anyhow::Result<Bitmap> {
    checked_size(bytes)?;
    let rgba = reader(bytes)?
        .decode()
        .context("decoding the image")?
        .to_rgba8();
    Ok(Bitmap {
        w: rgba.width(),
        h: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

/// What the header says the image measures, refused past the ceiling.
/// It is the question [`decode`] asks before it allocates, and the whole
/// of the question where the pixels are not wanted — checking bytes that
/// are about to be stored rather than drawn.
pub fn checked_size(bytes: &[u8]) -> anyhow::Result<(u32, u32)> {
    let (w, h) = reader(bytes)?
        .into_dimensions()
        .context("reading the image size")?;
    anyhow::ensure!(
        w <= MAX_SIDE && h <= MAX_SIDE,
        "image is too large: {w}x{h} px, over the {MAX_SIDE} px limit"
    );
    Ok((w, h))
}

/// A reader over `bytes` with the format taken from the content. The
/// dimensions are left to [`decode`], which refuses an oversized image in
/// its own words; what stays here is the allocation ceiling, the backstop
/// for a header that lies.
fn reader(bytes: &[u8]) -> anyhow::Result<image::ImageReader<Cursor<&[u8]>>> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("reading the image header")?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(4 * u64::from(MAX_SIDE) * u64::from(MAX_SIDE));
    reader.limits(limits);
    Ok(reader)
}

/// The box a bitmap of `px` pixels takes in world units: one pixel per
/// unit — a screenshot pasted at zoom 1 shows at the size it was taken —
/// shrunk to fit inside `max` when it overflows, aspect kept. A `max` of
/// zero (a viewport not measured yet) imposes nothing.
pub fn fit_size(px: (u32, u32), max: (f64, f64)) -> (f64, f64) {
    let (w, h) = (f64::from(px.0.max(1)), f64::from(px.1.max(1)));
    let room = |side: f64, room: f64| {
        if room > 0.0 {
            room / side
        } else {
            f64::INFINITY
        }
    };
    let scale = room(w, max.0).min(room(h, max.1)).min(1.0);
    (w * scale, h * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        encoded(w, h, image::ImageFormat::Png)
    }

    fn encoded(w: u32, h: u32, format: image::ImageFormat) -> Vec<u8> {
        let buf = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(buf)
            .write_to(&mut std::io::Cursor::new(&mut out), format)
            .unwrap();
        out
    }

    #[test]
    fn decodes_a_png_to_rgba8() {
        let bmp = decode(&png(3, 2)).unwrap();
        assert_eq!((bmp.w, bmp.h), (3, 2));
        assert_eq!(bmp.rgba.len(), 3 * 2 * 4);
        assert_eq!(&bmp.rgba[..4], &[10, 20, 30, 255]);
    }

    #[test]
    fn decodes_a_jpeg_by_its_content_not_its_label() {
        // What the clipboard announces and what it hands over can differ;
        // the bytes decide.
        let bmp = decode(&encoded(4, 4, image::ImageFormat::Jpeg)).unwrap();
        assert_eq!((bmp.w, bmp.h), (4, 4));
    }

    #[test]
    fn bytes_that_are_not_an_image_are_an_error() {
        let err = decode(b"not an image at all").unwrap_err().to_string();
        assert!(err.contains("image"), "{err}");
    }

    #[test]
    fn an_image_past_the_side_limit_is_refused_before_it_is_decoded() {
        // A few KiB of PNG must not become gigabytes of texels.
        let wide = png(MAX_SIDE + 1, 1);
        assert!(wide.len() < 100_000, "fixture got big: {}", wide.len());
        let err = decode(&wide).unwrap_err().to_string();
        assert!(err.contains("large") || err.contains("limit"), "{err}");
    }

    #[test]
    fn an_image_smaller_than_the_room_keeps_its_pixel_size() {
        // One image pixel is one world unit: a screenshot pasted at zoom 1
        // shows at the size it was taken.
        assert_eq!(fit_size((320, 200), (800.0, 600.0)), (320.0, 200.0));
    }

    #[test]
    fn a_wide_image_shrinks_to_the_room_keeping_its_aspect() {
        assert_eq!(fit_size((1600, 400), (800.0, 600.0)), (800.0, 200.0));
    }

    #[test]
    fn a_tall_image_shrinks_by_its_own_axis() {
        assert_eq!(fit_size((400, 1200), (800.0, 600.0)), (200.0, 600.0));
    }

    #[test]
    fn shrinking_answers_to_whichever_axis_is_tighter() {
        assert_eq!(fit_size((1000, 1000), (800.0, 600.0)), (600.0, 600.0));
    }

    #[test]
    fn a_room_of_nothing_still_gives_a_usable_box() {
        // A window one pixel tall, or a viewport not measured yet: the
        // paste must not come out zero-sized or NaN.
        let (w, h) = fit_size((100, 50), (0.0, 0.0));
        assert!(w > 0.0 && h > 0.0, "{w} x {h}");
        assert!((w / h - 2.0).abs() < 1e-9, "aspect kept: {w} x {h}");
    }
}
