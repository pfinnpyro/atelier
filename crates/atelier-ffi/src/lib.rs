//! A C interface to the Atelier engine, so other languages can use it.
//!
//! The .NET package in `dotnet/` calls these functions through P/Invoke, and
//! its C# declarations are generated from this file by `build.rs`.
//!
//! Rules every function follows:
//! - It returns an [`AtelierStatus`]. On anything but `Ok`, a message is kept
//!   for the calling thread and can be read with [`atelier_last_error`].
//! - Panics never cross the boundary; they come back as `Internal`.
//! - Pointers may be null; a null where a value is needed is `InvalidArgument`.
//! - Page numbers start at 0.
//! - A document handle can be used from several threads at once.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};

use atelier_raster::Color;
use atelier_render::{RenderOptions, page_transform, render_page};
use atelier_syntax::{Document, Error, Page};

/// Largest scale accepted: 64 device pixels per point is about 4600 dpi.
const MAX_SCALE: f64 = 64.0;
/// Largest bitmap one render may produce, in pixels (a 16384 x 16384 square).
const MAX_PIXELS: u64 = 1 << 28;

/// The result of every call.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtelierStatus {
    Ok = 0,
    /// A null pointer, a scale out of range or a bad pixel format.
    InvalidArgument = 1,
    /// The page number is past the last page.
    PageOutOfRange = 2,
    /// The file could not be read as a PDF, even after repair.
    InvalidPdf = 3,
    /// The file is encrypted; decryption comes in a later phase.
    Encrypted = 4,
    /// The file uses something this build does not handle yet.
    Unsupported = 5,
    /// The output buffer is smaller than `stride * height` or a row is too short.
    BufferTooSmall = 6,
    /// The bitmap would be larger than the engine allows.
    TooLarge = 7,
    /// A bug in the engine. The message says where.
    Internal = 99,
}

/// How rendered pixels are laid out in the caller's buffer. Every format is
/// 4 bytes per pixel, rows top down.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtelierPixelFormat {
    /// R, G, B, A with straight alpha. For PNG writers and most image libraries.
    Rgba8 = 0,
    /// R, G, B, A with premultiplied alpha. The engine's own format.
    Rgba8Premultiplied = 1,
    /// B, G, R, A with premultiplied alpha. WPF `Pbgra32`, GDI+
    /// `Format32bppPArgb`, Direct2D, WinUI and SkiaSharp `Bgra8888` premul.
    Bgra8Premultiplied = 2,
}

/// Facts about an open document.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct AtelierDocumentInfo {
    pub page_count: u32,
    /// 1 when the file is encrypted. Pages of encrypted files do not render yet.
    pub encrypted: u8,
    /// 1 when broken cross-reference data was rebuilt by scanning the file.
    pub repaired: u8,
    /// PDF version from the header, such as 17 for 1.7. 0 when there is none.
    pub version: u16,
}

/// An open PDF. Create with [`atelier_document_open`], release with
/// [`atelier_document_free`].
pub struct AtelierDocument {
    doc: Document,
}

thread_local! {
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

fn set_error(message: impl Into<String>) {
    LAST_ERROR.with(|e| *e.borrow_mut() = message.into());
}

/// Runs `f`, records any failure for [`atelier_last_error`] and turns panics
/// into [`AtelierStatus::Internal`].
fn guard(f: impl FnOnce() -> Result<(), (AtelierStatus, String)>) -> AtelierStatus {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(())) => {
            set_error("");
            AtelierStatus::Ok
        }
        Ok(Err((status, message))) => {
            set_error(message);
            status
        }
        Err(panic) => {
            let detail = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            set_error(format!("internal error: {detail}"));
            AtelierStatus::Internal
        }
    }
}

type Outcome<T> = Result<T, (AtelierStatus, String)>;

fn invalid<T>(message: &str) -> Outcome<T> {
    Err((AtelierStatus::InvalidArgument, message.to_string()))
}

fn engine_error(e: Error) -> (AtelierStatus, String) {
    let status = match e {
        Error::Encrypted => AtelierStatus::Encrypted,
        Error::Unsupported(_) => AtelierStatus::Unsupported,
        _ => AtelierStatus::InvalidPdf,
    };
    (status, e.to_string())
}

/// # Safety
/// `doc` is null or a live handle from [`atelier_document_open`].
unsafe fn document<'a>(doc: *const AtelierDocument) -> Outcome<&'a Document> {
    // SAFETY: the caller promises `doc` is null or live; null is checked here.
    match unsafe { doc.as_ref() } {
        Some(d) => Ok(&d.doc),
        None => invalid("document handle is null"),
    }
}

