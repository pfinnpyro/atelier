use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use atelier_syntax::{Dict, Document, Error, Name, ObjRef, Object, Result, Stream, Update};
use eframe::egui::{self, Rect};

use super::Viewer;

mod canvas;
mod vector;

type Marks = BTreeMap<usize, Vec<Rect>>;
type PixelMask = [usize; 4];

struct RasterPage {
    page: ObjRef,
    content: ObjRef,
    image: ObjRef,
    size: [usize; 2],
    points: [f64; 2],
    digest: [u8; 32],
    masks: Vec<PixelMask>,
}

pub(super) struct Redaction {
    source: Arc<Document>,
    marks: Marks,
    history: Vec<Vec<(usize, usize)>>,
    drag: Option<canvas::Drag>,
    tool: canvas::Tool,
    saving: Option<mpsc::Receiver<std::result::Result<PathBuf, String>>>,
    mode: ExportMode,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExportMode {
    Vector,
    Raster,
}

impl ExportMode {
    fn label(self) -> &'static str {
        match self {
            Self::Vector => "Preserve text and vectors",
            Self::Raster => "Export page images",
        }
    }
}

fn name(value: &str) -> Object {
    Object::Name(Name::from(value))
}

// Start with an empty object graph so there is no source catalog, metadata,
// resource stream, attachment, action or incremental revision to carry over.
fn empty_pdf() -> Result<Document> {
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let catalog = pdf.len();
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let pages = pdf.len();
    pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
    let start = pdf.len();
    write!(&mut pdf, "xref\n0 3\n0000000000 65535 f \n{catalog:010} 00000 n \n{pages:010} 00000 n \ntrailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n").unwrap();
    Document::open(pdf)
}

fn image_page(
    update: &mut Update<'_>,
    rgb: Vec<u8>,
    pixels: [usize; 2],
    points: [f64; 2],
    masks: Vec<PixelMask>,
) -> (RasterPage, usize) {
    let digest = atelier_crypto::sha256(&rgb);
    let packed = atelier_filters::encode_flate(&rgb);
    let len = packed.len();
    let mut image = Dict::new();
    image.insert("Type", name("XObject"));
    image.insert("Subtype", name("Image"));
    image.insert("Width", Object::Integer(pixels[0] as i64));
    image.insert("Height", Object::Integer(pixels[1] as i64));
    image.insert("ColorSpace", name("DeviceRGB"));
    image.insert("BitsPerComponent", Object::Integer(8));
    image.insert("Filter", name("FlateDecode"));
    let image = update.add(Object::Stream(Stream {
        dict: image,
        data: packed.into(),
    }));
    let [w, h] = points;
    let content = update.add(Object::Stream(Stream {
        dict: Dict::new(),
        data: image_commands(points).into_bytes().into(),
    }));
    let mut resources = Dict::new();
    let mut images = Dict::new();
    images.insert("Scan", Object::Reference(image));
    resources.insert("XObject", Object::Dict(images));
    let mut page = Dict::new();
    page.insert("Type", name("Page"));
    page.insert("Parent", Object::Reference(ObjRef::new(2, 0)));
    page.insert(
        "MediaBox",
        Object::Array([0.0, 0.0, w, h].map(Object::Real).to_vec()),
    );
    page.insert("Resources", Object::Dict(resources));
    page.insert("Contents", Object::Reference(content));
    let page = update.add(Object::Dict(page));
    (
        RasterPage {
            page,
            content,
            image,
            size: pixels,
            points,
            digest,
            masks,
        },
        len,
    )
}

fn image_commands([width, height]: [f64; 2]) -> String {
    format!("q\n{width:.6} 0 0 {height:.6} 0 0 cm\n/Scan Do\nQ\n")
}

fn only_keys(dict: &Dict, keys: &[&[u8]]) -> bool {
    dict.len() == keys.len() && keys.iter().all(|key| dict.contains_key(key))
}

