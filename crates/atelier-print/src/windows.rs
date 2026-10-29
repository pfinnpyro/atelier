//! Printing on Windows through the print spooler (GDI). Each page is
//! drawn by Atelier's own renderer at up to 300 dpi and sent to the
//! printer in bands. All unsafe code in Atelier is in this file; each
//! block only calls documented Win32 functions with buffers it owns.
#![allow(unsafe_code)]

use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDCW, DEVMODEW, DIB_RGB_COLORS, DM_COLLATE,
    DM_COLOR, DM_COPIES, DM_DUPLEX, DM_IN_BUFFER, DM_IN_PROMPT, DM_ORIENTATION, DM_OUT_BUFFER,
    DMCOLOR_COLOR, DMCOLOR_MONOCHROME, DMDUP_HORIZONTAL, DMDUP_SIMPLEX, DMDUP_VERTICAL,
    DMORIENT_LANDSCAPE, DMORIENT_PORTRAIT, DeleteDC, GetDeviceCaps, HALFTONE, HDC, HORZRES,
    LOGPIXELSX, LOGPIXELSY, PHYSICALHEIGHT, PHYSICALOFFSETX, PHYSICALOFFSETY, PHYSICALWIDTH,
    ResetDCW, SRCCOPY, SetBrushOrgEx, SetStretchBltMode, StretchDIBits, VERTRES,
};
use windows_sys::Win32::Graphics::Printing::{
    ClosePrinter, DocumentPropertiesW, EnumPrintersW, GetDefaultPrinterW, OpenPrinterW,
    PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL, PRINTER_HANDLE, PRINTER_INFO_4W,
};
use windows_sys::Win32::Storage::Xps::{AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};

use crate::{Duplex, Options, Paper, Printer, Source, landscape, place, sheets};

/// Pages are drawn at no more than this many dots per inch; the printer
/// driver scales up from there.
const MAX_DPI: f64 = 300.0;
/// Rows sent to the printer at a time.
const BAND: usize = 256;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// A NUL-terminated wide string to a String.
///
/// # Safety
/// `p` is null or points at a NUL-terminated UTF-16 string.
unsafe fn from_wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    // SAFETY: the caller promises a terminated string.
    while unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: the n units before the terminator are readable.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) })
}

pub fn printers() -> Vec<Printer> {
    let default = default_printer().unwrap_or_default();
    let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
    let (mut needed, mut count) = (0u32, 0u32);
    // SAFETY: a size query with no buffer.
    unsafe { EnumPrintersW(flags, null(), 4, null_mut(), 0, &mut needed, &mut count) };
    if needed == 0 {
        return Vec::new();
    }
    // u64s keep the structures in the buffer aligned.
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: the buffer holds `needed` bytes.
    let ok = unsafe {
        EnumPrintersW(
            flags,
            null(),
            4,
            buf.as_mut_ptr().cast(),
            needed,
            &mut needed,
            &mut count,
        )
    };
    if ok == 0 {
        return Vec::new();
    }
    // SAFETY: on success the buffer starts with `count` PRINTER_INFO_4W.
    let infos = unsafe {
        std::slice::from_raw_parts(buf.as_ptr().cast::<PRINTER_INFO_4W>(), count as usize)
    };
    infos
        .iter()
        .map(|i| {
            // SAFETY: the names point into the same buffer, terminated.
            let name = unsafe { from_wide(i.pPrinterName) };
            Printer {
                default: name == default,
                name,
            }
        })
        .collect()
}

fn default_printer() -> Option<String> {
    let mut len = 0u32;
    // SAFETY: a size query with no buffer.
    unsafe { GetDefaultPrinterW(null_mut(), &mut len) };
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u16; len as usize];
    // SAFETY: the buffer holds `len` units.
    let ok = unsafe { GetDefaultPrinterW(buf.as_mut_ptr(), &mut len) };
    // SAFETY: on success the buffer holds a terminated name.
    (ok != 0).then(|| unsafe { from_wide(buf.as_ptr()) })
}

