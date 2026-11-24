//! Background threads that render tiles and read page text, so the window
//! never waits on the document.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use atelier_ocr::{Engine, preview_orientation, read_page_with_hint};
use atelier_render::{
    PageText, Probed, RenderOptions, SpotColor, extract_text, probe_color, render_page,
    render_region, spot_colors,
};
use atelier_syntax::{Document, Page};
use atelier_view::{
    Basis, OutlineItem, TileKey, bookmarks, find_in_page, headings, upright_page, upright_rotation,
};

/// Text read with OCR from pages that have none of their own, by page.
/// Kept per open file; it stays right when pages turn, since it is in the
/// page's own coordinates.
pub type OcrTexts = Arc<Mutex<HashMap<usize, Arc<PageText>>>>;

/// Pages with fewer letters than this count as having no text, so OCR
/// reads them.
const MIN_LETTERS: usize = 12;

fn little_text(text: &PageText) -> bool {
    text.glyphs
        .iter()
        .filter(|g| !g.text.trim().is_empty())
        .take(MIN_LETTERS)
        .count()
        < MIN_LETTERS
}

/// A page's text: its own, or what OCR read when it has next to none.
pub fn page_text(doc: &Document, index: usize, page: &Page, ocr: &OcrTexts) -> Arc<PageText> {
    let text = extract_text(doc, page).unwrap_or_default();
    if little_text(&text)
        && let Some(read) = ocr.lock().unwrap_or_else(|e| e.into_inner()).get(&index)
    {
        return read.clone();
    }
    Arc::new(text)
}

/// What an auto-orient run needs to read scans with OCR.
#[derive(Clone)]
pub struct OcrRun {
    /// Where the models are.
    pub dir: PathBuf,
    /// Where the text read goes.
    pub texts: OcrTexts,
    /// Pages read so far.
    pub read: Arc<AtomicUsize>,
    pub workers: usize,
}

/// Work for the pool. Each job carries the document it is for, so a job
/// queued before another file was opened still finishes cleanly.
#[derive(Clone)]
pub enum Job {
    Tile {
        doc: Arc<Document>,
        generation: u64,
        key: TileKey,
        page_px: (usize, usize),
    },
    Text {
        doc: Arc<Document>,
        generation: u64,
        page: usize,
        ocr: OcrTexts,
    },
    /// A page drawn small for the sidebar, `scale` device pixels per point.
    Thumb {
        doc: Arc<Document>,
        generation: u64,
        page: usize,
        scale: f64,
    },
}

/// What a job makes, to tell two jobs for the same thing apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum JobKey {
    Tile(u64, TileKey),
    Text(u64, usize),
    Thumb(u64, usize),
}

impl Job {
    fn generation(&self) -> u64 {
        match self {
            Job::Tile { generation, .. }
            | Job::Text { generation, .. }
            | Job::Thumb { generation, .. } => *generation,
        }
    }

    fn key(&self) -> JobKey {
        match self {
            Job::Tile {
                generation, key, ..
            } => JobKey::Tile(*generation, *key),
            Job::Text {
                generation, page, ..
            } => JobKey::Text(*generation, *page),
            Job::Thumb {
                generation, page, ..
            } => JobKey::Thumb(*generation, *page),
        }
    }
}

