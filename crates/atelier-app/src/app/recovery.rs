//! Durable checkpoints for unsaved edits in unencrypted PDFs.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak, mpsc};
use std::time::{Duration, Instant};

use atelier_syntax::Document;
use eframe::egui;

use super::{Viewer, display_name, write_replacing_with};

const INTERVAL: Duration = Duration::from_secs(3);
const MAX_PDF: usize = 256 * 1024 * 1024;
const MAX_PATH: usize = 128 * 1024;
const MAGIC: &[u8; 8] = b"ATLDRAFT";

fn root() -> Option<PathBuf> {
    Some(
        crate::settings::Settings::ocr_models_dir()?
            .parent()?
            .join("recovery"),
    )
}

struct Entry {
    directory: PathBuf,
    lock: Option<File>,
    discard: AtomicBool,
}

impl Entry {
    fn lock(directory: PathBuf) -> io::Result<Self> {
        let lock = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("lock"))?;
        lock.try_lock()?;
        Ok(Self {
            directory,
            lock: Some(lock),
            discard: AtomicBool::new(false),
        })
    }

    fn create() -> io::Result<Self> {
        let root = root().ok_or_else(|| io::Error::other("No local data directory"))?;
        fs::create_dir_all(&root)?;
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let mut builder = builder;
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        let nonce = getrandom::u64().map_err(|error| io::Error::other(error.to_string()))?;
        let directory = root.join(format!("{nonce:016x}"));
        builder.create(&directory)?;
        Self::lock(directory)
    }

    fn path(&self) -> PathBuf {
        self.directory.join("draft")
    }

    fn discard(&self) -> io::Result<()> {
        match fs::remove_file(self.path()) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        self.discard.store(true, Ordering::SeqCst);
        Ok(())
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        let discard = self.discard.load(Ordering::SeqCst);
        if discard {
            let _ = fs::remove_file(self.path());
            if let Ok(files) = fs::read_dir(&self.directory) {
                for file in files.flatten() {
                    if file
                        .file_name()
                        .as_encoded_bytes()
                        .starts_with(b".atelier-saving-")
                    {
                        let _ = fs::remove_file(file.path());
                    }
                }
            }
        }
        if let Some(lock) = self.lock.take() {
            // A forked child may still have the descriptor open until it execs.
            let _ = lock.unlock();
        }
        if discard {
            let _ = fs::remove_file(self.directory.join("lock"));
            let _ = fs::remove_dir(&self.directory);
        }
    }
}

#[derive(Clone)]
enum Source {
    Opened(PathBuf, Arc<Document>),
    Fingerprint(PathBuf, u64, [u8; 32]),
}

impl Source {
    fn fingerprint(&self) -> (&Path, u64, [u8; 32]) {
        match self {
            Self::Opened(path, doc) => (
                path,
                doc.data().len() as u64,
                atelier_crypto::sha256(doc.data()),
            ),
            Self::Fingerprint(path, len, hash) => (path, *len, *hash),
        }
    }

    fn status(&self) -> io::Result<&'static str> {
        let (path, len, expected) = self.fingerprint();
        let file = match File::open(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok("Source is missing.");
            }
            result => result?,
        };
        if file.metadata()?.len() != len || len > MAX_PDF as u64 {
            return Ok("Source has changed.");
        }
        struct HashWriter(atelier_crypto::Sha256);
        impl Write for HashWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.update(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut hash = HashWriter(atelier_crypto::Sha256::new());
        if io::copy(&mut file.take(len + 1), &mut hash)? == len && hash.0.finish() == expected {
            Ok("Source matches the saved revision.")
        } else {
            Ok("Source has changed.")
        }
    }
}

#[derive(Default)]
pub(super) struct Draft {
    entry: Option<Arc<Entry>>,
    source: Option<Source>,
    restored: bool,
    last: Weak<Document>,
    last_pending: PendingEdits,
    job: Option<mpsc::Receiver<io::Result<Option<Source>>>>,
}

#[derive(Default, PartialEq, Clone)]
struct PendingEdits {
    field: Option<(usize, atelier_form::Value)>,
    ink: Option<(usize, atelier_form::NewAnnot)>,
}

