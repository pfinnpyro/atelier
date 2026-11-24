use super::*;
use atelier_form::{Form, Markup, NewAnnot, Review, Value};
use atelier_raster::Point;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn viewer() -> Viewer {
    Viewer::blank(
        egui::Context::default(),
        Settings {
            check_updates: false,
            ocr: false,
            orient_on_open: false,
            ..Default::default()
        },
    )
}

pub(super) fn fixture(pages: usize) -> Vec<u8> {
    let content = "BT /Helv 12 Tf 20 75 Td (Atelier search fixture) Tj ET";
    let kids = (0..pages)
        .map(|n| format!("{} 0 R", n + 6))
        .collect::<Vec<_>>()
        .join(" ");
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /DA (/Helv 12 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >> >>".into(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Before) /Rect [10 150 190 175] /DA (/Helv 12 Tf 0 g) /F 4 /P 6 0 R >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    for n in 0..pages {
        let annots = if n == 0 { "/Annots [4 0 R]" } else { "" };
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /Helv 3 0 R >> >> /Contents 5 0 R {annots} >>"));
    }
    let mut data = b"%PDF-1.7\n".to_vec();
    let mut entries = String::new();
    for (i, object) in objects.iter().enumerate() {
        entries.push_str(&format!("{:010} 00000 n\r\n", data.len()));
        data.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = data.len();
    let size = objects.len() + 1;
    data.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f\r\n{entries}trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    data
}

fn wait_for(mut poll: impl FnMut() -> bool) {
    let start = Instant::now();
    while !poll() {
        assert!(start.elapsed() < Duration::from_secs(15));
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "set ATELIER_QA_OPEN_PDF to benchmark a local PDF"]
fn measure_time_to_first_page() {
    let path = PathBuf::from(std::env::var_os("ATELIER_QA_OPEN_PDF").unwrap());
    let ctx = egui::Context::default();
    let mut viewer = Viewer::initialize(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Settings {
            check_updates: false,
            ..Default::default()
        },
    );
    let start = Instant::now();
    viewer.open_path(&path);
    let mut installed = None;
    wait_for(|| {
        let mut frame = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1100.0, 800.0))),
                ..Default::default()
            },
            |ui| viewer.window_ui(ui, None),
        );
        frame.textures_delta.clear();
        if let Some(open) = viewer.open.as_ref() {
            installed.get_or_insert_with(|| start.elapsed());
            open.tiles.bytes() > 0
        } else {
            false
        }
    });
    eprintln!(
        "Document ready: {:?}. First page tile drawn: {:?}. Cross-reference repaired: {}.",
        installed.unwrap(),
        start.elapsed(),
        viewer.open.as_ref().unwrap().doc.was_repaired()
    );
    if std::env::var_os("ATELIER_QA_OCR").is_some() {
        let mut first = None;
        let mut first_start = None;
        let mut drawn = std::array::from_fn::<_, 3, _>(|_| HashMap::new());
        let mut sharp = std::array::from_fn::<_, 3, _>(|_| HashMap::new());
        let limit = start + Duration::from_secs(100);
        loop {
            let mut frame = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1100.0, 800.0))),
                    ..Default::default()
                },
                |ui| viewer.window_ui(ui, None),
            );
            frame.textures_delta.clear();
            let open = viewer.open.as_ref().unwrap();
            for (page, times) in drawn.iter_mut().enumerate().take(open.layout.page_count()) {
                if open.tiles.page_tiles(page).next().is_some() {
                    times
                        .entry(open.session.rotation(page))
                        .or_insert_with(|| start.elapsed());
                }
                if let Some(rect) = open.layout.page_rect(page) {
                    let scale =
                        super::scale_key(open.layout.zoom(), f64::from(ctx.pixels_per_point()));
                    let expected = atelier_view::tiles_at_scale(&open.layout, rect, scale);
                    if expected
                        .iter()
                        .filter(|(key, _)| key.page == page)
                        .all(|(key, _)| open.tiles.contains(key))
                    {
                        sharp[page]
                            .entry(open.session.rotation(page))
                            .or_insert_with(|| start.elapsed());
                    }
                }
            }
            if let Some(run) = &open.orienting {
                first_start.get_or_insert_with(|| start.elapsed());
                if run.done.load(std::sync::atomic::Ordering::Relaxed) >= 3 && first.is_none() {
                    first = Some(start.elapsed());
                    eprintln!(
                        "First three orientation results: {:?}; {:?}.",
                        first.unwrap(),
                        (0..3)
                            .map(|p| (
                                open.session.rotation(p),
                                open.ocr_text(p).map_or(0, |t| t.glyphs.len())
                            ))
                            .collect::<Vec<_>>()
                    );
                }
            } else if first_start.is_some() {
                eprintln!(
                    "Completed {} pages at {:?}.",
                    open.layout.page_count(),
                    start.elapsed()
                );
                let times = drawn
                    .iter()
                    .enumerate()
                    .map(|(page, times)| times.get(&open.session.rotation(page)).copied())
                    .collect::<Vec<_>>();
                eprintln!("First correct render for pages 1–3: {times:?}.");
                let times = sharp
                    .iter()
                    .enumerate()
                    .map(|(page, times)| times.get(&open.session.rotation(page)).copied())
                    .collect::<Vec<_>>();
                eprintln!("All sharp tiles for pages 1–3: {times:?}.");
                break;
            }
            assert!(Instant::now() < limit);
            std::thread::sleep(Duration::from_millis(8));
        }
    }
}