fn verify_raster_copy(copy: &Document, expected: &[RasterPage]) -> Result<()> {
    let error =
        || Error::Structure("The redacted copy failed verification. No file was saved.".into());
    let root = ObjRef::new(1, 0);
    let parent = ObjRef::new(2, 0);
    let refs = |dict: &Dict, key: &[u8]| dict.get(key).and_then(Object::as_reference);
    let number = |dict: &Dict, key: &[u8]| dict.get(key).and_then(Object::as_i64);
    let names = |dict: &Dict, key: &[u8], value: &[u8]| {
        dict.get(key)
            .and_then(Object::as_name)
            .is_some_and(|name| name.as_bytes() == value)
    };
    let numbers: Vec<_> = [root.num, parent.num]
        .into_iter()
        .chain(
            expected
                .iter()
                .flat_map(|p| [p.image.num, p.content.num, p.page.num]),
        )
        .collect();
    if copy.was_repaired()
        || !only_keys(copy.trailer(), &[b"Root", b"Size"])
        || refs(copy.trailer(), b"Root") != Some(root)
        || copy.object_numbers() != numbers
    {
        return Err(error());
    }
    let catalog = copy.catalog()?;
    if !only_keys(&catalog, &[b"Type", b"Pages"])
        || !catalog.is_type(b"Catalog")
        || refs(&catalog, b"Pages") != Some(parent)
    {
        return Err(error());
    }
    let Object::Dict(tree) = copy.get(parent)? else {
        return Err(error());
    };
    let kids: Vec<_> = expected
        .iter()
        .map(|page| Object::Reference(page.page))
        .collect();
    if !only_keys(&tree, &[b"Type", b"Kids", b"Count"])
        || !tree.is_type(b"Pages")
        || tree.get(b"Kids") != Some(&Object::Array(kids))
        || number(&tree, b"Count") != Some(expected.len() as i64)
    {
        return Err(error());
    }
    let pages = copy.pages()?;
    if pages.len() != expected.len() {
        return Err(error());
    }
    for (page, expected) in pages.iter().zip(expected) {
        let [width, height] = expected.size;
        let Object::Dict(images) = copy.get_key(&page.resources, b"XObject")? else {
            return Err(error());
        };
        if page.obj_ref != Some(expected.page)
            || !only_keys(
                &page.dict,
                &[b"Type", b"Parent", b"MediaBox", b"Resources", b"Contents"],
            )
            || !page.dict.is_type(b"Page")
            || refs(&page.dict, b"Parent") != Some(parent)
            || refs(&page.dict, b"Contents") != Some(expected.content)
            || !only_keys(&page.resources, &[b"XObject"])
            || !only_keys(&images, &[b"Scan"])
            || refs(&images, b"Scan") != Some(expected.image)
            || page.media_box.x0 != 0.0
            || page.media_box.y0 != 0.0
            || (page.media_box.width() - expected.points[0]).abs() > 0.000_01
            || (page.media_box.height() - expected.points[1]).abs() > 0.000_01
        {
            return Err(error());
        }
        let Object::Stream(content) = copy.get(expected.content)? else {
            return Err(error());
        };
        if !only_keys(&content.dict, &[b"Length"])
            || content.data.as_ref() != image_commands(expected.points).as_bytes()
        {
            return Err(error());
        }
        let Object::Stream(image) = copy.get(expected.image)? else {
            return Err(error());
        };
        if !only_keys(
            &image.dict,
            &[
                b"Type",
                b"Subtype",
                b"Width",
                b"Height",
                b"ColorSpace",
                b"BitsPerComponent",
                b"Filter",
                b"Length",
            ],
        ) || !image.dict.is_type(b"XObject")
            || !names(&image.dict, b"Subtype", b"Image")
            || !names(&image.dict, b"ColorSpace", b"DeviceRGB")
            || !names(&image.dict, b"Filter", b"FlateDecode")
            || number(&image.dict, b"BitsPerComponent") != Some(8)
            || number(&image.dict, b"Width") != Some(width as i64)
            || number(&image.dict, b"Height") != Some(height as i64)
        {
            return Err(error());
        }
        let rgb = copy.decode_stream(&image)?.data;
        if rgb.len() != width * height * 3 || atelier_crypto::sha256(&rgb) != expected.digest {
            return Err(error());
        }
        for &[x0, y0, x1, y1] in &expected.masks {
            for row in y0..y1 {
                if rgb[(row * width + x0) * 3..(row * width + x1) * 3]
                    .iter()
                    .any(|&value| value != 0)
                {
                    return Err(error());
                }
            }
        }
    }
    if !copy.warnings().is_empty() {
        return Err(error());
    }
    Ok(())
}

