//! `render_page_fit_region` — the clipped-render entry point.
//!
//! `render_page_region` renders the whole page and crops the result, so it is
//! no help to a viewer whose problem IS the whole-page raster. This entry
//! point sizes the pixmap to the crop and post-translates the base transform,
//! so nothing outside the window is allocated or painted.
//!
//! # What "the same picture" means here, and why it is not byte equality
//!
//! One mechanism, and it is not the translate: **a path cut by the crop
//! boundary rasterizes differently than the same path whole.** tiny_skia
//! resolves antialiased coverage against the pixmap, so a glyph or stroke the
//! crop bisects gets a different edge from the one the full-page render gave
//! it — and the difference reaches as far INTO the crop as that path does, not
//! just to the boundary row.
//!
//! MEASURED, and this is the discriminating measurement rather than an
//! inference: on `sample.pdf` a 256x256 crop at `(256, 256)` cuts a line of
//! text and differs on 133 pixels in a 138x39 band hanging off its top edge,
//! while a 256x320 crop at `(256, 192)` — same content, same translate
//! magnitude, top edge moved clear of the glyphs — is **byte-identical**. A
//! crop of the full page, which has no interior boundary at all, is likewise
//! byte-identical. Deltas land on exact 1/16 steps (16, 32, 48), the
//! supersampler's coverage quantum.
//!
//! Over `1008.3918v2.pdf` page 0 tiled into 256 px windows: 66 / 465600
//! pixels differ at fit 600x800 (0.0142%, max channel delta 32) and
//! 1294 / 7454400 at fit 2400x3200 (0.0174%, max delta 64).
//!
//! The tolerance is 0.05% of pixels and a channel delta of 64. It still
//! discriminates: `an_off_by_one_crop_blows_the_tolerance` is the positive
//! control, and a window displaced by a single pixel differs on orders of
//! magnitude more than the bound allows.

use pdf_oxide::document::PdfDocument;
use pdf_oxide::rendering::{render_page_fit, render_page_fit_region, RenderOptions};

const FIT_W: u32 = 600;
const FIT_H: u32 = 800;
const MAX_DIFF_FRACTION: f64 = 0.0005;
const MAX_CHANNEL_DELTA: i32 = 64;

fn raw_opts() -> RenderOptions {
    RenderOptions::default().as_raw()
}

fn doc() -> PdfDocument {
    PdfDocument::open("tests/fixtures/1008.3918v2.pdf").expect("fixture opens")
}

/// Copy the `(x, y, w, h)` window out of a top-down RGBA buffer.
fn window(rgba: &[u8], full_w: u32, x: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h {
        let start = (((y + row) * full_w + x) * 4) as usize;
        out.extend_from_slice(&rgba[start..start + (w * 4) as usize]);
    }
    out
}

/// `(fraction of pixels that differ at all, largest single-channel delta)`.
fn compare(a: &[u8], b: &[u8]) -> (f64, i32) {
    assert_eq!(a.len(), b.len());
    let (mut differing, mut max_delta) = (0usize, 0i32);
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        let delta = (0..4)
            .map(|k| (pa[k] as i32 - pb[k] as i32).abs())
            .max()
            .unwrap_or(0);
        if delta > 0 {
            differing += 1;
            max_delta = max_delta.max(delta);
        }
    }
    (differing as f64 / (a.len() / 4) as f64, max_delta)
}

#[test]
fn a_crop_is_the_same_window_of_the_full_render() {
    let doc = doc();
    let opts = raw_opts();
    let full = render_page_fit(&doc, 0, FIT_W, FIT_H, &opts).expect("full render");

    // Interior, both single-axis offsets, and a 1x1 — an interior crop alone
    // would not catch a sign error in one axis of the translate.
    for (x, y, w, h) in [
        (128u32, 256u32, 256u32, 256u32),
        (0, 256, 256, 256),
        (128, 0, 256, 256),
        (411, 613, 1, 1),
    ] {
        let clipped = render_page_fit_region(&doc, 0, FIT_W, FIT_H, (x, y, w, h), &opts)
            .expect("clipped render");
        assert_eq!(
            (clipped.width, clipped.height),
            (w, h),
            "a clipped render returns the crop's own dimensions"
        );
        let (fraction, delta) = compare(&clipped.data, &window(&full.data, full.width, x, y, w, h));
        assert!(
            fraction <= MAX_DIFF_FRACTION && delta <= MAX_CHANNEL_DELTA,
            "crop ({x}, {y}, {w}, {h}): {:.4}% of pixels differ, max channel delta {delta} \
             (bounds {:.4}% / {MAX_CHANNEL_DELTA})",
            fraction * 100.0,
            MAX_DIFF_FRACTION * 100.0
        );
    }
}

#[test]
fn a_full_page_crop_is_byte_identical() {
    // The control for the tolerance above. A crop covering the whole page has
    // no interior boundary, so the one mechanism in the module doc is
    // switched off and the two paths must agree exactly. If this ever fails,
    // the difference is not clip-boundary coverage and the tolerance in the
    // sibling tests is hiding a real defect.
    let doc = doc();
    let opts = raw_opts();
    let full = render_page_fit(&doc, 0, FIT_W, FIT_H, &opts).expect("full render");
    let clipped =
        render_page_fit_region(&doc, 0, FIT_W, FIT_H, (0, 0, full.width, full.height), &opts)
            .expect("whole-page crop");
    assert_eq!((clipped.width, clipped.height), (full.width, full.height));
    assert_eq!(clipped.data, full.data);
}

