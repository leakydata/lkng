//! Turning a chosen photo into a publishable thumbnail.
//!
//! # EXIF is stripped by construction, not by parsing
//!
//! The image is drawn to a `<canvas>` and re-encoded. A canvas holds
//! *pixels*, not a file container — so orientation tags, camera model,
//! timestamps and **GPS coordinates** simply do not survive the round
//! trip. Nothing here parses EXIF, which means nothing here can miss a
//! variant of it.
//!
//! That matters more than usual: a phone photo routinely carries the exact
//! coordinates it was taken at. Publishing one to a public presence tile
//! would hand a scraper the precise location that the entire cell-and-
//! jitter design exists to withhold — a bypass of the app's central
//! privacy property via its most ordinary feature.
//!
//! # Size is enforced here, before anything is signed
//!
//! Every phone in a cell downloads every tile in it, so thumbnail bytes
//! are the one number that decides whether the grid is usable on mobile
//! data. The encoder steps quality down until the result fits, and gives
//! up rather than publishing something oversized.

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Blob, CanvasRenderingContext2d, HtmlCanvasElement, HtmlImageElement, Url};

/// Edge length of a published tile thumbnail, in pixels.
///
/// Small on purpose. It is displayed at roughly a third of a phone's width
/// in a 3-column grid, and every extra pixel is paid for by everyone
/// nearby.
pub const THUMB_PX: u32 = 256;

/// Hard ceiling, matching `lkng_presence::MAX_THUMBNAIL_BYTES`.
pub const MAX_BYTES: usize = 16 * 1024;

/// Quality ladder. Descending, so the first fit is the best fit.
const QUALITY_STEPS: &[f64] = &[0.82, 0.7, 0.6, 0.5, 0.4, 0.32];

#[derive(Debug)]
pub enum PhotoError {
    Decode,
    Encode,
    /// Could not reach the size limit even at the lowest quality.
    TooLarge(usize),
}

impl std::fmt::Display for PhotoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PhotoError::Decode => write!(f, "that image could not be read"),
            PhotoError::Encode => write!(f, "that image could not be processed"),
            PhotoError::TooLarge(n) => {
                write!(f, "still {n} bytes after compression — try a simpler photo")
            }
        }
    }
}

/// Decode, square-crop, downscale and re-encode a chosen file.
///
/// Returns WebP bytes under [`MAX_BYTES`], with no metadata of any kind.
pub async fn to_thumbnail(blob: &Blob) -> Result<Vec<u8>, PhotoError> {
    let url = Url::create_object_url_with_blob(blob).map_err(|_| PhotoError::Decode)?;
    let img = HtmlImageElement::new().map_err(|_| PhotoError::Decode)?;
    img.set_src(&url);

    // Wait for decode. `decode()` reports failure properly, unlike onload,
    // which fires for images the browser cannot actually rasterise.
    let decoded = JsFuture::from(img.decode()).await;
    let _ = Url::revoke_object_url(&url);
    decoded.map_err(|_| PhotoError::Decode)?;

    let (w, h) = (img.natural_width(), img.natural_height());
    if w == 0 || h == 0 {
        return Err(PhotoError::Decode);
    }

    // Centre-crop to a square first, so downscaling never distorts a face.
    let side = w.min(h);
    let (sx, sy) = ((w - side) / 2, (h - side) / 2);

    let doc = web_sys::window().and_then(|w| w.document()).ok_or(PhotoError::Encode)?;
    let canvas: HtmlCanvasElement = doc
        .create_element("canvas")
        .map_err(|_| PhotoError::Encode)?
        .dyn_into()
        .map_err(|_| PhotoError::Encode)?;
    canvas.set_width(THUMB_PX);
    canvas.set_height(THUMB_PX);

    let ctx: CanvasRenderingContext2d = canvas
        .get_context("2d")
        .map_err(|_| PhotoError::Encode)?
        .ok_or(PhotoError::Encode)?
        .dyn_into()
        .map_err(|_| PhotoError::Encode)?;
    ctx.set_image_smoothing_enabled(true);
    ctx.draw_image_with_html_image_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
        &img,
        sx as f64,
        sy as f64,
        side as f64,
        side as f64,
        0.0,
        0.0,
        THUMB_PX as f64,
        THUMB_PX as f64,
    )
    .map_err(|_| PhotoError::Encode)?;

    // Step quality down until it fits. WebP because it is markedly smaller
    // than JPEG at this size, and every byte is multiplied by everyone in
    // the cell.
    let mut last = 0usize;
    for q in QUALITY_STEPS {
        let data_url = canvas
            .to_data_url_with_type_and_encoder_options("image/webp", &JsValue::from_f64(*q))
            .map_err(|_| PhotoError::Encode)?;
        let bytes = decode_data_url(&data_url).ok_or(PhotoError::Encode)?;
        last = bytes.len();
        if bytes.len() <= MAX_BYTES {
            return Ok(bytes);
        }
    }
    Err(PhotoError::TooLarge(last))
}