#[test]
fn saved_forms_comments_and_ink_reopen_and_render_independently() {
    let temp = super::security_provider::Scratch::new().unwrap();
    let path = temp.0.join("Review, résumé.pdf");
    let mut viewer = viewer();
    viewer.open_bytes(fixture(1), "Fixture.pdf".into(), None);
    write_replacing(&path, viewer.open.as_ref().unwrap().doc.data()).unwrap();
    let open = viewer.open.as_mut().unwrap();
    open.path = Some(path.clone());
    open.session
        .set(0, Value::Text("Recovered field value".into()));
    open.session.add_annot(
        0,
        NewAnnot {
            markup: Markup::Note {
                at: Point::new(150.0, 120.0),
                review: Review {
                    text: "Review comment".into(),
                    author: "Reviewer".into(),
                    subject: "QA".into(),
                    created: "D:20261124120000Z".into(),
                    modified: "D:20261124120000Z".into(),
                    state: None,
                },
            },
            color: [1.0, 0.5, 0.0],
        },
    );
    open.session.add_annot(
        0,
        NewAnnot {
            markup: Markup::Ink {
                strokes: vec![vec![Point::new(20.0, 35.0), Point::new(120.0, 35.0)]],
                width: 6.0,
            },
            color: [0.0, 0.0, 1.0],
        },
    );
    assert!(viewer.try_rebuild(&HashSet::from([0]), false));
    viewer.save(SaveTo::Overwrite);
    wait_for(|| {
        viewer.finish_saves();
        viewer.saving == 0
    });
    assert!(viewer.error.is_none(), "{:?}", viewer.error);
    assert!(!viewer.is_dirty());
    let reopened = Document::open(std::fs::read(&path).unwrap()).unwrap();
    let form = Form::load(&reopened).unwrap().unwrap();
    assert_eq!(form.fields[0].value.display(), "Recovered field value");
    let page = &reopened.pages().unwrap()[0];
    let annots = reopened.get_key(&page.dict, b"Annots").unwrap();
    let atelier_syntax::Object::Array(annots) = annots else {
        panic!("missing annotations")
    };
    assert_eq!(annots.len(), 3);
    let note = reopened.resolve_dict(&annots[1]).unwrap();
    assert_eq!(
        note.get(b"Contents")
            .unwrap()
            .as_string()
            .unwrap()
            .to_text(),
        "Review comment"
    );

    if reader_available("pdftotext") {
        let text = reader_output(Command::new("pdftotext").arg(&path).arg("-")).unwrap();
        assert!(
            text.status.success(),
            "{}",
            String::from_utf8_lossy(&text.stderr)
        );
        assert!(String::from_utf8_lossy(&text.stdout).contains("Recovered field value"));
    }
    let preview = temp.0.join("preview");
    let poppler = reader_available("pdftoppm");
    if poppler {
        let raster = reader_output(
            Command::new("pdftoppm")
                .args(["-r", "72", "-png", "-singlefile"])
                .arg(&path)
                .arg(&preview),
        )
        .unwrap();
        assert!(
            raster.status.success(),
            "{}",
            String::from_utf8_lossy(&raster.stderr)
        );
    } else if cfg!(windows) && std::env::var_os("ATELIER_QA_WINDOWS_PDF").is_some() {
        render_with_windows_pdf(&path, &preview.with_extension("png"));
    }
    if preview.with_extension("png").exists() {
        let pixels = image::open(preview.with_extension("png"))
            .unwrap()
            .to_rgb8();
        let [r, g, b] = pixels.get_pixel(60, 165).0;
        assert!(r < 40 && g < 40 && b > 220, "ink pixel was {r}, {g}, {b}");
        assert!(
            (56..130)
                .any(|x| (26..50).any(|y| { pixels.get_pixel(x, y).0.iter().all(|v| *v < 180) })),
            "rendered field still has the original short value"
        );
        if poppler {
            assert!(
                (150..170).any(|x| (80..100).any(|y| {
                    let [r, g, b] = pixels.get_pixel(x, y).0;
                    r > 150 && b < 100 && g < 220
                })),
                "rendered comment icon is missing"
            );
        }
    }
}