fn raster_copy(source: &Document, marks: &Marks) -> Result<Arc<[u8]>> {
    let pages = source.pages()?;
    if marks.values().all(Vec::is_empty) || marks.keys().any(|&page| page >= pages.len()) {
        return Err(Error::Structure(
            "Mark at least one valid region to redact".into(),
        ));
    }
    let seed = empty_pdf()?;
    let mut update = seed.update();
    let mut kids = Vec::new();
    let mut expected = Vec::with_capacity(pages.len());
    let mut bytes = 0;
    if pages.len() > 512 {
        return Err(Error::Unsupported(
            "Split this file into smaller parts before raster redaction (maximum 512 pages)".into(),
        ));
    }
    for (index, page) in pages.iter().enumerate() {
        let (w, h) = (page.crop_box.width(), page.crop_box.height());
        let points = if matches!(page.rotate, 90 | 270) {
            [h, w]
        } else {
            [w, h]
        };
        let options = atelier_render::RenderOptions {
            scale: (200.0_f64 / 72.0).min(3200.0 / w.max(h).max(1.0)),
            background: atelier_raster::Color::WHITE,
            ..Default::default()
        };
        let rendered = atelier_render::render_page(source, page, &options)?;
        if let Some(warning) = rendered.warnings.first() {
            return Err(Error::Structure(format!(
                "Page {} cannot be redacted reliably: {warning}",
                index + 1
            )));
        }
        let raster = rendered.pixmap;
        let (width, height) = (raster.width(), raster.height());
        let mut rgb = Vec::with_capacity(width * height * 3);
        for pixel in raster.data().as_chunks::<4>().0 {
            rgb.extend_from_slice(&pixel[..3]);
        }
        let mut masks = Vec::new();
        if let Some(rects) = marks.get(&index) {
            for &rect in rects {
                if !rect.is_finite()
                    || rect.min.x < 0.0
                    || rect.min.y < 0.0
                    || rect.max.x > 1.0
                    || rect.max.y > 1.0
                    || rect.width() <= 0.0
                    || rect.height() <= 0.0
                {
                    return Err(Error::Structure(format!(
                        "Invalid redaction rectangle on page {}",
                        index + 1
                    )));
                }
                let x0 =
                    ((f64::from(rect.min.x) * width as f64).floor() as usize).saturating_sub(8);
                let y0 =
                    ((f64::from(rect.min.y) * height as f64).floor() as usize).saturating_sub(8);
                let x1 = ((f64::from(rect.max.x) * width as f64).ceil() as usize + 8).min(width);
                let y1 = ((f64::from(rect.max.y) * height as f64).ceil() as usize + 8).min(height);
                masks.push([x0, y0, x1, y1]);
                for row in y0..y1 {
                    rgb[(row * width + x0) * 3..(row * width + x1) * 3].fill(0);
                }
            }
        }
        let (page, size) = image_page(&mut update, rgb, [width, height], points, masks);
        bytes += size;
        if bytes > 256 * 1024 * 1024 {
            return Err(Error::Unsupported(
                "The raster copy exceeds 256 MB. Split the file and try again.".into(),
            ));
        }
        kids.push(Object::Reference(page.page));
        expected.push(page);
    }
    set_page_tree(&mut update, kids);
    let bytes: Arc<[u8]> = update.to_compact_bytes(&HashSet::new())?.into();
    drop(update);
    let copy = Document::open(bytes.clone())?;
    verify_raster_copy(&copy, &expected)?;
    Ok(bytes)
}