/// Extract the bytes from a `data:` URL's base64 payload.
fn decode_data_url(url: &str) -> Option<Vec<u8>> {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let b64 = url.split(',').nth(1)?;
    let mut vals = Vec::with_capacity(b64.len());
    for ch in b64.bytes() {
        if ch == b'=' {
            break;
        }
        vals.push(T.iter().position(|&t| t == ch)? as u32);
    }
    let mut out = Vec::with_capacity(vals.len() * 3 / 4);
    for c in vals.chunks(4) {
        let mut n = 0u32;
        for (i, v) in c.iter().enumerate() {
            n |= v << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if c.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if c.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// Larger cap for an album photo — see `lkng_album::MAX_PHOTO_BYTES`.
const ALBUM_MAX_BYTES: usize = 256 * 1024;
/// Album photos are looked at properly, not as a grid thumbnail.
const ALBUM_PX: u32 = 1024;

/// Prepare a photo for a private album.
///
/// Same canvas round trip as [`to_thumbnail`], for the same reason: EXIF —
/// including the exact coordinates a phone records — cannot survive being
/// redrawn as pixels. That matters at least as much here as on a tile. An
/// album is shared with people the owner chose, but "chose" is not "trusts
/// with their home address", and a photo taken indoors carries one.
///
/// Larger and higher quality than a tile: an album photo is fetched
/// deliberately by a few people rather than pushed to everyone in a cell.
pub async fn to_album_photo(blob: &Blob) -> Result<Vec<u8>, PhotoError> {
    let url = Url::create_object_url_with_blob(blob).map_err(|_| PhotoError::Decode)?;
    let img = HtmlImageElement::new().map_err(|_| PhotoError::Decode)?;
    img.set_src(&url);
    let decoded = JsFuture::from(img.decode()).await;
    let _ = Url::revoke_object_url(&url);
    decoded.map_err(|_| PhotoError::Decode)?;

    let (w, h) = (img.natural_width(), img.natural_height());
    if w == 0 || h == 0 {
        return Err(PhotoError::Decode);
    }

    // Fit inside a square bound, preserving aspect: an album photo is looked
    // at, so cropping it to a square would throw away what it is of.
    let scale = (ALBUM_PX as f64 / w.max(h) as f64).min(1.0);
    let (dw, dh) = (((w as f64) * scale) as u32, ((h as f64) * scale) as u32);

    let doc = web_sys::window().and_then(|w| w.document()).ok_or(PhotoError::Encode)?;
    let canvas: HtmlCanvasElement = doc
        .create_element("canvas")
        .map_err(|_| PhotoError::Encode)?
        .dyn_into()
        .map_err(|_| PhotoError::Encode)?;
    canvas.set_width(dw.max(1));
    canvas.set_height(dh.max(1));

    let ctx: CanvasRenderingContext2d = canvas
        .get_context("2d")
        .map_err(|_| PhotoError::Encode)?
        .ok_or(PhotoError::Encode)?
        .dyn_into()
        .map_err(|_| PhotoError::Encode)?;
    ctx.set_image_smoothing_enabled(true);
    ctx.draw_image_with_html_image_element_and_dw_and_dh(&img, 0.0, 0.0, dw as f64, dh as f64)
        .map_err(|_| PhotoError::Encode)?;

    let mut last = 0usize;
    for q in [0.86, 0.78, 0.7, 0.6, 0.5, 0.4] {
        let data_url = canvas
            .to_data_url_with_type_and_encoder_options("image/webp", &JsValue::from_f64(q))
            .map_err(|_| PhotoError::Encode)?;
        let bytes = decode_data_url(&data_url).ok_or(PhotoError::Encode)?;
        last = bytes.len();
        if bytes.len() <= ALBUM_MAX_BYTES {
            return Ok(bytes);
        }
    }
    Err(PhotoError::TooLarge(last))
}