fn reader_available(tool: &str) -> bool {
    reader_output(Command::new(tool).arg("-v")).is_ok_and(|run| run.status.success())
}

fn reader_output(command: &mut Command) -> std::io::Result<Output> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait()?.is_none() {
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "PDF reader timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output()
}

fn render_with_windows_pdf(path: &Path, output: &Path) {
    let script = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$fileType = [Windows.Storage.StorageFile,Windows.Storage,ContentType=WindowsRuntime]
$docType = [Windows.Data.Pdf.PdfDocument,Windows.Data.Pdf,ContentType=WindowsRuntime]
$stream = [Windows.Storage.Streams.InMemoryRandomAccessStream,Windows.Storage.Streams,ContentType=WindowsRuntime]::new()
$extensions = [System.WindowsRuntimeSystemExtensions]
$genericTask = $extensions.GetMethods() | Where-Object {
    $_.Name -eq 'AsTask' -and $_.IsGenericMethod -and $_.GetParameters().Count -eq 1 -and
    $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'
}
function Await-Result($operation, $type) {
    $genericTask.MakeGenericMethod($type).Invoke($null, @($operation)).GetAwaiter().GetResult()
}
$file = Await-Result ($fileType::GetFileFromPathAsync($env:ATELIER_QA_PDF)) $fileType
$pdf = Await-Result ($docType::LoadFromFileAsync($file)) $docType
if ($pdf.PageCount -ne 1) { throw 'Expected one page' }
$page = $pdf.GetPage(0)
$options = [Windows.Data.Pdf.PdfPageRenderOptions,Windows.Data.Pdf,ContentType=WindowsRuntime]::new()
$options.DestinationWidth = 200
$options.DestinationHeight = 200
$options.BitmapEncoderId = [Windows.Graphics.Imaging.BitmapEncoder,Windows.Graphics.Imaging,ContentType=WindowsRuntime]::PngEncoderId
$actionTask = $extensions.GetMethods() | Where-Object {
    $_.Name -eq 'AsTask' -and !$_.IsGenericMethod -and $_.GetParameters().Count -eq 1
}
$actionTask.Invoke($null, @($page.RenderToStreamAsync($stream, $options))).GetAwaiter().GetResult()
$stream.Seek(0)
$inputStream = [System.IO.WindowsRuntimeStreamExtensions]::AsStreamForRead($stream)
$output = [IO.File]::Create($env:ATELIER_QA_PREVIEW)
try { $inputStream.CopyTo($output) } finally { $output.Dispose(); $inputStream.Dispose(); $page.Dispose() }
"#;
    let render = reader_output(
        Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("ATELIER_QA_PDF", path)
            .env("ATELIER_QA_PREVIEW", output),
    )
    .unwrap();
    assert!(
        render.status.success(),
        "{}",
        String::from_utf8_lossy(&render.stderr)
    );
}

#[test]
fn reopening_restores_page_and_zoom_after_switching_tabs() {
    let temp = super::security_provider::Scratch::new().unwrap();
    let path = temp.0.join("Three pages.pdf");
    std::fs::write(&path, fixture(3)).unwrap();
    let mut viewer = viewer();
    viewer.open_bytes(fixture(3), "Three pages.pdf".into(), None);
    viewer.open.as_mut().unwrap().path = Some(path.clone());
    viewer.zoom_mode = ZoomMode::Custom;
    viewer.set_zoom(3.0, Vec2::ZERO);
    viewer.go_to_page(1);
    viewer.offset = viewer.scroll_to.take().unwrap();
    viewer.open_bytes(fixture(1), "Other.pdf".into(), None);
    viewer.request_close_tab(0);
    #[cfg(target_os = "macos")]
    viewer.run_command(crate::commands::Command::ReopenClosedTab);
    #[cfg(not(target_os = "macos"))]
    {
        let ctx = viewer.ctx.clone();
        let modifiers = egui::Modifiers::COMMAND | egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: Key::T,
                    physical_key: Some(Key::T),
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            },
            |_| viewer.tab_keys(&ctx),
        );
        output.textures_delta.clear();
    }
    wait_for(|| {
        viewer.finish_loads();
        viewer.loading == 0
    });
    assert_eq!(
        viewer.open.as_ref().unwrap().path.as_deref(),
        Some(path.as_path())
    );
    assert_eq!(viewer.zoom_mode, ZoomMode::Custom);
    assert!((viewer.zoom() - 3.0).abs() < f64::EPSILON);
    let restored = viewer.scroll_to.unwrap();
    assert_eq!(
        viewer
            .open
            .as_ref()
            .unwrap()
            .layout
            .page_at_y(f64::from(restored.y + viewer.view_size.y / 2.0)),
        1
    );
}