pub enum Done {
    Tile {
        generation: u64,
        key: TileKey,
        /// Premultiplied RGBA rows.
        size: [usize; 2],
        rgba: Vec<u8>,
        warnings: Vec<String>,
    },
    /// A tile that failed; it is not asked for again until the zoom changes.
    TileFailed {
        generation: u64,
        key: TileKey,
        error: String,
    },
    Text {
        generation: u64,
        page: usize,
        text: Arc<PageText>,
    },
    SearchHits {
        generation: u64,
        search: u64,
        page: usize,
        hits: Vec<std::ops::Range<usize>>,
        text: Option<Arc<PageText>>,
    },
    SearchEnded {
        generation: u64,
        search: u64,
    },
    Thumb {
        generation: u64,
        page: usize,
        size: [usize; 2],
        rgba: Vec<u8>,
    },
    /// The outline for the sidebar: bookmarks, or headings found in the
    /// text (`from_headings`). `opened` is the file it is for.
    Outline {
        opened: u64,
        items: Vec<OutlineItem>,
        from_headings: bool,
    },
    /// Auto-orient has judged one page: its upright /Rotate and what
    /// that was judged from, None where the page gave nothing to go by.
    /// Sent as each page is done, so pages turn while the rest are read.
    OrientedPage {
        generation: u64,
        page: usize,
        rotation: Option<(u16, Basis)>,
    },
    /// Auto-orient has finished: the same for every page.
    Oriented {
        generation: u64,
        rotations: Vec<Option<(u16, Basis)>>,
    },
    /// OCR read a page of the file opened as `opened`.
    OcrPage {
        opened: u64,
        page: usize,
    },
    OcrStarted {
        opened: u64,
        page: usize,
    },
    OrientationPreviews {
        opened: u64,
        rotations: [Option<u16>; 3],
    },
    /// The OCR models could not be loaded.
    OcrFailed {
        opened: u64,
        error: String,
    },
    /// A picked colour.
    Probed {
        generation: u64,
        page: usize,
        result: Result<Probed, String>,
    },
    /// The tables and blanks found on one page of the file opened as
    /// `opened`, by detection run `run`.
    Detected {
        opened: u64,
        run: u64,
        page: usize,
        found: atelier_form::Detected,
    },
    /// A detection run looked at every page.
    DetectEnded {
        opened: u64,
        run: u64,
    },
    /// The spot colours of the file opened as `opened`.
    Spots {
        opened: u64,
        spots: Vec<SpotColor>,
    },
}

struct Queue {
    jobs: VecDeque<Job>,
    /// Jobs a thread is working on now.
    running: HashSet<JobKey>,
    quit: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
}

pub struct Pool {
    shared: Arc<Shared>,
    done_tx: Sender<Done>,
    pub done: Receiver<Done>,
    /// The search that should keep running; any other stops.
    search: Arc<AtomicU64>,
    /// The field detection that should keep running; any other stops.
    detect: Arc<AtomicU64>,
    threads: Vec<thread::JoinHandle<()>>,
    /// The OCR engine, loaded the first time it is needed.
    engine: Arc<Mutex<Option<Arc<Engine>>>>,
}