/// An open printer, closed when dropped.
struct Handle(PRINTER_HANDLE);

impl Handle {
    fn open(name: &[u16]) -> Option<Handle> {
        let mut h = PRINTER_HANDLE::default();
        // SAFETY: the name is terminated; `h` receives the handle.
        let ok = unsafe { OpenPrinterW(name.as_ptr(), &mut h, null()) };
        (ok != 0).then_some(Handle(h))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenPrinterW.
        unsafe { ClosePrinter(self.0) };
    }
}

/// A printer's settings (DEVMODE with the driver's own data after it), in
/// a buffer aligned for it.
struct DevMode {
    buf: Vec<u64>,
}

impl DevMode {
    /// The printer's current settings, or with `prompt` as the user changes
    /// them in the driver's dialog (None if cancelled).
    fn of(name: &[u16], prompt: Option<&DevMode>) -> Option<DevMode> {
        let printer = Handle::open(name)?;
        // SAFETY: a size query.
        let size = unsafe {
            DocumentPropertiesW(null_mut(), printer.0, name.as_ptr(), null_mut(), null(), 0)
        };
        if size <= 0 {
            return None;
        }
        let mut dm = DevMode {
            buf: vec![0u64; (size as usize).div_ceil(8)],
        };
        let (input, mode) = match prompt {
            Some(p) => (p.ptr(), DM_IN_BUFFER | DM_OUT_BUFFER | DM_IN_PROMPT),
            None => (null(), DM_OUT_BUFFER),
        };
        // SAFETY: the output buffer is the size the driver asked for.
        let r = unsafe {
            DocumentPropertiesW(
                null_mut(),
                printer.0,
                name.as_ptr(),
                dm.ptr_mut(),
                input,
                mode,
            )
        };
        // IDOK is 1; IDCANCEL is 2.
        (r == 1).then_some(dm)
    }

    fn ptr(&self) -> *const DEVMODEW {
        self.buf.as_ptr().cast()
    }

    fn ptr_mut(&mut self) -> *mut DEVMODEW {
        self.buf.as_mut_ptr().cast()
    }

    fn get(&mut self) -> &mut DEVMODEW {
        // SAFETY: the buffer is at least a DEVMODEW, aligned, and owned.
        unsafe { &mut *self.ptr_mut() }
    }

    fn set_orientation(&mut self, landscape: bool) {
        let dm = self.get();
        dm.dmFields |= DM_ORIENTATION;
        let o = if landscape {
            DMORIENT_LANDSCAPE
        } else {
            DMORIENT_PORTRAIT
        };
        dm.Anonymous1.Anonymous1.dmOrientation = o as i16;
    }

    fn apply(&mut self, options: &Options) {
        let dm = self.get();
        dm.dmFields |= DM_COPIES | DM_COLLATE | DM_COLOR | DM_DUPLEX;
        // Copies are sent one by one, so they come out in the order asked
        // for whatever the driver does.
        dm.Anonymous1.Anonymous1.dmCopies = 1;
        dm.dmCollate = 0;
        dm.dmColor = if options.color {
            DMCOLOR_COLOR
        } else {
            DMCOLOR_MONOCHROME
        };
        dm.dmDuplex = match options.duplex {
            Duplex::Off => DMDUP_SIMPLEX,
            Duplex::LongEdge => DMDUP_VERTICAL,
            Duplex::ShortEdge => DMDUP_HORIZONTAL,
        };
    }
}

/// A printer device context, deleted when dropped.
struct Dc(HDC);

impl Dc {
    fn create(name: &[u16], dm: &DevMode) -> Option<Dc> {
        let driver = wide("WINSPOOL");
        // SAFETY: the strings are terminated and the DEVMODE is complete.
        let dc = unsafe { CreateDCW(driver.as_ptr(), name.as_ptr(), null(), dm.ptr()) };
        (!dc.is_null()).then_some(Dc(dc))
    }

