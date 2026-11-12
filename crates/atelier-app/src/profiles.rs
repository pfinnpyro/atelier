//! Saved form profiles: one file each in a `profiles` folder beside the
//! settings file.
//!
//! A profile is kept either as plain text or sealed under a passphrase.
//! A sealed profile is only ever decrypted into memory: unlocking reads
//! the file and keeps the details in this process, nothing decrypted is
//! written anywhere, and locking overwrites the details and the keys.
//! The passphrase itself is never kept, only the keys worked out from
//! it, and those only while the profile is unlocked.
//!
//! A sealed file shows the profile's name, so the list can be shown
//! without a passphrase, and nothing else.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use atelier_crypto::wipe;
use atelier_form::{Profile, wipe_string};

use crate::sealing;
use crate::settings::Settings;

const PLAIN: &str = "atelier-profile 1";
const SEALED: &str = "atelier-profile-sealed 1";
const KDF: &str = "pbkdf2-hmac-sha256";
/// What each guess at a passphrase costs. Saved with the profile, so it
/// can be raised without locking anyone out.
const ITERATIONS: u32 = 600_000;
/// A file asking for more work than this is not one Atelier wrote.
const MOST_ITERATIONS: u32 = 20_000_000;

/// Why a profile could not be unlocked or saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// The passphrase is wrong, or the file was changed.
    WrongPassphrase,
    /// The file is not a profile.
    Damaged,
    /// The profile is sealed and was not unlocked first.
    Locked,
    /// The system gave no random numbers to seal with.
    NoRandomness,
    /// The cryptographic module refused or failed.
    Module,
    /// There is nowhere to keep profiles.
    NoFolder,
    Io(String),
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongPassphrase => f.write_str("That passphrase is not right."),
            Self::Damaged => f.write_str("The profile's file is damaged."),
            Self::Locked => f.write_str("Unlock the profile first."),
            Self::NoRandomness => f.write_str("The system gave no random numbers to encrypt with."),
            Self::Module => f.write_str("The system's cryptography could not be used."),
            Self::NoFolder => f.write_str("There is no settings folder to keep profiles in."),
            Self::Io(e) => write!(f, "Could not save the profile: {e}"),
        }
    }
}

impl From<std::io::Error> for Problem {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// What is needed to seal a profile again without its passphrase.
pub struct Keys {
    key: sealing::Key,
    salt: [u8; 16],
    iterations: u32,
}

pub enum State {
    /// Kept as plain text.
    Plain(Profile),
    /// Sealed, and not decrypted.
    Locked,
    /// Sealed on disk, decrypted in memory since `used`.
    Unlocked {
        profile: Profile,
        keys: Keys,
        used: Instant,
    },
}

pub struct Item {
    /// The file's name without its extension.
    pub id: String,
    pub name: String,
    pub state: State,
    /// Goes up whenever the details change.
    pub revision: u64,
}

impl Item {
    /// The details, unless they are locked away.
    pub fn profile(&self) -> Option<&Profile> {
        match &self.state {
            State::Plain(p) | State::Unlocked { profile: p, .. } => Some(p),
            State::Locked => None,
        }
    }

    /// Sealed on disk, locked or not.
    pub fn encrypted(&self) -> bool {
        !matches!(self.state, State::Plain(_))
    }

    pub fn locked(&self) -> bool {
        matches!(self.state, State::Locked)
    }
}

pub struct Store {
    pub items: Vec<Item>,
    dir: Option<PathBuf>,
    iterations: u32,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

fn random<const N: usize>() -> Result<[u8; N], Problem> {
    let mut bytes = [0u8; N];
    if sealing::random(&mut bytes) {
        Ok(bytes)
    } else {
        Err(Problem::NoRandomness)
    }
}

/// A sealed profile as its file has it.
struct SealedFile {
    name: String,
    iterations: u32,
    salt: [u8; 16],
    iv: [u8; 16],
    data: Vec<u8>,
    tag: Vec<u8>,
}

/// What the tag covers besides the ciphertext: changing the name or the
/// cost of a guess makes the profile fail to open.
fn header(name: &str, iterations: u32, salt: &[u8; 16]) -> Vec<u8> {
    format!("{SEALED}\n{name}\n{KDF}\n{iterations}\n{}", hex(salt)).into_bytes()
}

impl SealedFile {
    fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        if lines.next()?.trim() != SEALED {
            return None;
        }
        let (mut name, mut iterations, mut salt, mut iv, mut data, mut tag) =
            (None, None, None, None, None, None);
        for line in lines {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "profile" => name = Some(value.to_string()),
                "kdf" if value != KDF => return None,
                "iterations" => iterations = value.parse::<u32>().ok(),
                "salt" => salt = unhex(value),
                "iv" => iv = unhex(value),
                "data" => data = unhex(value),
                "tag" => tag = unhex(value),
                _ => {}
            }
        }
        Some(Self {
            name: name?,
            iterations: iterations.filter(|n| (1..=MOST_ITERATIONS).contains(n))?,
            salt: salt?.try_into().ok()?,
            iv: iv?.try_into().ok()?,
            data: data?,
            tag: tag?,
        })
    }