impl Pool {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                jobs: VecDeque::new(),
                running: HashSet::new(),
                quit: false,
            }),
            ready: Condvar::new(),
        });
        let (done_tx, done) = channel();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let n = thread::available_parallelism()
            .map(|n| n.get().saturating_sub(1))
            .unwrap_or(2)
            .clamp(1, 8);
        let threads = (0..n)
            .map(|i| {
                let shared = shared.clone();
                let tx = done_tx.clone();
                let wake = wake.clone();
                thread::Builder::new()
                    .name(format!("render-{i}"))
                    .spawn(move || run(&shared, &tx, &*wake))
                    .expect("spawn render thread")
            })
            .collect();
        Self {
            shared,
            done_tx,
            done,
            search: Arc::new(AtomicU64::new(0)),
            detect: Arc::new(AtomicU64::new(0)),
            threads,
            engine: Arc::new(Mutex::new(None)),
        }
    }

    /// Replaces everything still waiting with `jobs`, first job first.
    /// Jobs already running finish, and are not queued again.
    pub fn set_jobs(&self, jobs: Vec<Job>) {
        let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        let jobs: VecDeque<Job> = jobs
            .into_iter()
            .filter(|j| !q.running.contains(&j.key()))
            .collect();
        q.jobs = jobs;
        drop(q);
        self.shared.ready.notify_all();
    }

    /// Starts searching every page for `query` on its own thread, stopping
    /// any earlier search. Hits arrive page by page.
    pub fn search(
        &self,
        doc: Arc<Document>,
        generation: u64,
        query: String,
        ocr: OcrTexts,
        wake: impl Fn() + Send + 'static,
    ) -> u64 {
        let id = self.search.fetch_add(1, Ordering::SeqCst) + 1;
        let current = self.search.clone();
        let tx = self.done_tx.clone();
        let spawned = thread::Builder::new().name("search".into()).spawn(move || {
            let Ok(pages) = doc.pages() else {
                let _ = tx.send(Done::SearchEnded {
                    generation,
                    search: id,
                });
                wake();
                return;
            };
            // Pages are read on several threads and handed on in page
            // order, so hits still arrive first page first.
            let threads = thread::available_parallelism()
                .map_or(2, |n| n.get().saturating_sub(1))
                .clamp(1, 6)
                .min(pages.len().max(1));
            let next = AtomicUsize::new(0);
            type Found = (usize, Vec<std::ops::Range<usize>>, Option<Arc<PageText>>);
            let (found_tx, found_rx) = channel::<Found>();
            thread::scope(|s| {
                for _ in 0..threads {
                    let found_tx = found_tx.clone();
                    let (next, current, doc, ocr, query) = (&next, &current, &doc, &ocr, &query);
                    s.spawn(move || {
                        while current.load(Ordering::SeqCst) == id {
                            let i = next.fetch_add(1, Ordering::SeqCst);
                            let Some(page) = pages.get(i) else { break };
                            // A page that panics the reader has no hits.
                            let read = std::panic::AssertUnwindSafe(|| {
                                let text = page_text(doc, i, page, ocr);
                                let hits: Vec<_> = find_in_page(&text, i, query)
                                    .into_iter()
                                    .map(|m| m.glyphs)
                                    .collect();
                                (hits, text)
                            });
                            let (hits, text) = std::panic::catch_unwind(read).unwrap_or_default();
                            let text = (!hits.is_empty()).then_some(text);
                            if found_tx.send((i, hits, text)).is_err() {
                                break;
                            }
                        }
                    });
                }
                drop(found_tx);
                let mut waiting = std::collections::BTreeMap::new();
                let mut want = 0;
                for (i, hits, text) in found_rx {
                    if current.load(Ordering::SeqCst) != id {
                        break;
                    }
                    waiting.insert(i, (hits, text));
                    while let Some((hits, text)) = waiting.remove(&want) {
                        let _ = tx.send(Done::SearchHits {
                            generation,
                            search: id,
                            page: want,
                            hits,
                            text,
                        });
                        want += 1;
                    }
                    wake();
                }
            });
            if current.load(Ordering::SeqCst) != id {
                return;
            }
            let _ = tx.send(Done::SearchEnded {
                generation,
                search: id,
            });
            wake();
        });
        if spawned.is_err() {
            let _ = self.done_tx.send(Done::SearchEnded {
                generation,
                search: id,
            });
        }
        id
    }

    /// Prioritizes the first three pages and publishes each result as it arrives.
    pub fn orient(
        &self,
        doc: Arc<Document>,
        generation: u64,
        done: Arc<AtomicUsize>,
        cancelled: Arc<AtomicBool>,
        ocr: Option<OcrRun>,
        wake: impl Fn() + Send + Sync + 'static,
    ) {
        let tx = self.done_tx.clone();
        let engine = self.engine.clone();
        let spawn = thread::Builder::new().name("orient".into()).spawn(move || {
            let pages = doc.pages().unwrap_or_default();
            let first = pages.len().min(3);
            let early = if let Some(run) = &ocr {
                let guessed = Mutex::new([None; 3]);
                run_page_workers(0..first, run.workers, &cancelled, |index| {
                    let page = &pages[index];
                    let hint = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let text = extract_text(&doc, page).unwrap_or_default();
                        if little_text(&text) {
                            preview_orientation(&doc, page).ok().flatten()
                        } else {
                            upright_rotation(&text)
                        }
                    }))
                    .ok()
                    .flatten();
                    if let Some(rotation) = hint
                        && !cancelled.load(Ordering::Relaxed)
                    {
                        guessed.lock().unwrap_or_else(|e| e.into_inner())[index] = Some(rotation);
                    }
                });
                guessed.into_inner().unwrap_or_else(|e| e.into_inner())
            } else {
                [None; 3]
            };
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            if early.iter().any(Option::is_some) {
                let _ = tx.send(Done::OrientationPreviews {
                    opened: generation,
                    rotations: early,
                });
                wake();
            }
            let ocr = ocr.and_then(|run| match load_engine(&engine, &run) {
                Ok(e) => Some((e, run)),
                Err(error) => {
                    let _ = tx.send(Done::OcrFailed {
                        opened: generation,
                        error: format!("OCR could not start: {error}"),
                    });
                    wake();
                    None
                }
            });
            let rotations = Mutex::new(vec![None; pages.len()]);
            let previous = AtomicUsize::new(early[0].map_or(360, usize::from));
            let read_page = |index: usize| {
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                let page = &pages[index];
                let ocr = ocr.as_ref().map(|(engine, run)| (engine.as_ref(), run));
                let hint = previous.load(Ordering::Relaxed);
                let early_hint = early.get(index).copied().flatten();
                let hint = early_hint.or_else(|| (hint < 360).then_some(hint as u16));
                let mut read = false;
                let rotation = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    orient_page(
                        &doc,
                        index,
                        page,
                        ocr,
                        hint,
                        &|| {
                            let _ = tx.send(Done::OcrStarted {
                                opened: generation,
                                page: index,
                            });
                            wake();
                        },
                        &mut |result| {
                            if cancelled.load(Ordering::Relaxed) {
                                return;
                            }
                            match result {
                                Ok(text) => {
                                    if let Some(text) = text
                                        && let Some((_, run)) = ocr
                                    {
                                        run.texts
                                            .lock()
                                            .unwrap_or_else(|e| e.into_inner())
                                            .insert(index, Arc::new(text));
                                        run.read.fetch_add(1, Ordering::SeqCst);
                                    }
                                    read = true;
                                }
                                Err(error) => {
                                    let _ = tx.send(Done::OcrFailed {
                                        opened: generation,
                                        error: format!(
                                            "OCR could not read page {}: {error}",
                                            index + 1
                                        ),
                                    });
                                }
                            }
                        },
                    )
                }))
                .ok()
                .flatten();
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                if read
                    && tx
                        .send(Done::OcrPage {
                            opened: generation,
                            page: index,
                        })
                        .is_err()
                {
                    return;
                }
                if index == 0
                    && let Some((angle, Basis::Ocr | Basis::Text)) = rotation
                {
                    previous.store(usize::from(angle), Ordering::Relaxed);
                }
                rotations.lock().unwrap_or_else(|e| e.into_inner())[index] = rotation;
                if tx
                    .send(Done::OrientedPage {
                        generation,
                        page: index,
                        rotation,
                    })
                    .is_err()
                {
                    return;
                }
                done.fetch_add(1, Ordering::SeqCst);
                wake();
            };
            let workers = ocr.as_ref().map_or(1, |(_, run)| run.workers);
            for range in [0..first, first..pages.len()] {
                run_page_workers(range, workers, &cancelled, read_page);
            }
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            let rotations = rotations.into_inner().unwrap_or_else(|e| e.into_inner());
            let _ = tx.send(Done::Oriented {
                generation,
                rotations,
            });
            wake();
        });
        if spawn.is_err() {
            let _ = self.done_tx.send(Done::Oriented {
                generation,
                rotations: Vec::new(),
            });
        }
    }

    /// Reads the outline on its own thread: the bookmarks, or when there
    /// are none, headings found in the text of the first pages.
    pub fn outline(
        &self,
        doc: Arc<Document>,
        opened: u64,
        ocr: OcrTexts,
        wake: impl Fn() + Send + 'static,
    ) {
        /// Pages read for headings at most.
        const MAX_PAGES: usize = 1500;
        let tx = self.done_tx.clone();
        let _ = thread::Builder::new()
            .name("outline".into())
            .spawn(move || {
                let run = || {
                    let items = bookmarks(&doc);
                    if !items.is_empty() {
                        return (items, false);
                    }
                    let texts: Vec<_> = doc
                        .pages()
                        .map(|p| {
                            p.iter()
                                .take(MAX_PAGES)
                                .enumerate()
                                .map(|(i, p)| (*page_text(&doc, i, p, &ocr)).clone())
                                .collect()
                        })
                        .unwrap_or_default();
                    (headings(&texts), true)
                };
                let (items, from_headings) =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).unwrap_or_default();
                let _ = tx.send(Done::Outline {
                    opened,
                    items,
                    from_headings,
                });
                wake();
            });
    }

    /// Works out the colour at device pixel `xy` of `page` drawn at
    /// `scale`, on a thread of its own.
    pub fn probe(
        &self,
        doc: Arc<Document>,
        generation: u64,
        page: usize,
        scale: f64,
        xy: (usize, usize),
        wake: impl Fn() + Send + 'static,
    ) {
        let tx = self.done_tx.clone();
        let _ = thread::Builder::new().name("probe".into()).spawn(move || {
            let run = || {
                let p = doc
                    .pages()
                    .map_err(|e| e.to_string())?
                    .get(page)
                    .ok_or("no such page")?;
                let options = RenderOptions {
                    scale,
                    ..RenderOptions::default()
                };
                probe_color(&doc, p, &options, xy).map_err(|e| e.to_string())
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run))
                .unwrap_or_else(|_| Err("The page could not be drawn.".into()));
            let _ = tx.send(Done::Probed {
                generation,
                page,
                result,
            });
            wake();
        });
    }

    /// Lists the file's spot colours on a thread of its own.
    pub fn spots(&self, doc: Arc<Document>, opened: u64, wake: impl Fn() + Send + 'static) {
        let tx = self.done_tx.clone();
        let _ = thread::Builder::new().name("spots".into()).spawn(move || {
            let spots =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| spot_colors(&doc)))
                    .unwrap_or_default();
            let _ = tx.send(Done::Spots { opened, spots });
            wake();
        });
    }

    /// Stops the running search, if any.
    pub fn stop_search(&self) {
        self.search.fetch_add(1, Ordering::SeqCst);
    }

    /// Looks for tables and the blanks of a form on every page, on a
    /// thread of its own, stopping any earlier run. What is found arrives
    /// page by page. `taken` holds, for each page, the boxes of the
    /// fields it already has. Returns the run's number.
    pub fn detect(
        &self,
        doc: Arc<Document>,
        opened: u64,
        ocr: OcrTexts,
        taken: Vec<Vec<atelier_syntax::Rect>>,
        wake: impl Fn() + Send + 'static,
    ) -> u64 {
        let run = self.detect.fetch_add(1, Ordering::SeqCst) + 1;
        let current = self.detect.clone();
        let tx = self.done_tx.clone();
        let ended = self.done_tx.clone();
        let spawned = thread::Builder::new().name("detect".into()).spawn(move || {
            for (i, page) in doc.pages().unwrap_or_default().iter().enumerate() {
                if current.load(Ordering::SeqCst) != run {
                    return;
                }
                let look = || {
                    let text = page_text(&doc, i, page, &ocr);
                    let taken = taken.get(i).map_or(&[][..], Vec::as_slice);
                    atelier_form::detect_page(&doc, page, &text, taken)
                };
                // A page that cannot be read gives nothing; the rest go on.
                let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(look))
                    .unwrap_or_default();
                let _ = tx.send(Done::Detected {
                    opened,
                    run,
                    page: i,
                    found,
                });
                wake();
            }
            let _ = tx.send(Done::DetectEnded { opened, run });
            wake();
        });
        if spawned.is_err() {
            let _ = ended.send(Done::DetectEnded { opened, run });
        }
        run
    }

    /// Stops the running field detection, if any.
    pub fn stop_detect(&self) {
        self.detect.fetch_add(1, Ordering::SeqCst);
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.stop_search();
        if let Ok(mut q) = self.shared.queue.lock() {
            q.quit = true;
            q.jobs.clear();
        }
        self.shared.ready.notify_all();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

fn run(shared: &Shared, tx: &Sender<Done>, wake: &(dyn Fn() + Send + Sync)) {
    loop {
        let job = {
            let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if q.quit {
                    return;
                }
                if let Some(job) = q.jobs.pop_front() {
                    q.running.insert(job.key());
                    break job;
                }
                q = shared.ready.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        let generation = job.generation();
        let key = job.key();
        // A page that panics the renderer must not take the window down.
        let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(job.clone())))
            .unwrap_or_else(|_| match job {
                Job::Tile { key, .. } => Done::TileFailed {
                    generation,
                    key,
                    error: "the renderer crashed on this page".into(),
                },
                Job::Text { page, .. } => Done::Text {
                    generation,
                    page,
                    text: Arc::default(),
                },
                Job::Thumb { page, .. } => Done::Thumb {
                    generation,
                    page,
                    size: [1, 1],
                    rgba: vec![255; 4],
                },
            });
        let sent = tx.send(done).is_ok();
        shared
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .running
            .remove(&key);
        if !sent {
            return;
        }
        wake();
    }
}