impl PendingEdits {
    fn apply(self, doc: Arc<Document>) -> io::Result<Arc<Document>> {
        if self == Self::default() {
            return Ok(doc);
        }
        let mut session = atelier_form::Session::new(doc);
        if let Some((field, value)) = self.field {
            session.set(field, value);
        }
        if let Some((page, ink)) = self.ink {
            session.add_annot(page, ink);
        }
        session
            .build()
            .map(|built| built.doc)
            .map_err(io::Error::other)
    }
}

impl Draft {
    pub(super) fn new(path: Option<&Path>, doc: &Arc<Document>) -> Self {
        Self {
            source: path
                .and_then(|path| std::path::absolute(path).ok())
                .map(|path| Source::Opened(path, doc.clone())),
            ..Self::default()
        }
    }

    pub(super) fn restored(&self) -> bool {
        self.restored
    }

    pub(super) fn clear(&mut self) {
        if let Some(entry) = self.entry.take() {
            let _ = entry.discard();
        }
        self.restored = false;
        self.last = Weak::new();
        self.last_pending = PendingEdits::default();
        self.job = None;
    }

    pub(super) fn saved(&mut self, overwrite: bool, path: Option<&Path>, doc: &Arc<Document>) {
        self.clear();
        if overwrite {
            self.source = Self::new(path, doc).source;
        }
    }

    fn checkpoint(
        &mut self,
        doc: &Arc<Document>,
        pending: PendingEdits,
        ctx: &egui::Context,
    ) -> io::Result<()> {
        if let Some(job) = &self.job {
            let result = match job.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => return Ok(()),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Err(io::Error::other("Recovery worker stopped"))
                }
            };
            self.job = None;
            match result {
                Ok(source) => self.source = source,
                Err(error) => {
                    self.last = Weak::new();
                    return Err(error);
                }
            }
        }
        if self.last.ptr_eq(&Arc::downgrade(doc)) && self.last_pending == pending {
            return Ok(());
        }
        if self.entry.is_none() {
            self.entry = Some(Arc::new(Entry::create()?));
        }
        let entry = self.entry.as_ref().unwrap().clone();
        let source = self.source.clone();
        let wake = ctx.clone();
        let document = doc.clone();
        let edits = pending.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("recovery".into())
            .spawn(move || {
                if !entry.discard.load(Ordering::SeqCst) {
                    let result = edits.apply(document).and_then(|edited| {
                        write_draft(&entry.path(), source.as_ref(), edited.data())
                    });
                    // Release the worker's ownership before reporting completion.
                    drop(entry);
                    let _ = tx.send(result);
                }
                wake.request_repaint();
            })?;
        self.last = Arc::downgrade(doc);
        self.last_pending = pending;
        self.job = Some(rx);
        Ok(())
    }
}

fn encode_path(path: &Path) -> Vec<u8> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect()
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
}

fn decode_path(bytes: Vec<u8>) -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        if !bytes.len().is_multiple_of(2) {
            return Err(invalid_draft());
        }
        let wide: Vec<_> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .copied()
            .map(u16::from_le_bytes)
            .collect();
        Ok(PathBuf::from(std::ffi::OsString::from_wide(&wide)))
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
}

fn invalid_draft() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid recovery draft")
}

fn write_draft(target: &Path, source: Option<&Source>, pdf: &[u8]) -> io::Result<Option<Source>> {
    let (path, len, hash) = source.map_or((Path::new(""), 0, [0; 32]), Source::fingerprint);
    let encoded = encode_path(path);
    if encoded.len() > MAX_PATH || pdf.len() > MAX_PDF {
        return Err(io::Error::other("Document exceeds the recovery size limit"));
    }
    write_replacing_with(target, |file| {
        file.write_all(MAGIC)?;
        file.write_all(&(encoded.len() as u32).to_le_bytes())?;
        file.write_all(&len.to_le_bytes())?;
        file.write_all(&hash)?;
        file.write_all(&encoded)?;
        file.write_all(pdf)
    })?;
    Ok(source.map(|_| Source::Fingerprint(path.to_owned(), len, hash)))
}

fn read_header(file: &mut File) -> io::Result<Option<Source>> {
    let mut magic = [0; 8];
    let mut path_len = [0; 4];
    let mut len = [0; 8];
    let mut hash = [0; 32];
    file.read_exact(&mut magic)?;
    file.read_exact(&mut path_len)?;
    file.read_exact(&mut len)?;
    file.read_exact(&mut hash)?;
    let path_len = u32::from_le_bytes(path_len) as usize;
    if &magic != MAGIC || path_len > MAX_PATH {
        return Err(invalid_draft());
    }
    if path_len == 0 {
        return Ok(None);
    }
    let mut path = vec![0; path_len];
    file.read_exact(&mut path)?;
    Ok(Some(Source::Fingerprint(
        decode_path(path)?,
        u64::from_le_bytes(len),
        hash,
    )))
}

