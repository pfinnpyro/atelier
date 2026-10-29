//! Background threads that render tiles and read page text, so the window
//! never waits on the document.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use atelier_ocr::{Engine, read_page};
use atelier_render::{PageText, RenderOptions, extract_text, render_page, render_region};
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
        text: Arc<PageText>,
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
    /// Auto-orient: the upright /Rotate of each page and what it was
    /// judged from, None where a page gave nothing to go by.
    Oriented {
        generation: u64,
        rotations: Vec<Option<(u16, Basis)>>,
    },
    /// OCR read a page of the file opened as `opened`.
    OcrPage {
        opened: u64,
        page: usize,
    },
    /// The OCR models could not be loaded.
    OcrFailed {
        error: String,
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
            for (i, page) in pages.iter().enumerate() {
                if current.load(Ordering::SeqCst) != id {
                    return;
                }
                let text = page_text(&doc, i, page, &ocr);
                let hits = find_in_page(&text, i, &query)
                    .into_iter()
                    .map(|m| m.glyphs)
                    .collect();
                let _ = tx.send(Done::SearchHits {
                    generation,
                    search: id,
                    page: i,
                    hits,
                    text,
                });
                wake();
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

    /// Works out which way up each page reads, on threads of its own.
    /// `done` counts the pages finished, for showing progress.
    pub fn orient(
        &self,
        doc: Arc<Document>,
        generation: u64,
        done: Arc<AtomicUsize>,
        ocr: Option<OcrRun>,
        wake: impl Fn() + Send + Sync + 'static,
    ) {
        let tx = self.done_tx.clone();
        let engine = self.engine.clone();
        let _ = thread::Builder::new().name("orient".into()).spawn(move || {
            let ocr = ocr.and_then(|run| match load_engine(&engine, &run) {
                Ok(e) => Some((e, run)),
                Err(error) => {
                    let _ = tx.send(Done::OcrFailed { error });
                    None
                }
            });
            let count = doc.pages().map_or(0, <[_]>::len);
            let next = AtomicUsize::new(0);
            let results = Mutex::new(vec![None; count]);
            let cores = thread::available_parallelism().map_or(2, |n| n.get());
            // OCR spreads each page over the cores itself and takes a lot of
            // memory per page, so fewer pages go at once.
            let threads = if ocr.is_some() {
                (cores / 2).clamp(1, 4)
            } else {
                cores.clamp(1, 8)
            };
            thread::scope(|s| {
                for _ in 0..threads {
                    s.spawn(|| {
                        let Ok(pages) = doc.pages() else { return };
                        loop {
                            let i = next.fetch_add(1, Ordering::SeqCst);
                            let Some(page) = pages.get(i) else { break };
                            // A page that panics the renderer is left as it is.
                            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                let ocr = ocr.as_ref().map(|(e, run)| (&**e, run));
                                orient_page(&doc, i, page, ocr, &|| {
                                    let _ = tx.send(Done::OcrPage {
                                        opened: generation,
                                        page: i,
                                    });
                                    wake();
                                })
                            }))
                            .ok()
                            .flatten();
                            results.lock().unwrap_or_else(|e| e.into_inner())[i] = r;
                            done.fetch_add(1, Ordering::SeqCst);
                            wake();
                        }
                    });
                }
            });
            let rotations = results.into_inner().unwrap_or_else(|e| e.into_inner());
            let _ = tx.send(Done::Oriented {
                generation,
                rotations,
            });
            wake();
        });
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

    /// Stops the running search, if any.
    pub fn stop_search(&self) {
        self.search.fetch_add(1, Ordering::SeqCst);
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
    read: &dyn Fn(),
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
            return upright_rotation(&t).map(|r| (r, Basis::Ocr));
        }
        if let Ok(Some(reading)) = read_page(engine, doc, page) {
            run.texts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(index, Arc::new(reading.text));
            run.read.fetch_add(1, Ordering::SeqCst);
            read();
            return Some((reading.rotate, Basis::Ocr));
        }
    }
    upright_page(doc, page)
}