fn work(job: Job) -> Done {
    match job {
        Job::Tile {
            doc,
            generation,
            key,
            page_px,
        } => {
            let result = doc.pages().and_then(|pages| {
                let page = pages
                    .get(key.page)
                    .ok_or_else(|| atelier_syntax::Error::Structure("no such page".into()))?;
                let options = RenderOptions {
                    scale: key.scale(),
                    ..RenderOptions::default()
                };
                render_region(&doc, page, &options, key.region(page_px))
            });
            match result {
                Ok(r) => Done::Tile {
                    generation,
                    key,
                    size: [r.pixmap.width(), r.pixmap.height()],
                    rgba: r.pixmap.data().to_vec(),
                    warnings: r.warnings,
                },
                Err(e) => Done::TileFailed {
                    generation,
                    key,
                    error: e.to_string(),
                },
            }
        }
        Job::Thumb {
            doc,
            generation,
            page,
            scale,
        } => {
            let options = RenderOptions {
                scale,
                ..RenderOptions::default()
            };
            let r = doc
                .pages()
                .ok()
                .and_then(|p| p.get(page))
                .and_then(|p| render_page(&doc, p, &options).ok());
            match r {
                Some(r) => Done::Thumb {
                    generation,
                    page,
                    size: [r.pixmap.width(), r.pixmap.height()],
                    rgba: r.pixmap.data().to_vec(),
                },
                None => Done::Thumb {
                    generation,
                    page,
                    size: [1, 1],
                    rgba: vec![255; 4],
                },
            }
        }
        Job::Text {
            doc,
            generation,
            page,
            ocr,
        } => {
            let text = doc
                .pages()
                .ok()
                .and_then(|p| p.get(page).map(|p| page_text(&doc, page, p, &ocr)))
                .unwrap_or_default();
            Done::Text {
                generation,
                page,
                text,
            }
        }
    }
}