fn set_page_tree(update: &mut Update<'_>, kids: Vec<Object>) {
    let mut tree = Dict::new();
    tree.insert("Type", name("Pages"));
    tree.insert("Count", Object::Integer(kids.len() as i64));
    tree.insert("Kids", Object::Array(kids));
    update.set(ObjRef::new(2, 0), Object::Dict(tree));
}

impl Viewer {
    pub(super) fn undo_redaction_mark(&mut self) {
        if let Some(redaction) = &mut self.redaction
            && redaction.saving.is_none()
        {
            redaction.undo();
        }
    }

    pub(super) fn open_redaction(&mut self) {
        if self.redaction.is_some() {
            return;
        }
        self.finish_editing(true);
        self.commit_ink();
        let Some(open) = &self.open else {
            return;
        };
        if open.readers.is_some() || open.doc.is_encrypted() {
            self.error = Some("Redaction requires an unprotected PDF copy.".into());
            return;
        }
        if let Err(error) = open.doc.signature_write_policy().check(true) {
            self.error = Some(error.to_string());
            return;
        }
        let source = open.doc.clone();
        if self.organize.grid {
            self.toggle_page_grid();
        }
        self.close_detect();
        self.selection = None;
        self.selecting = false;
        self.redaction = Some(Redaction {
            source,
            marks: BTreeMap::new(),
            history: Vec::new(),
            drag: None,
            tool: canvas::Tool::Area,
            saving: None,
            mode: ExportMode::Vector,
        });
    }