fn page(doc: &Document, index: u32) -> Outcome<&Page> {
    let pages = doc.pages().map_err(engine_error)?;
    pages.get(index as usize).ok_or_else(|| {
        (
            AtelierStatus::PageOutOfRange,
            format!(
                "page {index} does not exist (the file has {} pages, numbered from 0)",
                pages.len()
            ),
        )
    })
}

fn check_scale(scale: f64) -> Outcome<()> {
    if scale.is_finite() && scale > 0.0 && scale <= MAX_SCALE {
        Ok(())
    } else {
        invalid("scale must be above 0 and at most 64 (1.0 is 72 dpi)")
    }
}

/// Bitmap size for `page` at `scale`, refusing sizes over [`MAX_PIXELS`].
fn pixel_size(page: &Page, scale: f64) -> Outcome<(usize, usize)> {
    check_scale(scale)?;
    let (_, w, h) = page_transform(page, scale);
    if (w as u64).saturating_mul(h as u64) > MAX_PIXELS {
        return Err((
            AtelierStatus::TooLarge,
            format!("a {w} x {h} bitmap is larger than the engine allows; use a smaller scale"),
        ));
    }
    Ok((w, h))
}

/// # Safety
/// `out` is null or valid for writing one `T`.
unsafe fn write<T>(out: *mut T, value: T) -> Outcome<()> {
    if out.is_null() {
        return invalid("output pointer is null");
    }
    // SAFETY: non-null, and the caller promises it is valid for a write.
    unsafe { out.write(value) };
    Ok(())
}

/// The engine version as a NUL-terminated UTF-8 string. The pointer is static
/// and must not be freed.
#[unsafe(no_mangle)]
pub extern "C" fn atelier_version() -> *const u8 {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr()
}

/// Copies the calling thread's last error message into `buf` as UTF-8 with a
/// NUL at the end, cut to fit `len` bytes. Returns the message length in bytes
/// without the NUL, so a caller can pass a null `buf` first to ask the size.
/// The message is empty after a call that succeeded.
///
/// # Safety
/// `buf` is null or valid for writing `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn atelier_last_error(buf: *mut u8, len: usize) -> usize {
    LAST_ERROR.with(|e| {
        let e = e.borrow();
        let bytes = e.as_bytes();
        if !buf.is_null() && len > 0 {
            let n = bytes.len().min(len - 1);
            // SAFETY: the caller promises `len` writable bytes at `buf`, and
            // n + 1 <= len. The message is ours, so the ranges cannot overlap.
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, n);
                buf.add(n).write(0);
            }
        }
        bytes.len()
    })
}

/// Opens a PDF from bytes in memory. The bytes are copied, so the caller may
/// free them as soon as this returns. On success `*out` holds a new handle;
/// on failure it is set to null.
///
/// # Safety
/// `data` is valid for reading `len` bytes, and `out` is null or valid for
/// writing one pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn atelier_document_open(
    data: *const u8,
    len: usize,
    out: *mut *mut AtelierDocument,
) -> AtelierStatus {
    guard(|| {
        if out.is_null() {
            return invalid("output pointer is null");
        }
        // SAFETY: `out` is non-null and the caller promises it is writable.
        unsafe { out.write(std::ptr::null_mut()) };
        if data.is_null() {
            return invalid("data is null");
        }
        // SAFETY: the caller promises `len` readable bytes at `data`.
        let bytes = unsafe { std::slice::from_raw_parts(data, len) };
        let doc = Document::open(bytes.to_vec()).map_err(engine_error)?;
        let handle = Box::into_raw(Box::new(AtelierDocument { doc }));
        // SAFETY: as above.
        unsafe { out.write(handle) };
        Ok(())
    })
}

/// Releases a document. Null is ignored. The handle must not be used again.
///
/// # Safety
/// `doc` is null or a handle from [`atelier_document_open`] not freed yet,
/// and no other thread is using it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn atelier_document_free(doc: *mut AtelierDocument) {
    if !doc.is_null() {
        // SAFETY: the caller hands back ownership of a live handle.
        let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(doc) })));
    }
}

/// Fills `*out` with the page count, version and flags.
///
/// # Safety
/// `doc` is null or a live handle; `out` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn atelier_document_info(
    doc: *const AtelierDocument,
    out: *mut AtelierDocumentInfo,
) -> AtelierStatus {
    guard(|| {
        // SAFETY: forwarded from this function's contract.
        let d = unsafe { document(doc) }?;
        let pages = d.pages().map_err(engine_error)?;
        let version = d
            .version()
            .split_once('.')
            .and_then(|(a, b)| Some(a.parse::<u16>().ok()? * 10 + b.get(..1)?.parse::<u16>().ok()?))
            .unwrap_or(0);
        let info = AtelierDocumentInfo {
            page_count: u32::try_from(pages.len()).unwrap_or(u32::MAX),
            encrypted: d.is_encrypted().into(),
            repaired: d.was_repaired().into(),
            version,
        };
        // SAFETY: forwarded from this function's contract.
        unsafe { write(out, info) }
    })
}