    fn to_text(&self) -> String {
        format!(
            "{SEALED}\nprofile = {}\nkdf = {KDF}\niterations = {}\nsalt = {}\niv = {}\ndata = {}\ntag = {}\n",
            self.name,
            self.iterations,
            hex(&self.salt),
            hex(&self.iv),
            hex(&self.data),
            hex(&self.tag)
        )
    }
}

/// A profile's name as one line.
fn one_line(name: &str) -> String {
    name.replace(['\n', '\r'], " ").trim().to_string()
}

/// Overwrites a file's contents before it is replaced or removed, so
/// details kept as plain text do not linger where the file was. Drives
/// that move data around as they write (most solid-state ones) may still
/// keep an old copy; nothing a program does can help that.
fn scrub(path: &Path) {
    use std::io::Write;
    let Ok(len) = std::fs::metadata(path).map(|m| m.len()) else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ = file.write_all(&vec![0u8; len as usize]);
        let _ = file.sync_all();
    }
}

/// Writes a profile's file so that only its owner can read it.
fn write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    scrub(path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    std::io::Write::write_all(&mut file, data)?;
    file.sync_all()
}

impl Store {
    /// The profiles in the user's settings folder.
    pub fn load() -> Self {
        Self::at(Settings::path().map(|p| p.with_file_name("profiles")))
    }

    /// The profiles in a folder. Sealed ones start locked.
    pub fn at(dir: Option<PathBuf>) -> Self {
        let mut store = Self {
            items: Vec::new(),
            dir,
            iterations: ITERATIONS,
        };
        let Some(entries) = store.dir.as_ref().and_then(|d| std::fs::read_dir(d).ok()) else {
            return store;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "profile") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(mut text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let item = if let Some(sealed) = SealedFile::parse(&text) {
                Some((sealed.name, State::Locked))
            } else if let Some(body) = text.strip_prefix(PLAIN) {
                let profile = Profile::parse(body);
                Some((profile.name.clone(), State::Plain(profile)))
            } else {
                None
            };
            wipe_string(&mut text);
            if let Some((name, state)) = item {
                store.items.push(Item {
                    id: id.to_string(),
                    name,
                    state,
                    revision: 0,
                });
            }
        }
        store.sort();
        store
    }

    /// A store for tests: a folder of its own, and passphrases cheap
    /// enough to guess that the tests stay quick.
    #[cfg(test)]
    pub fn for_test(test: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "atelier-profiles-{test}-{}",
            hex(&random::<6>().unwrap())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = Self::at(Some(dir));
        store.iterations = 50;
        store
    }