pub(super) struct Candidate {
    entry: Arc<Entry>,
    source: Option<Source>,
}

type RestoreResult = io::Result<(Document, &'static str)>;
pub(super) type Restore = mpsc::Receiver<(Candidate, RestoreResult)>;

impl Candidate {
    fn read(&self) -> RestoreResult {
        let mut file = File::open(self.entry.path())?;
        read_header(&mut file)?;
        let mut data = Vec::new();
        file.take(MAX_PDF as u64 + 1).read_to_end(&mut data)?;
        if data.len() > MAX_PDF {
            return Err(invalid_draft());
        }
        let doc = Document::open(Arc::<[u8]>::from(data)).map_err(io::Error::other)?;
        doc.pages().map_err(io::Error::other)?;
        if doc.is_encrypted() || !atelier_pgp::protected_payload(&doc).is_ok_and(|p| p.is_none()) {
            return Err(invalid_draft());
        }
        let status = self
            .source
            .as_ref()
            .map_or("Recovered an unsaved document.", |source| {
                source.status().unwrap_or("Source could not be checked.")
            });
        Ok((doc, status))
    }
}

pub(super) fn discover() -> Vec<Candidate> {
    let Some(entries) = root().and_then(|root| fs::read_dir(root).ok()) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|dir| {
            let name = dir.file_name();
            let name = name.as_encoded_bytes();
            if name.len() != 16
                || !name.iter().all(u8::is_ascii_hexdigit)
                || !dir.file_type().ok()?.is_dir()
                || !dir.path().join("draft").is_file()
            {
                return None;
            }
            let entry = Arc::new(Entry::lock(dir.path()).ok()?);
            let mut file = File::open(entry.path()).ok()?;
            let source = read_header(&mut file).ok()?;
            Some(Candidate { entry, source })
        })
        .collect()
}

impl Viewer {
    pub(super) fn checkpoint_recovery(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        if self.tabs.is_empty() {
            return;
        }
        if now < self.recovery_tick_at {
            ctx.request_repaint_after(self.recovery_tick_at - now);
            return;
        }
        self.recovery_tick_at = now + INTERVAL;
        ctx.request_repaint_after(INTERVAL);
        for (index, open) in self
            .open
            .iter_mut()
            .chain(self.tabs.iter_mut().flatten().map(|tab| &mut tab.open))
            .enumerate()
        {
            if open.readers.is_some() || open.doc.is_encrypted() {
                continue;
            }
            let pending = PendingEdits {
                field: open
                    .form
                    .as_ref()
                    .and_then(super::form::FormView::draft_value),
                ink: (index == 0).then(|| self.annotate.draft_ink()).flatten(),
            };
            if open.session.is_dirty()
                || open.recovery.restored()
                || pending != PendingEdits::default()
            {
                if let Err(error) = open.recovery.checkpoint(&open.doc, pending, ctx) {
                    self.error = Some(format!("Could not checkpoint {}: {error}", open.name));
                }
            } else if open.recovery.entry.is_some() {
                open.recovery.clear();
            }
        }
    }