    fn cap(&self, index: u32) -> f64 {
        // SAFETY: a plain query on a live DC.
        f64::from(unsafe { GetDeviceCaps(self.0, index as _) })
    }

    /// The sheet as it is set up now, in points, and the dots per inch.
    fn paper(&self) -> (Paper, f64, f64) {
        let (dx, dy) = (self.cap(LOGPIXELSX).max(1.0), self.cap(LOGPIXELSY).max(1.0));
        let (w, h) = (self.cap(PHYSICALWIDTH), self.cap(PHYSICALHEIGHT));
        let (ox, oy) = (self.cap(PHYSICALOFFSETX), self.cap(PHYSICALOFFSETY));
        let (rw, rh) = (self.cap(HORZRES), self.cap(VERTRES));
        let pt = |v: f64, dpi: f64| v * 72.0 / dpi;
        let paper = Paper {
            size: (pt(w, dx), pt(h, dy)),
            printable: (pt(ox, dx), pt(oy, dy), pt(ox + rw, dx), pt(oy + rh, dy)),
        };
        (paper, dx, dy)
    }
}

impl Drop for Dc {
    fn drop(&mut self) {
        // SAFETY: the DC came from CreateDCW.
        unsafe { DeleteDC(self.0) };
    }
}

pub fn paper(printer: &str) -> Option<Paper> {
    let name = wide(printer);
    let mut dm = DevMode::of(&name, None)?;
    dm.set_orientation(false);
    let dc = Dc::create(&name, &dm)?;
    Some(dc.paper().0)
}

pub fn properties(printer: &str) -> Result<(), String> {
    let name = wide(printer);
    let current = DevMode::of(&name, None).ok_or("Could not read the printer's settings.")?;
    // The dialog's changes apply to this job's defaults in the driver.
    let _ = DevMode::of(&name, Some(&current));
    Ok(())
}

pub fn print(
    options: &Options,
    title: &str,
    source: &dyn Source,
    progress: &dyn Fn(usize, usize),
    cancel: &AtomicBool,
) -> Result<(), String> {
    let name = wide(&options.printer);
    let mut dm = DevMode::of(&name, None)
        .ok_or_else(|| format!("Could not open the printer “{}”.", options.printer))?;
    dm.apply(options);
    let order = sheets(options);
    let first = order[0];
    let mut turned = landscape(source.page_size(first), options.orientation);
    dm.set_orientation(turned);
    let dc = Dc::create(&name, &dm).ok_or("Could not start the printer.")?;
    let doc_name = wide(title);
    let info = DOCINFOW {
        cbSize: std::mem::size_of::<DOCINFOW>() as i32,
        lpszDocName: doc_name.as_ptr(),
        lpszOutput: null(),
        lpszDatatype: null(),
        fwType: 0,
    };
    // SAFETY: the DOCINFOW and its name outlive the call.
    if unsafe { StartDocW(dc.0, &info) } <= 0 {
        return Err("The printer would not take the job.".into());
    }
    let total = order.len();
    for (n, &page) in order.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            // SAFETY: a document is open on this DC.
            unsafe { AbortDoc(dc.0) };
            return Err("Printing was cancelled.".into());
        }
        progress(n, total);
        let size = source.page_size(page);
        let want = landscape(size, options.orientation);
        if want != turned {
            turned = want;
            dm.set_orientation(turned);
            // SAFETY: between pages, with a complete DEVMODE.
            unsafe { ResetDCW(dc.0, dm.ptr()) };
        }
        // SAFETY: a document is open on this DC.
        if unsafe { StartPage(dc.0) } <= 0 {
            // SAFETY: as above.
            unsafe { AbortDoc(dc.0) };
            return Err("The printer stopped taking pages.".into());
        }
        let sent = send_page(&dc, source, page, size, options);
        // SAFETY: a page is open on this DC.
        unsafe { EndPage(dc.0) };
        if let Err(e) = sent {
            // SAFETY: a document is open on this DC.
            unsafe { AbortDoc(dc.0) };
            return Err(e);
        }
    }
    // SAFETY: a document is open on this DC.
    unsafe { EndDoc(dc.0) };
    progress(total, total);
    Ok(())
}