    /// Removes a test's folder.
    #[cfg(test)]
    pub fn clean_up(self) {
        if let Some(dir) = &self.dir {
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    fn sort(&mut self) {
        self.items
            .sort_by_cached_key(|i| (i.name.to_lowercase(), i.id.clone()));
    }

    fn path(&self, id: &str) -> Result<PathBuf, Problem> {
        self.dir
            .as_ref()
            .map(|d| d.join(format!("{id}.profile")))
            .ok_or(Problem::NoFolder)
    }

    pub fn find(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    fn find_mut(&mut self, id: &str) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    /// Decrypts a sealed profile into memory.
    pub fn unlock(&mut self, id: &str, passphrase: &str) -> Result<(), Problem> {
        let path = self.path(id)?;
        let text = std::fs::read_to_string(path)?;
        let sealed = SealedFile::parse(&text).ok_or(Problem::Damaged)?;
        let key = sealing::Key::derive(passphrase.as_bytes(), &sealed.salt, sealed.iterations)
            .ok_or(Problem::Module)?;
        let secret = key
            .open(
                &sealed.iv,
                &header(&sealed.name, sealed.iterations, &sealed.salt),
                &sealed.data,
                &sealed.tag,
            )
            .ok_or(Problem::WrongPassphrase)?;
        // The bytes become the text without being copied.
        let mut text = match String::from_utf8(secret) {
            Ok(t) => t,
            Err(e) => {
                wipe(&mut e.into_bytes());
                return Err(Problem::Damaged);
            }
        };
        let mut profile = Profile::parse(&text);
        wipe_string(&mut text);
        profile.name = sealed.name.clone();
        let item = self.find_mut(id).ok_or(Problem::Damaged)?;
        item.name = sealed.name;
        item.revision += 1;
        item.state = State::Unlocked {
            profile,
            keys: Keys {
                key,
                salt: sealed.salt,
                iterations: sealed.iterations,
            },
            used: Instant::now(),
        };
        Ok(())
    }

    /// Forgets a sealed profile's details and keys, overwriting them.
    /// Returns whether it was unlocked.
    pub fn lock(&mut self, id: &str) -> bool {
        let Some(item) = self.find_mut(id) else {
            return false;
        };
        if !matches!(item.state, State::Unlocked { .. }) {
            return false;
        }
        // Dropping the old state wipes the profile and the keys.
        item.state = State::Locked;
        item.revision += 1;
        true
    }

    /// Locks every unlocked profile; returns whether any was.
    pub fn lock_all(&mut self) -> bool {
        let ids: Vec<String> = self.items.iter().map(|i| i.id.clone()).collect();
        ids.iter().fold(false, |any, id| self.lock(id) | any)
    }

    /// Locks profiles nobody has used for `idle`; returns whether any
    /// was locked, and how long until the next one would be.
    pub fn lock_idle(&mut self, idle: Duration) -> (bool, Option<Duration>) {
        let now = Instant::now();
        let mut stale = Vec::new();
        let mut next: Option<Duration> = None;
        for item in &self.items {
            if let State::Unlocked { used, .. } = &item.state {
                let since = now.duration_since(*used);
                if since >= idle {
                    stale.push(item.id.clone());
                } else {
                    let left = idle - since;
                    next = Some(next.map_or(left, |n| n.min(left)));
                }
            }
        }
        let any = stale.iter().fold(false, |any, id| self.lock(id) | any);
        (any, next)
    }

    /// Notes that an unlocked profile is in use, putting off its locking.
    pub fn touch(&mut self, id: &str) {
        if let Some(Item {
            state: State::Unlocked { used, .. },
            ..
        }) = self.find_mut(id)
        {
            *used = Instant::now();
        }
    }

    fn new_id(&self) -> Result<String, Problem> {
        loop {
            let id = hex(&random::<8>()?);
            if self.find(&id).is_none() {
                return Ok(id);
            }
        }
    }

    /// Puts a saved profile in the list, in place of the one it replaces.
    fn put(&mut self, id: String, name: String, state: State) -> String {
        let revision = self.find(&id).map_or(0, |i| i.revision) + 1;
        self.items.retain(|i| i.id != id);
        self.items.push(Item {
            id: id.clone(),
            name,
            state,
            revision,
        });
        self.sort();
        id
    }

    /// Saves a profile as plain text: a new one, or over the one with
    /// this id. Saving over a sealed profile takes its encryption off.
    pub fn save_plain(
        &mut self,
        id: Option<&str>,
        mut profile: Profile,
    ) -> Result<String, Problem> {
        if id.and_then(|id| self.find(id)).is_some_and(Item::locked) {
            return Err(Problem::Locked);
        }
        profile.name = one_line(&profile.name);
        let id = match id {
            Some(id) => id.to_string(),
            None => self.new_id()?,
        };
        let mut text = format!("{PLAIN}\n{}", profile.to_text());
        let written = write(&self.path(&id)?, text.as_bytes());
        wipe_string(&mut text);
        written?;
        let name = profile.name.clone();
        Ok(self.put(id, name, State::Plain(profile)))
    }

    /// Saves a profile sealed: a new one, or over the one with this id,
    /// whose plain text is overwritten first if it had any. A new
    /// passphrase makes new keys; without one, the profile must be
    /// unlocked and keeps the keys it has. It stays unlocked afterwards.
    pub fn save_sealed(
        &mut self,
        id: Option<&str>,
        mut profile: Profile,
        passphrase: Option<&str>,
    ) -> Result<String, Problem> {
        profile.name = one_line(&profile.name);
        let fresh = match passphrase {
            Some(passphrase) => {
                let salt = random::<16>()?;
                Some(Keys {
                    key: sealing::Key::derive(passphrase.as_bytes(), &salt, self.iterations)
                        .ok_or(Problem::Module)?,
                    salt,
                    iterations: self.iterations,
                })
            }
            None => None,
        };
        let id = match id {
            Some(id) => id.to_string(),
            None => self.new_id()?,
        };
        let path = self.path(&id)?;
        let keys = match (&fresh, self.find(&id).map(|i| &i.state)) {
            (Some(keys), _) | (None, Some(State::Unlocked { keys, .. })) => keys,
            _ => return Err(Problem::Locked),
        };
        let iv = random::<16>()?;
        let mut text = profile.to_text();
        let sealed = keys.key.seal(
            &iv,
            &header(&profile.name, keys.iterations, &keys.salt),
            text.as_bytes(),
        );
        wipe_string(&mut text);
        let (data, tag) = sealed.ok_or(Problem::Module)?;
        let file = SealedFile {
            name: profile.name.clone(),
            iterations: keys.iterations,
            salt: keys.salt,
            iv,
            data,
            tag: tag.to_vec(),
        };
        write(&path, file.to_text().as_bytes())?;
        // The file is written; the keys move on to the new state.
        let keys = match fresh {
            Some(keys) => keys,
            None => match self
                .find_mut(&id)
                .map(|i| std::mem::replace(&mut i.state, State::Locked))
            {
                Some(State::Unlocked { keys, .. }) => keys,
                _ => return Err(Problem::Locked),
            },
        };
        let name = profile.name.clone();
        Ok(self.put(
            id,
            name,
            State::Unlocked {
                profile,
                keys,
                used: Instant::now(),
            },
        ))
    }

    /// Removes a profile and its file.
    pub fn delete(&mut self, id: &str) -> Result<(), Problem> {
        let path = self.path(id)?;
        scrub(&path);
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
        self.items.retain(|i| i.id != id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atelier_form::{Entry, EntryKind, Standard};

    /// An empty folder of its own for a test.
    fn folder(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "atelier-profiles-{test}-{}",
            hex(&random::<6>().unwrap())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn store(dir: &Path) -> Store {
        let mut s = Store::at(Some(dir.to_path_buf()));
        // Enough to test with; the real cost would slow the tests down.
        s.iterations = 50;
        s
    }

    fn profile(name: &str) -> Profile {
        Profile {
            name: name.into(),
            entries: vec![
                Entry {
                    kind: EntryKind::Standard(Standard::Name),
                    value: "Ada Lovelace".into(),
                },
                Entry {
                    kind: EntryKind::Standard(Standard::MailingAddress),
                    value: "PO Box 12\nLondon".into(),
                },
                Entry {
                    kind: EntryKind::Custom("Member number".into()),
                    value: "M-4471".into(),
                },
            ],
        }
    }

    /// Everything the folder's files hold, as text.
    fn on_disk(dir: &Path) -> String {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| String::from_utf8_lossy(&std::fs::read(e.path()).unwrap()).into_owned())
            .collect()
    }

    #[test]
    fn plain_profiles_come_back_as_saved() {
        let dir = folder("plain");
        let mut s = store(&dir);
        let id = s.save_plain(None, profile("Home")).unwrap();
        s.save_plain(None, profile("away")).unwrap();
        let again = store(&dir);
        assert_eq!(
            again
                .items
                .iter()
                .map(|i| i.name.as_str())
                .collect::<Vec<_>>(),
            ["away", "Home"]
        );
        let item = again.find(&id).unwrap();
        assert!(!item.encrypted());
        assert_eq!(item.profile(), Some(&profile("Home")));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_sealed_profile_never_reaches_the_disk_decrypted() {
        let dir = folder("sealed");
        let mut s = store(&dir);
        let id = s
            .save_sealed(None, profile("Home"), Some("correct horse"))
            .unwrap();
        // Unlocked in memory after saving, and only its name on disk.
        assert_eq!(s.find(&id).unwrap().profile(), Some(&profile("Home")));
        let disk = on_disk(&dir);
        assert!(disk.contains("profile = Home"));
        for secret in ["Ada", "Lovelace", "PO Box", "London", "M-4471", "Member"] {
            assert!(!disk.contains(secret), "{secret} is on disk");
        }
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

        // A fresh start finds it locked, and only the passphrase opens it.
        let mut s = store(&dir);
        let item = s.find(&id).unwrap();
        assert!(item.locked() && item.encrypted());
        assert_eq!(item.profile(), None);
        assert_eq!(s.unlock(&id, "wrong"), Err(Problem::WrongPassphrase));
        assert!(s.find(&id).unwrap().locked());
        assert_eq!(s.unlock(&id, "correct horse"), Ok(()));
        assert_eq!(s.find(&id).unwrap().profile(), Some(&profile("Home")));
        // Unlocking wrote nothing.
        assert_eq!(on_disk(&dir), disk);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

        // Locking takes the details away again.
        assert!(s.lock(&id));
        assert!(!s.lock(&id));
        assert_eq!(s.find(&id).unwrap().profile(), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn edits_reseal_under_the_same_passphrase() {
        let dir = folder("reseal");
        let mut s = store(&dir);
        let id = s.save_sealed(None, profile("Home"), Some("one")).unwrap();
        let before = on_disk(&dir);
        let mut changed = profile("Home");
        changed.entries[0].value = "Augusta Ada King".into();
        // Locked profiles cannot be saved over without their passphrase.
        s.lock(&id);
        assert_eq!(
            s.save_sealed(Some(&id), profile("Home"), None),
            Err(Problem::Locked)
        );
        assert_eq!(
            s.save_plain(Some(&id), profile("Home")),
            Err(Problem::Locked)
        );
        s.unlock(&id, "one").unwrap();
        assert_eq!(s.save_sealed(Some(&id), changed.clone(), None).unwrap(), id);
        let after = on_disk(&dir);
        assert_ne!(before, after);
        assert!(!after.contains("Augusta"));
        let mut s = store(&dir);
        s.unlock(&id, "one").unwrap();
        assert_eq!(s.find(&id).unwrap().profile(), Some(&changed));

        // A new passphrase replaces the old one.
        s.save_sealed(Some(&id), changed.clone(), Some("two"))
            .unwrap();
        let mut s = store(&dir);
        assert_eq!(s.unlock(&id, "one"), Err(Problem::WrongPassphrase));
        assert_eq!(s.unlock(&id, "two"), Ok(()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn encryption_goes_on_and_comes_off() {
        let dir = folder("convert");
        let mut s = store(&dir);
        let id = s.save_plain(None, profile("Home")).unwrap();
        assert!(on_disk(&dir).contains("Ada Lovelace"));
        s.save_sealed(Some(&id), profile("Home"), Some("pass"))
            .unwrap();
        assert!(!on_disk(&dir).contains("Ada"));
        assert_eq!(s.items.len(), 1);
        assert!(s.find(&id).unwrap().encrypted());
        s.save_plain(Some(&id), profile("Home")).unwrap();
        assert!(on_disk(&dir).contains("Ada Lovelace"));
        assert!(!s.find(&id).unwrap().encrypted());
        s.delete(&id).unwrap();
        assert!(s.items.is_empty());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_changed_file_does_not_open() {
        let dir = folder("tamper");
        let mut s = store(&dir);
        let id = s.save_sealed(None, profile("Home"), Some("pass")).unwrap();
        let path = s.path(&id).unwrap();
        let good = std::fs::read_to_string(&path).unwrap();
        // The name and the cost of a guess are covered by the tag too.
        for (from, to) in [
            ("profile = Home", "profile = Work"),
            ("iterations = 50", "iterations = 1"),
        ] {
            assert!(good.contains(from));
            std::fs::write(&path, good.replace(from, to)).unwrap();
            let mut s = store(&dir);
            assert_eq!(s.unlock(&id, "pass"), Err(Problem::WrongPassphrase));
        }
        let data = good.find("data = ").unwrap() + 7;
        let mut bent = good.clone().into_bytes();
        bent[data] = if bent[data] == b'0' { b'1' } else { b'0' };
        std::fs::write(&path, bent).unwrap();
        assert_eq!(
            store(&dir).unlock(&id, "pass"),
            Err(Problem::WrongPassphrase)
        );
        std::fs::write(&path, "atelier-profile-sealed 1\nprofile = Home\n").unwrap();
        assert!(store(&dir).items.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn idle_profiles_lock_themselves() {
        let dir = folder("idle");
        let mut s = store(&dir);
        let id = s.save_sealed(None, profile("Home"), Some("pass")).unwrap();
        s.save_plain(None, profile("Plain")).unwrap();
        let (locked, next) = s.lock_idle(Duration::from_secs(60));
        assert!(!locked);
        assert!(next.is_some_and(|n| n <= Duration::from_secs(60)));
        std::thread::sleep(Duration::from_millis(30));
        s.touch(&id);
        assert!(!s.lock_idle(Duration::from_millis(25)).0);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(s.lock_idle(Duration::from_millis(25)), (true, None));
        assert!(s.find(&id).unwrap().locked());
        // Plain profiles have nothing to lock.
        assert!(!s.lock_all());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