/// The page's visible size in points (1/72 inch), after its crop box and
/// rotation are applied, so width and height are as the reader sees them.
///
/// # Safety
/// `doc` is null or a live handle; each output is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn atelier_page_size(
    doc: *const AtelierDocument,
    index: u32,
    out_width: *mut f64,
    out_height: *mut f64,
) -> AtelierStatus {
    guard(|| {
        // SAFETY: forwarded from this function's contract.
        let p = page(unsafe { document(doc) }?, index)?;
        let (w, h) = (p.crop_box.width(), p.crop_box.height());
        let (w, h) = if p.rotate % 180 == 90 { (h, w) } else { (w, h) };
        // SAFETY: forwarded from this function's contract.
        unsafe { write(out_width, w) }?;
        // SAFETY: forwarded from this function's contract.
        unsafe { write(out_height, h) }
    })
}

/// The bitmap size [`atelier_render_page`] produces for this page at `scale`
/// (device pixels per point; 1.0 is 72 dpi, 2.0833 is 150 dpi).
///
/// # Safety
/// `doc` is null or a live handle; each output is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn atelier_page_pixel_size(
    doc: *const AtelierDocument,
    index: u32,
    scale: f64,
    out_width: *mut u32,
    out_height: *mut u32,
) -> AtelierStatus {
    guard(|| {
        // SAFETY: forwarded from this function's contract.
        let p = page(unsafe { document(doc) }?, index)?;
        let (w, h) = pixel_size(p, scale)?;
        // Both fit: MAX_PIXELS keeps each side under 2^28.
        // SAFETY: forwarded from this function's contract.
        unsafe { write(out_width, w as u32) }?;
        // SAFETY: forwarded from this function's contract.
        unsafe { write(out_height, h as u32) }
    })
}

/// Renders a page into the caller's buffer. Ask for the size first with
/// [`atelier_page_pixel_size`]; the buffer needs `stride * height` bytes and
/// `stride` at least `width * 4`. `background` is 0xAARRGGBB with straight
/// alpha (0xFFFFFFFF is opaque white, 0 is transparent).
///
/// # Safety
/// `doc` is null or a live handle; `buf` is null or valid for writing
/// `buf_len` bytes and not read or written by anyone else during the call.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn atelier_render_page(
    doc: *const AtelierDocument,
    index: u32,
    scale: f64,
    background: u32,
    format: i32,
    buf: *mut u8,
    stride: usize,
    buf_len: usize,
) -> AtelierStatus {
    guard(|| {
        // SAFETY: forwarded from this function's contract.
        let d = unsafe { document(doc) }?;
        let p = page(d, index)?;
        let format = match format {
            0 => AtelierPixelFormat::Rgba8,
            1 => AtelierPixelFormat::Rgba8Premultiplied,
            2 => AtelierPixelFormat::Bgra8Premultiplied,
            _ => return invalid("unknown pixel format"),
        };
        let (w, h) = pixel_size(p, scale)?;
        if buf.is_null() {
            return invalid("buffer is null");
        }
        let row = w * 4;
        let needed = stride.checked_mul(h - 1).and_then(|n| n.checked_add(row));
        if stride < row || needed.is_none_or(|n| buf_len < n) {
            return Err((
                AtelierStatus::BufferTooSmall,
                format!(
                    "a {w} x {h} page needs stride >= {row} and {} bytes",
                    stride.saturating_mul(h)
                ),
            ));
        }
        // SAFETY: the caller promises `buf_len` writable bytes that nobody
        // else touches during the call.
        let out = unsafe { std::slice::from_raw_parts_mut(buf, buf_len) };

        let channel = |shift: u32| f32::from((background >> shift) as u8) / 255.0;
        let options = RenderOptions {
            scale,
            background: Color {
                r: channel(16),
                g: channel(8),
                b: channel(0),
                a: channel(24),
            },
        };
        let rendered = render_page(d, p, &options).map_err(engine_error)?;
        let pm = &rendered.pixmap;
        debug_assert_eq!((pm.width(), pm.height()), (w, h));
        let src = match format {
            AtelierPixelFormat::Rgba8 => std::borrow::Cow::Owned(pm.to_rgba()),
            _ => std::borrow::Cow::Borrowed(pm.data()),
        };
        for (y, src_row) in src.chunks_exact(row).enumerate() {
            let dst = &mut out[y * stride..y * stride + row];
            dst.copy_from_slice(src_row);
            if format == AtelierPixelFormat::Bgra8Premultiplied {
                for px in dst.chunks_exact_mut(4) {
                    px.swap(0, 2);
                }
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests;