#[test]
fn save_as_changes_the_file_path_and_moving_the_tab_preserves_it() {
    let temp = super::security_provider::Scratch::new().unwrap();
    let original = temp.0.join("Original.pdf");
    let target = temp.0.join("Saved, résumé.pdf");
    let mut first = viewer();
    let data = fixture(1);
    std::fs::write(&original, &data).unwrap();
    first.open_bytes(data.clone(), "Original.pdf".into(), None);
    first.open.as_mut().unwrap().path = Some(original.clone());
    first
        .open
        .as_mut()
        .unwrap()
        .session
        .set(0, Value::Text("Saved as".into()));
    assert!(first.try_rebuild(&HashSet::from([0]), false));
    first.write_save(target.clone(), SaveTo::As, None);
    wait_for(|| {
        first.finish_saves();
        first.saving == 0
    });
    assert!(first.error.is_none(), "{:?}", first.error);
    assert_eq!(std::fs::read(original).unwrap(), data);
    let parked = first.take_tab(0).unwrap();
    let mut second = viewer();
    second.adopt(vec![parked], 0);
    assert_eq!(
        second.open.as_ref().unwrap().path.as_deref(),
        Some(target.as_path())
    );
    #[cfg(windows)]
    assert!(second.command_enabled(crate::commands::Command::ShowInExplorer));
    let saved = Document::open(std::fs::read(target).unwrap()).unwrap();
    assert_eq!(
        Form::load(&saved).unwrap().unwrap().fields[0]
            .value
            .display(),
        "Saved as"
    );
}

#[test]
fn rapid_tab_switches_cancel_large_search_and_keep_final_results() {
    let mut viewer = viewer();
    let data = fixture(2500);
    let start = Instant::now();
    viewer.open_bytes(data, "Large.pdf".into(), None);
    let open = viewer.open.as_ref().unwrap();
    let page = &open.doc.pages().unwrap()[0];
    atelier_render::render_page(&open.doc, page, &atelier_render::RenderOptions::default())
        .unwrap();
    eprintln!(
        "2500-page fixture, open through first CPU raster: {:?}",
        start.elapsed()
    );
    viewer.open_bytes(fixture(1), "Small.pdf".into(), None);
    for _ in 0..12 {
        viewer.switch_to(0);
        viewer.start_search("Atelier".into());
        viewer.switch_to(1);
        viewer.start_search("missing".into());
    }
    viewer.start_search("search fixture".into());
    wait_for(|| {
        viewer.receive();
        viewer.search.done
    });
    assert_eq!(viewer.search.query, "search fixture");
    assert_eq!(viewer.search.hits.len(), 1);
    assert_eq!(viewer.search.hits[0].page, 0);
    assert_eq!(viewer.search.pages_done, 1);
}

#[test]
fn subsequent_changes_group_forms_and_annotations_and_show_unknown_objects() {
    let base = Document::open(fixture(1)).unwrap();
    let prepared = base.prepare_signature("Fixture signer", 1024).unwrap();
    let signed = Arc::new(Document::open(prepared.finish(&[0x30, 1, 0]).unwrap()).unwrap());
    let mut edits = Session::new(signed.clone());
    edits.set(0, Value::Text("Later update".into()));
    edits.add_annot(
        0,
        NewAnnot {
            markup: Markup::Ink {
                strokes: vec![vec![Point::new(3.0, 3.0), Point::new(20.0, 20.0)]],
                width: 2.0,
            },
            color: [1.0, 0.0, 0.0],
        },
    );
    let changed = edits.build().unwrap().doc;
    let report = changed.changes_since_signed_revision(signed.data().len());
    let revision = &report.as_ref().as_ref().unwrap().revisions[0];
    let summary = revision.classification();
    assert_eq!(summary.form_updates, 1);
    assert_eq!(summary.annotation_updates, 1);
    assert!(summary.unclassified_objects > 0);
}

