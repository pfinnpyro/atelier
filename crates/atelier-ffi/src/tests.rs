//! Calls the C interface the way a foreign caller would.

use super::*;

/// Two pages: a 100 x 50 point page with a red square at the top left, and a
/// 200 x 100 point page rotated 90 degrees.
fn sample() -> Vec<u8> {
    let content = b"1 0 0 rg 0 25 25 25 re f";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 50] /Resources << >> /Contents 4 0 R >>"
            .to_string(),
        format!(
            "<< /Length {} >>\nstream\n{}\nendstream",
            content.len(),
            std::str::from_utf8(content).unwrap()
        ),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Rotate 90 /Resources << >> /Contents 4 0 R >>"
            .to_string(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f\r\n", objects.len() + 1).as_bytes(),
    );
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n\r\n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

fn last_error() -> String {
    // SAFETY: a null buffer only asks for the length.
    let len = unsafe { atelier_last_error(std::ptr::null_mut(), 0) };
    let mut buf = vec![0u8; len + 1];
    // SAFETY: `buf` has len + 1 writable bytes.
    unsafe { atelier_last_error(buf.as_mut_ptr(), buf.len()) };
    buf.pop();
    String::from_utf8(buf).unwrap()
}

fn open(bytes: &[u8]) -> (AtelierStatus, *mut AtelierDocument) {
    let mut doc = std::ptr::null_mut();
    // SAFETY: valid slice and output pointer.
    let status = unsafe { atelier_document_open(bytes.as_ptr(), bytes.len(), &mut doc) };
    (status, doc)
}

#[test]
fn version_is_a_c_string() {
    // SAFETY: the pointer is static and NUL-terminated.
    let v = unsafe { std::ffi::CStr::from_ptr(atelier_version().cast()) };
    assert_eq!(v.to_str().unwrap(), env!("CARGO_PKG_VERSION"));
}

#[test]
fn info_and_sizes() {
    let (status, doc) = open(&sample());
    assert_eq!(status, AtelierStatus::Ok);
    let mut info = AtelierDocumentInfo::default();
    // SAFETY: live handle and valid output pointers throughout.
    unsafe {
        assert_eq!(atelier_document_info(doc, &mut info), AtelierStatus::Ok);
        assert_eq!(
            (info.page_count, info.version, info.encrypted, info.repaired),
            (2, 17, 0, 0)
        );

        let (mut w, mut h) = (0.0, 0.0);
        assert_eq!(atelier_page_size(doc, 0, &mut w, &mut h), AtelierStatus::Ok);
        assert_eq!((w, h), (100.0, 50.0));
        // Rotated 90 degrees, so width and height swap.
        assert_eq!(atelier_page_size(doc, 1, &mut w, &mut h), AtelierStatus::Ok);
        assert_eq!((w, h), (100.0, 200.0));

        let (mut pw, mut ph) = (0, 0);
        assert_eq!(
            atelier_page_pixel_size(doc, 0, 2.0, &mut pw, &mut ph),
            AtelierStatus::Ok
        );
        assert_eq!((pw, ph), (200, 100));

        assert_eq!(
            atelier_page_size(doc, 2, &mut w, &mut h),
            AtelierStatus::PageOutOfRange
        );
        assert!(last_error().contains("page 2 does not exist"));
        atelier_document_free(doc);
    }
}

