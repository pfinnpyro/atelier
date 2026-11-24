//! One window for files opened from outside. The first Atelier to start
//! holds a lock and listens on the loopback interface; a later one started
//! with files hands them over and quits, so they open as tabs in the
//! window already there. Only someone who can read the user's own
//! settings folder knows where it listens and the secret it wants, so
//! other users on the machine cannot open files in it.
//!
//! Finder sends a running Mac app its files itself; this is for the
//! files Explorer, a desktop file manager or a terminal starts Atelier
//! with.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// What a handover starts with, so a stray connection is turned away early.
const HELLO: &str = "atelier-open 1";
/// The most a handover may be, in bytes.
const MOST: u64 = 1024 * 1024;

/// Being the Atelier that files are handed to.
pub struct Primary {
    listener: TcpListener,
    token: String,
    /// Held for as long as this Atelier runs; the lock goes with it.
    _lock: File,
}

fn dir() -> Option<PathBuf> {
    crate::settings::Settings::path()?
        .parent()
        .map(Path::to_path_buf)
}

/// Whether this Atelier is the one files are handed to.
pub enum Claim {
    Primary(Primary),
    /// Another Atelier is.
    Taken,
    /// None can be: there is nowhere to keep the lock, or nothing to
    /// listen with.
    Unavailable,
}

/// Tries to become the Atelier that files are handed to.
pub fn claim() -> Claim {
    let Some(dir) = dir() else {
        return Claim::Unavailable;
    };
    let _ = std::fs::create_dir_all(&dir);
    let Ok(lock) = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("instance.lock"))
    else {
        return Claim::Unavailable;
    };
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Claim::Taken,
        Err(_) => return Claim::Unavailable,
    }
    listen(&dir, lock).map_or(Claim::Unavailable, Claim::Primary)
}

fn listen(dir: &Path, lock: File) -> Option<Primary> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    let mut secret = [0u8; 32];
    getrandom::fill(&mut secret).ok()?;
    let token: String = secret.iter().map(|b| format!("{b:02x}")).collect();
    // Written beside and moved into place, so a later Atelier never reads
    // half of it.
    let info = dir.join("instance");
    let part = dir.join("instance.part");
    write_private(&part, format!("{port}\n{token}\n").as_bytes()).ok()?;
    std::fs::rename(&part, &info).ok()?;
    Some(Primary {
        listener,
        token,
        _lock: lock,
    })
}

#[cfg(unix)]
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = File::options()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(data)
}

/// The settings folder under `%APPDATA%` is the user's alone already.
#[cfg(not(unix))]
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, data)
}

/// Hands `paths` to the Atelier already running, which has the lock.
/// An empty list requests another window in that process.
/// False when it did not take them in time; this Atelier then opens
/// them itself.
pub fn hand_over(paths: &[PathBuf]) -> bool {
    let Some(dir) = dir() else { return false };
    let mut lines = Vec::new();
    for p in paths {
        let p = std::path::absolute(p).unwrap_or_else(|_| p.clone());
        match p.to_str() {
            Some(s) if !s.contains(['\n', '\r']) => lines.push(s.to_owned()),
            // A name that cannot be written as a line of text is opened here.
            _ => return false,
        }
    }
    // The one running may still be starting up; it is given a moment.
    let until = Instant::now() + Duration::from_secs(4);
    loop {
        if let Some((port, token)) = read_info(&dir.join("instance"))
            && send(port, &token, &lines).is_ok()
        {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn read_info(path: &Path) -> Option<(u16, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let port = lines.next()?.trim().parse().ok()?;
    let token = lines.next()?.trim().to_owned();
    Some((port, token))
}

fn send(port: u16, token: &str, lines: &[String]) -> std::io::Result<()> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(500))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut message = format!("{HELLO}\n{token}\n");
    for line in lines {
        message.push_str(line);
        message.push('\n');
    }
    stream.write_all(message.as_bytes())?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut answer = String::new();
    stream.take(16).read_to_string(&mut answer)?;
    if answer.trim() == "ok" {
        Ok(())
    } else {
        Err(std::io::Error::other(
            "the running Atelier turned the files away",
        ))
    }
}

impl Primary {
    /// Takes handovers on a thread of its own, passing on each batch of
    /// files and waking the window.
    pub fn serve(self, files: Sender<Vec<PathBuf>>, wake: eframe::egui::Context) {
        let Primary {
            listener,
            token,
            _lock: lock,
        } = self;
        let work = move || {
            // The lock lives as long as this thread, which is as long as
            // the window; naming it keeps the closure from leaving it out.
            let _lock = lock;
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                if let Some(paths) = receive(stream, &token) {
                    if files.send(paths).is_err() {
                        return;
                    }
                    wake.request_repaint_of(eframe::egui::ViewportId::ROOT);
                }
            }
        };
        let _ = std::thread::Builder::new()
            .name("instance".into())
            .spawn(work);
    }
}

fn receive(stream: TcpStream, token: &str) -> Option<Vec<PathBuf>> {
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .ok()?;
    let mut reader = BufReader::new(stream.try_clone().ok()?.take(MOST));
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    if line.trim_end() != HELLO {
        return None;
    }
    line.clear();
    reader.read_line(&mut line).ok()?;
    if !same(line.trim_end().as_bytes(), token.as_bytes()) {
        return None;
    }
    let mut rest = String::new();
    reader.read_to_string(&mut rest).ok()?;
    let paths = rest
        .lines()
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect();
    let mut stream = stream;
    stream.write_all(b"ok\n").ok()?;
    Some(paths)
}

/// Compares secrets without stopping at the first difference.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handover_reaches_the_listener_only_with_the_secret() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let mut got = Vec::new();
            for _ in 0..3 {
                let (stream, _) = listener.accept().unwrap();
                got.push(receive(stream, "secret"));
            }
            got
        });
        let lines = vec!["C:\\a.pdf".to_owned(), "/b c.pdf".to_owned()];
        assert!(send(port, "wrong", &lines).is_err());
        send(port, "secret", &lines).unwrap();
        send(port, "secret", &[]).unwrap();
        let got = server.join().unwrap();
        assert_eq!(got[0], None);
        assert_eq!(
            got[1],
            Some(vec![PathBuf::from("C:\\a.pdf"), PathBuf::from("/b c.pdf")])
        );
        assert_eq!(got[2], Some(Vec::new()));
    }

    #[test]
    fn secrets_compare() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"abcd"));
    }
}