fn run_page_workers(
    range: std::ops::Range<usize>,
    workers: usize,
    cancelled: &AtomicBool,
    process: impl Fn(usize) + Sync,
) {
    let next = AtomicUsize::new(range.start);
    thread::scope(|scope| {
        for _ in 0..workers.min(range.len()) {
            scope.spawn(|| {
                while !cancelled.load(Ordering::Relaxed) {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= range.end {
                        break;
                    }
                    process(index);
                }
            });
        }
    });
}

/// The OCR engine, loaded once and then kept.
fn load_engine(cache: &Mutex<Option<Arc<Engine>>>, run: &OcrRun) -> Result<Arc<Engine>, String> {
    let mut slot = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(e) = &*slot {
        return Ok(e.clone());
    }
    let e = Arc::new(Engine::load(&run.dir)?);
    *slot = Some(e.clone());
    Ok(e)
}

/// Which way up a page goes: by its own text, else by the words OCR reads
/// (keeping them as its text), else by the look of its image. `read` is
/// called when OCR has read the page.
fn orient_page(
    doc: &Document,
    index: usize,
    page: &Page,
    ocr: Option<(&Engine, &OcrRun)>,
    hint: Option<u16>,
    started: &dyn Fn(),
    read: &mut dyn FnMut(Result<Option<PageText>, String>),
) -> Option<(u16, Basis)> {
    let text = extract_text(doc, page).unwrap_or_default();
    if let Some(r) = upright_rotation(&text) {
        return Some((r, Basis::Text));
    }
    if let Some((engine, run)) = ocr
        && little_text(&text)
    {
        let known = run
            .texts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&index)
            .cloned();
        if let Some(t) = known {
            read(Ok(None));
            return upright_rotation(&t)
                .map(|r| (r, Basis::Ocr))
                .or_else(|| upright_page(doc, page));
        }
        started();
        match read_page_with_hint(engine, doc, page, hint) {
            Ok(Some(reading)) => {
                read(Ok(Some(reading.text)));
                return Some((reading.rotate, Basis::Ocr));
            }
            Ok(None) => {}
            Err(error) => read(Err(error)),
        }
    }
    upright_page(doc, page)
}