#[test]
fn renders_each_pixel_format() {
    let (_, doc) = open(&sample());
    let (w, h) = (100usize, 50usize);
    for (format, red) in [
        (0, [255, 0, 0, 255]),
        (1, [255, 0, 0, 255]),
        (2, [0, 0, 255, 255]),
    ] {
        // Padded rows, as bitmaps on Windows often have.
        let stride = w * 4 + 8;
        let mut buf = vec![0xAAu8; stride * h];
        // SAFETY: live handle; `buf` has `buf.len()` writable bytes.
        let status = unsafe {
            atelier_render_page(
                doc,
                0,
                1.0,
                0xFFFF_FFFF,
                format,
                buf.as_mut_ptr(),
                stride,
                buf.len(),
            )
        };
        assert_eq!(status, AtelierStatus::Ok, "{}", last_error());
        let px = |x: usize, y: usize| &buf[y * stride + x * 4..y * stride + x * 4 + 4];
        assert_eq!(px(10, 10), red, "format {format}");
        assert_eq!(px(60, 40), [255, 255, 255, 255], "format {format}");
        // Padding is left alone.
        assert_eq!(&buf[w * 4..w * 4 + 8], &[0xAA; 8]);
    }
    // SAFETY: live handle, freed once.
    unsafe { atelier_document_free(doc) };
}

#[test]
fn transparent_background() {
    let (_, doc) = open(&sample());
    let mut buf = vec![0u8; 100 * 50 * 4];
    // SAFETY: live handle; `buf` has `buf.len()` writable bytes.
    unsafe {
        let status = atelier_render_page(doc, 0, 1.0, 0, 0, buf.as_mut_ptr(), 400, buf.len());
        assert_eq!(status, AtelierStatus::Ok);
        atelier_document_free(doc);
    }
    assert_eq!(buf[(40 * 100 + 60) * 4 + 3], 0);
}

#[test]
fn rejects_bad_arguments() {
    let (_, doc) = open(&sample());
    let mut buf = vec![0u8; 16];
    // SAFETY: live handle and valid pointers; the engine must refuse each call.
    unsafe {
        let render = |scale, format, buf: &mut [u8], stride| {
            atelier_render_page(
                doc,
                0,
                scale,
                0,
                format,
                buf.as_mut_ptr(),
                stride,
                buf.len(),
            )
        };
        assert_eq!(render(1.0, 0, &mut buf, 400), AtelierStatus::BufferTooSmall);
        assert_eq!(render(1.0, 0, &mut buf, 4), AtelierStatus::BufferTooSmall);
        assert_eq!(
            render(0.0, 0, &mut buf, 400),
            AtelierStatus::InvalidArgument
        );
        assert_eq!(
            render(f64::NAN, 0, &mut buf, 400),
            AtelierStatus::InvalidArgument
        );
        assert_eq!(
            render(1.0, 7, &mut buf, 400),
            AtelierStatus::InvalidArgument
        );
        assert_eq!(
            atelier_document_info(std::ptr::null(), std::ptr::null_mut()),
            AtelierStatus::InvalidArgument
        );
        assert_eq!(
            atelier_document_info(doc, std::ptr::null_mut()),
            AtelierStatus::InvalidArgument
        );
        atelier_document_free(doc);
        atelier_document_free(std::ptr::null_mut());
    }
}

#[test]
fn not_a_pdf() {
    let (status, doc) = open(b"hello");
    assert_ne!(status, AtelierStatus::Ok);
    assert!(doc.is_null());
    assert!(!last_error().is_empty());
}

#[test]
fn error_message_is_cut_to_fit() {
    set_error("abcdef");
    let mut buf = [0xFFu8; 4];
    // SAFETY: `buf` has 4 writable bytes.
    let len = unsafe { atelier_last_error(buf.as_mut_ptr(), buf.len()) };
    assert_eq!(len, 6);
    assert_eq!(&buf, b"abc\0");
}

#[test]
fn shared_across_threads() {
    let (_, doc) = open(&sample());
    let addr = doc as usize;
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(move || {
                let mut buf = vec![0u8; 100 * 50 * 4];
                // SAFETY: the handle outlives the scope and is Sync.
                let status = unsafe {
                    atelier_render_page(
                        addr as *const _,
                        0,
                        1.0,
                        !0,
                        0,
                        buf.as_mut_ptr(),
                        400,
                        buf.len(),
                    )
                };
                assert_eq!(status, AtelierStatus::Ok);
            });
        }
    });
    // SAFETY: all threads have finished.
    unsafe { atelier_document_free(doc) };
}