    pub(super) fn recovery_window(&mut self, ctx: &egui::Context) {
        if self.confirm.is_some() || self.locked.is_some() {
            return;
        }
        if let Some(restore) = &self.recovery_restore {
            match restore.try_recv() {
                Ok((candidate, result)) => {
                    self.recovery_restore = None;
                    match result {
                        Ok((doc, status)) => {
                            let name = format!("Recovered - {}", candidate.label());
                            let draft = Draft {
                                entry: Some(candidate.entry),
                                source: candidate.source,
                                restored: true,
                                ..Draft::default()
                            };
                            self.install(doc, name, None, None, Some(draft));
                            if let Some(open) = &mut self.open {
                                open.recovery.last = Arc::downgrade(&open.doc);
                            }
                            self.notice = Some(status.into());
                        }
                        Err(error) => {
                            self.error = Some(format!("Could not restore recovery draft: {error}"))
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.recovery_restore = None;
                    self.error = Some("Recovery worker stopped.".into());
                }
            }
            return;
        }
        let Some(candidate) = self.recovery_queue.last() else {
            return;
        };
        let source_path = candidate
            .source
            .as_ref()
            .map(|source| source.fingerprint().0);
        enum Choice {
            Restore,
            Discard,
            Later,
        }
        let mut choice = None;
        egui::Modal::new(egui::Id::unique("recovery")).show(ctx, |ui| {
            ui.heading("Recover unsaved changes");
            ui.label(candidate.label());
            if let Some(path) = source_path {
                ui.label(path.display().to_string());
            }
            ui.label("Restore opens the draft in a new tab. Save it to a location you choose.");
            ui.horizontal(|ui| {
                if ui.button("Restore").clicked() {
                    choice = Some(Choice::Restore);
                }
                if ui.button("Discard Draft").clicked() {
                    choice = Some(Choice::Discard);
                }
                if ui.button("Later").clicked() {
                    choice = Some(Choice::Later);
                }
            });
        });
        match choice {
            Some(Choice::Discard) => match candidate.entry.discard() {
                Ok(()) => {
                    self.recovery_queue.pop();
                }
                Err(error) => {
                    self.error = Some(format!("Could not discard recovery draft: {error}"));
                }
            },
            Some(Choice::Later) => {
                self.recovery_queue.pop();
            }
            Some(Choice::Restore) => {
                let candidate = self.recovery_queue.pop().unwrap();
                let (tx, rx) = mpsc::channel();
                let wake = ctx.clone();
                match std::thread::Builder::new()
                    .name("restore".into())
                    .spawn(move || {
                        let result = candidate.read();
                        let _ = tx.send((candidate, result));
                        wake.request_repaint();
                    }) {
                    Ok(_) => self.recovery_restore = Some(rx),
                    Err(error) => {
                        self.error =
                            Some(format!("Could not start restoring recovery draft: {error}"))
                    }
                }
            }
            None => {}
        }
    }
}

impl Candidate {
    fn label(&self) -> String {
        self.source.as_ref().map_or_else(
            || "Untitled PDF".into(),
            |source| display_name(source.fingerprint().0),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "invoked as a disposable child process by the interruption test"]
    fn checkpoint_writer_child() {
        let folder = PathBuf::from(std::env::var_os("ATELIER_CRASH_TEST_DIR").unwrap());
        let entry = Entry::lock(folder.clone()).unwrap();
        write_draft(&entry.path(), None, b"first checkpoint").unwrap();
        write_replacing_with(&entry.path(), |file| {
            file.write_all(b"incomplete replacement")?;
            file.sync_all()?;
            fs::write(folder.join("ready"), [])?;
            std::thread::sleep(Duration::from_secs(30));
            Err(io::Error::other("Interruption test did not stop the child"))
        })
        .unwrap();
    }

    #[test]
    fn process_termination_keeps_the_last_complete_checkpoint() {
        let scratch = super::super::security_provider::Scratch::new().unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "app::recovery::tests::checkpoint_writer_child",
            ])
            .env("ATELIER_CRASH_TEST_DIR", &scratch.0)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !scratch.0.join("ready").exists()
            && Instant::now() < deadline
            && child.try_wait().unwrap().is_none()
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        let was_writing = scratch.0.join("ready").exists();
        let _ = child.kill();
        child.wait().unwrap();
        assert!(
            was_writing,
            "child never began writing the second checkpoint"
        );
        let entry = Entry::lock(scratch.0.clone()).unwrap();
        let mut file = File::open(entry.path()).unwrap();
        assert!(read_header(&mut file).unwrap().is_none());
        let mut data = Vec::new();
        file.read_to_end(&mut data).unwrap();
        assert_eq!(data, b"first checkpoint");
    }

    #[test]
    fn checkpoint_from_a_live_editor_restores_as_unsaved_after_source_changes() {
        let scratch = super::super::security_provider::Scratch::new().unwrap();
        let source = scratch.0.join("Original.pdf");
        let original = super::super::quality_tests::fixture(1);
        fs::write(&source, &original).unwrap();
        let doc = Arc::new(Document::open(original).unwrap());
        let folder = scratch.0.join("session");
        fs::create_dir(&folder).unwrap();
        let ctx = egui::Context::default();
        let mut draft = Draft::new(Some(&source), &doc);
        draft.entry = Some(Arc::new(Entry::lock(folder.clone()).unwrap()));
        draft
            .checkpoint(
                &doc,
                PendingEdits {
                    field: Some((0, atelier_form::Value::Text("Still typing".into()))),
                    ink: None,
                },
                &ctx,
            )
            .unwrap();
        let saved_source = draft
            .job
            .take()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        drop(draft);

        let changed = b"other bytes";
        fs::write(&source, changed).unwrap();
        let entry = Arc::new(Entry::lock(folder).unwrap());
        let candidate = Candidate {
            entry,
            source: saved_source,
        };
        let result = candidate.read();
        let (tx, rx) = mpsc::channel();
        tx.send((candidate, result)).unwrap();
        let mut viewer = Viewer::blank(ctx.clone(), crate::settings::Settings::default());
        viewer.recovery_restore = Some(rx);
        viewer.recovery_window(&ctx);

        assert!(viewer.is_dirty());
        assert_eq!(viewer.notice.as_deref(), Some("Source has changed."));
        assert_eq!(fs::read(source).unwrap(), changed);
        let open = viewer.open.as_ref().unwrap();
        assert!(open.path.is_none());
        let field = &open.form.as_ref().unwrap().form.fields[0];
        assert_eq!(field.value.display(), "Still typing");
        let checkpoint = open.recovery.entry.as_ref().unwrap().path();
        viewer.close_tab(0);
        assert!(!checkpoint.exists());
    }

    #[test]
    fn locked_checkpoint_survives_interrupted_replacement_and_detects_source_changes() {
        let scratch = super::super::security_provider::Scratch::new().unwrap();
        let draft_dir = scratch.0.join("draft-dir");
        fs::create_dir(&draft_dir).unwrap();
        let source_path = scratch.0.join("Source, résumé.pdf");
        fs::write(&source_path, b"original").unwrap();
        let fingerprint =
            Source::Fingerprint(source_path.clone(), 8, atelier_crypto::sha256(b"original"));
        let entry = Entry::lock(draft_dir.clone()).unwrap();
        write_draft(&entry.path(), Some(&fingerprint), b"first checkpoint").unwrap();
        assert!(Entry::lock(draft_dir.clone()).is_err());
        assert!(
            write_replacing_with(&entry.path(), |file| {
                file.write_all(b"partial checkpoint")?;
                Err(io::Error::other("simulated full disk"))
            })
            .is_err()
        );
        // On Unix a child can briefly inherit the lock descriptor while spawning.
        #[cfg(unix)]
        let inherited = entry.lock.as_ref().unwrap().try_clone().unwrap();
        drop(entry);
        let entry = Entry::lock(draft_dir.clone()).unwrap();
        #[cfg(unix)]
        drop(inherited);
        let mut file = File::open(entry.path()).unwrap();
        let stored = read_header(&mut file).unwrap().unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        drop(file);
        assert_eq!(bytes, b"first checkpoint");
        assert_eq!(stored.fingerprint().0, source_path);
        assert_eq!(
            stored.status().unwrap(),
            "Source matches the saved revision."
        );
        fs::write(&source_path, b"modified").unwrap();
        assert_eq!(stored.status().unwrap(), "Source has changed.");
        fs::remove_file(&source_path).unwrap();
        assert_eq!(stored.status().unwrap(), "Source is missing.");
        assert_eq!(fs::read_dir(&draft_dir).unwrap().count(), 2);
        entry.discard().unwrap();
        drop(entry);
        assert!(!draft_dir.exists());
    }

    #[cfg(windows)]
    #[test]
    fn read_only_target_keeps_prior_bytes_and_leaves_no_temporary_file() {
        let scratch = super::super::security_provider::Scratch::new().unwrap();
        let target = scratch.0.join("read-only.pdf");
        fs::write(&target, b"original").unwrap();
        let writable = fs::metadata(&target).unwrap().permissions();
        let mut read_only = writable.clone();
        read_only.set_readonly(true);
        fs::set_permissions(&target, read_only).unwrap();
        let outcome = super::super::write_replacing(&target, b"changed");
        fs::set_permissions(&target, writable).unwrap();
        assert!(outcome.is_err());
        assert_eq!(fs::read(&target).unwrap(), b"original");
        assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 1);
    }
}
