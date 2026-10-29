//! Background threads that render tiles and read page text, so the window
//! never waits on the document.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use atelier_render::{PageText, RenderOptions, extract_text, render_region};
use atelier_syntax::Document;
use atelier_view::{TileKey, find_in_page, upright_rotation};

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
    },
}

/// What a job makes, to tell two jobs for the same thing apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum JobKey {
    Tile(u64, TileKey),
    Text(u64, usize),
}

impl Job {
    fn generation(&self) -> u64 {
        match self {
            Job::Tile { generation, .. } | Job::Text { generation, .. } => *generation,
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
    /// Auto-orient: the upright /Rotate of each page, None where a page
    /// has too little text to tell.
    Oriented {
        generation: u64,
        rotations: Vec<Option<u16>>,
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
                let text = Arc::new(extract_text(&doc, page).unwrap_or_default());
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

    /// Works out which way up each page reads, on its own thread.
    pub fn orient(&self, doc: Arc<Document>, generation: u64, wake: impl Fn() + Send + 'static) {
        let tx = self.done_tx.clone();
        let _ = thread::Builder::new().name("orient".into()).spawn(move || {
            let rotations = doc
                .pages()
                .map(|pages| {
                    pages
                        .iter()
                        .map(|p| {
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                extract_text(&doc, p)
                                    .ok()
                                    .and_then(|t| upright_rotation(&t))
                            }))
                            .ok()
                            .flatten()
                        })
                        .collect()
                })
                .unwrap_or_default();
            let _ = tx.send(Done::Oriented {
                generation,
                rotations,
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
        Job::Text {
            doc,
            generation,
            page,
        } => {
            let text = doc
                .pages()
                .ok()
                .and_then(|p| p.get(page))
                .and_then(|p| extract_text(&doc, p).ok())
                .unwrap_or_default();
            Done::Text {
                generation,
                page,
                text: Arc::new(text),
            }
        }
    }
}