#[test]
fn adjacent_crops_do_not_seam() {
    // The consequence that matters to a tiling caller. Tiles used to come
    // from one whole-page render, so a boundary between two of them could not
    // disagree by construction; rendered separately, each one's edge pixels
    // are resolved against its own pixmap bounds. If that produced a visible
    // discontinuity, tiling by clipped render would trade one artefact for
    // another.
    //
    // The two columns either side of the shared edge are compared against the
    // full render's own pixels there, which is what a seam would show up as.
    // This is the boundary the module doc's mechanism acts on, so it is the
    // one place the difference is guaranteed to be present rather than
    // incidental.
    let doc = doc();
    let opts = raw_opts();
    let full = render_page_fit(&doc, 0, FIT_W, FIT_H, &opts).expect("full render");
    let left = render_page_fit_region(&doc, 0, FIT_W, FIT_H, (0, 0, 256, 512), &opts)
        .expect("left crop");
    let right = render_page_fit_region(&doc, 0, FIT_W, FIT_H, (256, 0, 256, 512), &opts)
        .expect("right crop");

    let mut worst = 0i32;
    for row in 0..512u32 {
        for (data, w, col, page_col) in [
            (&left.data, 256u32, 255u32, 255u32),
            (&right.data, 256, 0, 256),
        ] {
            let i = ((row * w + col) * 4) as usize;
            let f = ((row * full.width + page_col) * 4) as usize;
            for k in 0..4 {
                worst = worst.max((data[i + k] as i32 - full.data[f + k] as i32).abs());
            }
        }
    }
    assert!(
        worst <= MAX_CHANNEL_DELTA,
        "the columns either side of a tile boundary differ from the whole-page          render by up to {worst}, which would read as a seam"
    );
}

#[test]
fn an_off_by_one_crop_blows_the_tolerance() {
    // Positive control: the bound in `a_crop_is_the_same_window_of_the_full_render`
    // is only worth having if a wrong window fails it. One pixel is the
    // smallest possible geometry error.
    let doc = doc();
    let opts = raw_opts();
    let full = render_page_fit(&doc, 0, FIT_W, FIT_H, &opts).expect("full render");
    let clipped = render_page_fit_region(&doc, 0, FIT_W, FIT_H, (128, 256, 256, 256), &opts)
        .expect("clipped render");
    let (fraction, _) = compare(&clipped.data, &window(&full.data, full.width, 129, 256, 256, 256));
    assert!(
        fraction > MAX_DIFF_FRACTION * 20.0,
        "a one-pixel displacement differs on only {:.4}% of pixels, so the tolerance \
         does not discriminate a wrong window",
        fraction * 100.0
    );
}

#[test]
fn a_crop_running_past_the_page_edge_is_clamped_not_refused() {
    // A viewport tiled into fixed-size tiles routinely asks for a last
    // column/row that overhangs. Clamping is the same answer the full-page
    // path gives that caller: it simply has no pixels out there.
    let doc = doc();
    let opts = raw_opts();
    let full = render_page_fit(&doc, 0, FIT_W, FIT_H, &opts).expect("full render");
    let (x, y) = (full.width - 10, full.height - 10);
    let clipped = render_page_fit_region(&doc, 0, FIT_W, FIT_H, (x, y, 256, 256), &opts)
        .expect("an overhanging crop renders");
    assert_eq!((clipped.width, clipped.height), (10, 10));
    let (fraction, delta) = compare(&clipped.data, &window(&full.data, full.width, x, y, 10, 10));
    assert!(fraction <= MAX_DIFF_FRACTION.max(0.01) && delta <= MAX_CHANNEL_DELTA);
}

#[test]
fn a_degenerate_crop_is_an_error_rather_than_a_zero_pixmap() {
    let doc = doc();
    let opts = raw_opts();
    assert!(render_page_fit_region(&doc, 0, FIT_W, FIT_H, (0, 0, 0, 64), &opts).is_err());
    assert!(render_page_fit_region(&doc, 0, FIT_W, FIT_H, (0, 0, 64, 0), &opts).is_err());
}

#[test]
fn page_fit_dimensions_predicts_the_full_render_without_rendering() {
    // `render_page_fit_region`'s crop is expressed in the pixels of the raster
    // `render_page_fit` produces, so a caller must be able to learn that
    // extent up front — the whole point of the region path is not paying for
    // the full render.
    let doc = doc();
    let opts = raw_opts();
    for (fw, fh) in [(600u32, 800u32), (777, 999), (1000, 1000), (2400, 3200)] {
        let full = render_page_fit(&doc, 0, fw, fh, &opts).expect("full render");
        assert_eq!(
            pdf_oxide::rendering::page_fit_dimensions(&doc, 0, fw, fh).expect("dimensions"),
            (full.width, full.height),
            "predicted dimensions for fit {fw}x{fh} must match the render"
        );
    }
}