#[test]
fn toolbar_and_tab_strip_leave_document_space_in_narrow_scaled_windows() {
    for (width, scale) in [(360.0, 1.0), (320.0, 1.5), (280.0, 2.0)] {
        let ctx = egui::Context::default();
        let creation = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut viewer = Viewer::initialize(&creation, Settings::default());
        viewer.settings.ocr = false;
        viewer.settings.orient_on_open = false;
        viewer
            .settings
            .toolbar_display
            .fill(crate::settings::ButtonDisplay::IconAndText);
        for title in ["First document.pdf", "Second document with a long name.pdf"] {
            viewer.open_bytes(fixture(1), title.into(), None);
        }
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(width, 460.0));
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .native_pixels_per_point = Some(scale);
        let mut output = ctx.run_ui(input, |ui| {
            egui::Panel::top("toolbar").show(ui, |ui| viewer.toolbar(ui));
            viewer.tab_bar(ui);
            assert!(ui.available_height() >= 200.0);
            assert!((ui.ctx().pixels_per_point() - scale).abs() < f32::EPSILON);
        });
        output.textures_delta.clear();
        assert!(!output.shapes.is_empty());
    }
}

#[cfg(not(target_os = "macos"))]
#[test]
fn menu_layouts_keep_the_expected_controls_in_each_row() {
    use crate::settings::MenuDisplay;

    fn draw(viewer: &mut Viewer, events: Vec<Event>) -> HashMap<String, Pos2> {
        let ctx = viewer.ctx.clone();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(850.0, 600.0))),
                events,
                ..Default::default()
            },
            |ui| viewer.toolbar(ui),
        );
        output.textures_delta.clear();
        output
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .into_iter()
            .filter_map(|(_, node)| {
                let label = node.label()?.to_owned();
                let rect = node.bounds()?;
                Some((
                    label,
                    egui::pos2(
                        ((rect.x0 + rect.x1) / 2.0) as f32,
                        ((rect.y0 + rect.y1) / 2.0) as f32,
                    ),
                ))
            })
            .collect()
    }

    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut viewer = Viewer::initialize(
        &eframe::CreationContext::_new_kittest(ctx),
        Settings {
            check_updates: false,
            ..Default::default()
        },
    );
    let initial = draw(&mut viewer, Vec::new());
    assert!(initial.contains_key("Menu") && initial.contains_key("Open"));
    assert!(!initial.contains_key("File"));
    viewer.settings.menu_display = MenuDisplay::Expandable;
    let closed = draw(&mut viewer, Vec::new());
    let click = |pos| {
        vec![
            Event::PointerMoved(pos),
            Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
            Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
        ]
    };
    draw(&mut viewer, click(closed["Expand menu"]));
    let expanded = draw(&mut viewer, Vec::new());
    assert!(expanded.contains_key("File"));
    assert!(!expanded.contains_key("Open"));
    if crate::chrome::custom_windows() {
        assert!((expanded["Close window"].y - expanded["File"].y).abs() < 2.0);
    }
    draw(&mut viewer, click(expanded["Collapse menu"]));
    let collapsed = draw(&mut viewer, Vec::new());
    assert!(collapsed.contains_key("Open") && !collapsed.contains_key("File"));

    viewer.settings.menu_display = MenuDisplay::TopBar;
    let top = draw(&mut viewer, Vec::new());
    assert!(top["File"].y < top["Open"].y);
    if crate::chrome::custom_windows() {
        assert!((top["Close window"].y - top["File"].y).abs() < 2.0);
    }
    draw(&mut viewer, click(top["File"]));
    let file_menu = draw(&mut viewer, Vec::new());
    assert!(file_menu.contains_key("Open Recent"));
}

#[cfg(windows)]
#[test]
fn explorer_paths_handle_recent_file_prefixes_without_losing_unicode() {
    use super::commands::explorer_path;

    for (path, expected) in [
        (
            r"\\?\C:\My Docs\Review, résumé.pdf",
            r"C:\My Docs\Review, résumé.pdf",
        ),
        (
            r"\\?\UNC\server\Share\Review, résumé.pdf",
            r"\\server\Share\Review, résumé.pdf",
        ),
    ] {
        assert_eq!(
            explorer_path(Path::new(path)),
            std::ffi::OsStr::new(expected)
        );
    }
}