    pub(super) fn redaction_bar(&mut self, panel: &mut egui::Ui) {
        let Some(mut redaction) = self.redaction.take() else {
            return;
        };
        let Some(open) = &self.open else {
            return;
        };
        let ctx = panel.ctx().clone();
        if let Some(receiver) = &redaction.saving {
            match receiver.try_recv() {
                Ok(Ok(path)) => {
                    self.notice = Some(format!(
                        "Verified and saved redacted copy as {}.",
                        super::display_name(&path)
                    ));
                    return;
                }
                Ok(Err(error)) => {
                    self.error = Some(error);
                    redaction.saving = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some("The redaction worker stopped.".into());
                    redaction.saving = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if redaction.saving.is_none() && !Arc::ptr_eq(&redaction.source, &open.doc) {
            self.notice =
                Some("The document changed. Open redaction again to place new marks.".into());
            self.selection = None;
            return;
        }
        let mut close = false;
        let mut export = false;
        egui::Panel::top("secure-redaction").show(panel, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Redaction");
                if redaction.saving.is_some() {
                    ui.spinner();
                    ui.label("Verifying and saving?");
                    return;
                }
                for tool in canvas::Tool::ALL {
                    if ui
                        .selectable_value(&mut redaction.tool, tool, tool.label())
                        .on_hover_text(tool.help())
                        .changed()
                    {
                        redaction.drag = None;
                        self.selection = None;
                    }
                }
                ui.separator();
                if ui
                    .add_enabled(
                        !redaction.history.is_empty(),
                        egui::Button::new("Undo mark"),
                    )
                    .clicked()
                {
                    redaction.undo();
                }
                egui::ComboBox::from_id_salt("redaction-export")
                    .selected_text(redaction.mode.label())
                    .show_ui(ui, |ui| {
                        for mode in [ExportMode::Vector, ExportMode::Raster] {
                            ui.selectable_value(&mut redaction.mode, mode, mode.label());
                        }
                    });
                export = ui
                    .add_enabled(
                        !redaction.history.is_empty(),
                        egui::Button::new("Export redacted copy?"),
                    )
                    .clicked();
                close = ui.button("Exit redaction").clicked();
            });
        });
        if export {
            let stem = open.name.strip_suffix(".pdf").unwrap_or(&open.name);
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("PDF documents", &["pdf"])
                .set_file_name(format!("{stem} (redacted).pdf"))
                .save_file()
            {
                if open
                    .path
                    .as_ref()
                    .is_some_and(|source| crate::recent::same_file(source, &path))
                {
                    self.error = Some("Choose a separate filename for the redacted copy.".into());
                } else {
                    let source = redaction.source.clone();
                    let marks = redaction.marks.clone();
                    let mode = redaction.mode;
                    let (sender, receiver) = mpsc::channel();
                    let wake = ctx.clone();
                    let viewport = self.viewport;
                    let spawned = std::thread::Builder::new()
                        .name("save-redacted-copy".into())
                        .spawn(move || {
                            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                                || match mode {
                                    ExportMode::Vector => vector::vector_copy(&source, &marks),
                                    ExportMode::Raster => raster_copy(&source, &marks),
                                },
                            ))
                            .map_err(|_| {
                                "The renderer stopped while preparing the redacted copy.".to_owned()
                            })
                            .and_then(|result| result.map_err(|e| e.to_string()))
                            .and_then(|bytes| {
                                super::write_replacing(&path, &bytes).map_err(|e| e.to_string())
                            })
                            .map(|()| path);
                            let _ = sender.send(result);
                            wake.request_repaint_of(viewport);
                        });
                    match spawned {
                        Ok(_) => redaction.saving = Some(receiver),
                        Err(error) => {
                            self.error = Some(format!("Could not start redaction: {error}"))
                        }
                    }
                }
            }
        }
        if close {
            self.selection = None;
        } else {
            self.redaction = Some(redaction);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verification_rejects_unmasked_pixels_and_catalog_metadata() {
        let seed = empty_pdf().unwrap();
        let mut update = seed.update();
        let (mut expected, _) = image_page(&mut update, vec![255; 12], [2, 2], [2.0, 2.0], vec![]);
        set_page_tree(&mut update, vec![Object::Reference(expected.page)]);
        let reopen = |update: &Update<'_>| {
            Document::open(update.to_compact_bytes(&HashSet::new()).unwrap()).unwrap()
        };
        let copy = reopen(&update);
        assert!(verify_raster_copy(&copy, std::slice::from_ref(&expected)).is_ok());
        expected.masks.push([0, 0, 1, 1]);
        assert!(verify_raster_copy(&copy, std::slice::from_ref(&expected)).is_err());
        expected.masks.clear();
        let mut catalog = copy.catalog().unwrap();
        catalog.insert(
            "Lang",
            Object::String(atelier_syntax::PdfString::from_text("en")),
        );
        update.set(ObjRef::new(1, 0), Object::Dict(catalog));
        assert!(verify_raster_copy(&reopen(&update), &[expected]).is_err());
    }

    #[test]
    fn redaction_writes_only_the_painted_pixels_and_keeps_other_pages() {
        let original = Document::open(super::super::quality_tests::fixture(2)).unwrap();
        let marks = Marks::from([(
            0,
            vec![Rect::from_min_max(
                egui::pos2(0.08, 0.45),
                egui::pos2(0.83, 0.74),
            )],
        )]);
        let bytes = raster_copy(&original, &marks).unwrap();
        assert!(
            !bytes
                .windows(b"Atelier search fixture".len())
                .any(|window| window == b"Atelier search fixture")
        );
        if let Some(path) = std::env::var_os("ATELIER_QA_REDACTED_PDF") {
            std::fs::write(path, &bytes).unwrap();
        }
        let copy = Document::open(bytes).unwrap();
        assert!(!copy.catalog().unwrap().contains_key(b"AcroForm"));
        assert!(!copy.trailer().contains_key(b"Prev"));
        let pages = copy.pages().unwrap();
        assert_eq!(pages.len(), 2);
        for page in pages {
            assert!(
                atelier_render::extract_text(&copy, page)
                    .unwrap()
                    .glyphs
                    .is_empty()
            );
        }
        let rendered = atelier_render::render_page(
            &copy,
            &pages[0],
            &atelier_render::RenderOptions {
                scale: 1.0,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(rendered.pixmap.pixel(80, 110), [0, 0, 0, 255]);
        assert_eq!(rendered.pixmap.pixel(5, 5), [255; 4]);
    }
}