/// Draws one page and sends it to the open printer page in bands.
fn send_page(
    dc: &Dc,
    source: &dyn Source,
    page: usize,
    size: (f64, f64),
    options: &Options,
) -> Result<(), String> {
    let (paper, dx, dy) = dc.paper();
    let spot = place(size, &paper, options.sizing);
    // Device pixels are counted from the printable area's corner.
    let (ox, oy) = (paper.printable.0, paper.printable.1);
    let x0 = ((spot.x - ox) * dx / 72.0).round() as i32;
    let y0 = ((spot.y - oy) * dy / 72.0).round() as i32;
    let dw = (spot.w * dx / 72.0).round().max(1.0) as i32;
    let dh = (spot.h * dy / 72.0).round().max(1.0) as i32;
    // Drawn at the printer's resolution or 300 dpi, whichever is less.
    let factor = spot.w / size.0.max(1.0);
    let dpi = dx.min(MAX_DPI);
    let scale = factor * dpi / 72.0;
    let pic = source
        .draw(page, scale)
        .ok_or_else(|| format!("Page {} could not be drawn.", page + 1))?;
    let (w, h) = (pic.width, pic.height);
    // SAFETY: plain settings on a live DC.
    unsafe {
        SetStretchBltMode(dc.0, HALFTONE);
        SetBrushOrgEx(dc.0, 0, 0, null_mut());
    }
    let mut row0 = 0;
    while row0 < h {
        let rows = BAND.min(h - row0);
        let bgr = band_bgr(&pic, row0, rows, options.color);
        let header = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            // Negative: rows run top to bottom.
            biHeight: -(rows as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        };
        let bmi = BITMAPINFO {
            bmiHeader: header,
            ..Default::default()
        };
        let top = y0 + (i64::from(dh) * row0 as i64 / h as i64) as i32;
        let bottom = y0 + (i64::from(dh) * (row0 + rows) as i64 / h as i64) as i32;
        // SAFETY: `bgr` holds `rows` rows of `w` 32-bit pixels, as the
        // header says.
        let done = unsafe {
            StretchDIBits(
                dc.0,
                x0,
                top,
                dw,
                bottom - top,
                0,
                0,
                w as i32,
                rows as i32,
                bgr.as_ptr().cast(),
                &bmi,
                DIB_RGB_COLORS,
                SRCCOPY,
            )
        };
        if done == 0 {
            return Err(format!("The printer refused part of page {}.", page + 1));
        }
        row0 += rows;
    }
    Ok(())
}

/// Rows of a page picture as the 32-bit BGR that GDI takes, over white,
/// in grey for black-and-white printing.
fn band_bgr(pic: &crate::Picture, row0: usize, rows: usize, color: bool) -> Vec<u8> {
    let w = pic.width;
    let src = &pic.rgba[row0 * w * 4..(row0 + rows) * w * 4];
    let mut out = Vec::with_capacity(src.len());
    for p in src.chunks_exact(4) {
        // Premultiplied: composite over white.
        let white = 255 - u32::from(p[3]);
        let (r, g, b) = (
            (u32::from(p[0]) + white).min(255),
            (u32::from(p[1]) + white).min(255),
            (u32::from(p[2]) + white).min(255),
        );
        if color {
            out.extend([b as u8, g as u8, r as u8, 0]);
        } else {
            let y = ((r * 299 + g * 587 + b * 114) / 1000) as u8;
            out.extend([y, y, y, 0]);
        }
    }
    out
}
