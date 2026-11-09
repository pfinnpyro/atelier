//! Updates from GitHub releases. The release workflow
//! (.github/workflows/release.yml) publishes each version with fixed asset
//! names and a SHA256SUMS.txt; this finds the newest release on the chosen
//! channel, downloads it with the system's curl, checks it against the
//! sums, and puts it in place: by running the installer for an installed
//! copy, or by swapping the program file for a portable one.

mod json;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use json::Value;

/// Where releases come from.
pub const REPO: &str = "pfinnpyro/atelier";
/// The program for a portable copy, the installer, and the sums.
const EXE_ASSET: &str = "atelier-windows-x64.exe";
const SETUP_ASSET: &str = "atelier-setup-windows-x64.exe";
const SUMS_ASSET: &str = "SHA256SUMS.txt";

/// This program's version.
pub fn current() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or_default()
}

/// A version such as 0.2.0 or 0.2.0-beta.1.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Version {
    pub nums: [u64; 3],
    /// Empty for a stable release.
    pub pre: String,
}

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let t = text.trim().trim_start_matches('v');
        let (main, pre) = t.split_once('-').unwrap_or((t, ""));
        let mut nums = [0u64; 3];
        let mut parts = main.split('.');
        for n in &mut nums {
            *n = parts.next()?.parse().ok()?;
        }
        if parts.next().is_some() {
            return None;
        }
        Some(Version {
            nums,
            pre: pre.to_string(),
        })
    }

    pub fn is_preview(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [a, b, c] = self.nums;
        write!(f, "{a}.{b}.{c}")?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre)?;
        }
        Ok(())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    /// Semantic version order: a pre-release comes before its release,
    /// and pre-release parts compare as numbers where both are numbers.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering::*;
        match self.nums.cmp(&other.nums) {
            Equal => {}
            o => return o,
        }
        match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => return Equal,
            (true, false) => return Greater,
            (false, true) => return Less,
            _ => {}
        }
        let mut a = self.pre.split('.');
        let mut b = other.pre.split('.');
        loop {
            match (a.next(), b.next()) {
                (None, None) => return Equal,
                (None, Some(_)) => return Less,
                (Some(_), None) => return Greater,
                (Some(x), Some(y)) => {
                    let o = match (x.parse::<u64>(), y.parse::<u64>()) {
                        (Ok(x), Ok(y)) => x.cmp(&y),
                        (Ok(_), Err(_)) => Less,
                        (Err(_), Ok(_)) => Greater,
                        _ => x.cmp(y),
                    };
                    if o != Equal {
                        return o;
                    }
                }
            }
        }
    }
}

/// A published release.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: Version,
    pub notes: String,
    /// The release's web page.
    pub page: String,
    /// Asset names, download links and sizes.
    pub assets: Vec<(String, String, u64)>,
}

impl Release {
    pub(crate) fn asset(&self, name: &str) -> Option<&(String, String, u64)> {
        self.assets.iter().find(|a| a.0 == name)
    }
}

/// The newest release on the channel (stable, or with `preview` also
/// pre-releases) that is newer than `than`.
pub fn newest(releases: &[Release], preview: bool, than: &Version) -> Option<Release> {
    releases
        .iter()
        .filter(|r| preview || !r.version.is_preview())
        .filter(|r| r.version > *than)
        .max_by(|a, b| a.version.cmp(&b.version))
        .cloned()
}

/// Linux releases must carry the matching architecture package and checksum manifest.
pub fn newest_for_platform(releases: &[Release], preview: bool, than: &Version) -> Option<Release> {
    if cfg!(target_os = "linux") {
        let name = linux_asset_name()?;
        let compatible: Vec<_> = releases
            .iter()
            .filter(|release| release.asset(name).is_some() && release.asset(SUMS_ASSET).is_some())
            .cloned()
            .collect();
        newest(&compatible, preview, than)
    } else {
        newest(releases, preview, than)
    }
}

/// Reads GitHub's release list.
pub fn parse_releases(text: &str) -> Result<Vec<Release>, String> {
    let v = json::parse(text)?;
    Ok(v.array()
        .iter()
        .filter(|r| r.get("draft").and_then(Value::bool) != Some(true))
        .filter_map(|r| {
            let version = Version::parse(r.get("tag_name")?.str()?)?;
            let assets = r
                .get("assets")?
                .array()
                .iter()
                .filter_map(|a| {
                    Some((
                        a.get("name")?.str()?.to_string(),
                        a.get("browser_download_url")?.str()?.to_string(),
                        a.get("size").and_then(Value::num).unwrap_or(0.0) as u64,
                    ))
                })
                .collect();
            Some(Release {
                version,
                notes: r
                    .get("body")
                    .and_then(Value::str)
                    .unwrap_or_default()
                    .to_string(),
                page: r
                    .get("html_url")
                    .and_then(Value::str)
                    .unwrap_or_default()
                    .to_string(),
                assets,
            })
        })
        .collect())
}

/// Asks GitHub for the releases.
pub fn fetch_releases() -> Result<Vec<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=30");
    let text = fetch_text(&url)?;
    parse_releases(&text)
}

/// How an update was put in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installed {
    /// The program file was replaced; restarting runs the new version.
    Swapped,
    /// The installer is running and starts the new version when done; this
    /// copy should quit now.
    InstallerRunning,
}

/// Whether this copy can update itself here.
pub fn supported() -> bool {
    cfg!(windows) || linux_managed_root().is_some()
}

/// Downloads and puts `release` in place, counting bytes in `progress`.
pub fn install(release: &Release, progress: &AtomicU64) -> Result<Installed, String> {
    if let Some(root) = linux_managed_root() {
        let name = linux_asset_name().ok_or("Unsupported Linux architecture")?;
        let asset = release
            .asset(name)
            .ok_or("This release has no package for this Linux architecture")?;
        let output = Command::new("python3")
            .arg(root.join("current/install.py"))
            .arg("--root")
            .arg(&root)
            .arg("--update")
            .arg(release.version.to_string())
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("Could not start the Linux updater: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "Linux update failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        progress.store(asset.2, Ordering::SeqCst);
        return Ok(Installed::Swapped);
    }
    if !supported() {
        return Err("Updating from inside Atelier works on Windows only.".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("Atelier's folder is unknown.")?;
    let sums = release
        .asset(SUMS_ASSET)
        .ok_or("The release has no checksums, so it was not installed.")?;
    let sums = fetch_text(&sums.1)?;
    let installed = dir.join("unins000.exe").exists();
    let asset_name = if installed { SETUP_ASSET } else { EXE_ASSET };
    let asset = release
        .asset(asset_name)
        .ok_or_else(|| format!("The release has no {asset_name}."))?;
    let want = sums
        .lines()
        .filter_map(|l| l.split_once(char::is_whitespace))
        .find(|(_, name)| name.trim().trim_start_matches('*') == asset_name)
        .map(|(hash, _)| hash.trim().to_ascii_lowercase())
        .ok_or_else(|| format!("The checksums do not list {asset_name}."))?;
    let target = if installed {
        std::env::temp_dir().join(format!("atelier-setup-{}.exe", release.version))
    } else {
        dir.join("atelier-viewer.exe.new")
    };
    let _ = fs::remove_file(&target);
    fetch_file(&asset.1, &target, progress)?;
    let data = fs::read(&target).map_err(|e| e.to_string())?;
    if hex(&atelier_crypto::sha256(&data)) != want {
        let _ = fs::remove_file(&target);
        return Err("The download did not match its checksum, so it was thrown away.".into());
    }
    if installed {
        // The installer closes this copy if needed and starts the new one.
        Command::new(&target)
            .args([
                "/VERYSILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART",
                "/update=1",
            ])
            .spawn()
            .map_err(|e| format!("Could not start the installer: {e}"))?;
        return Ok(Installed::InstallerRunning);
    }
    // A running program can be renamed on Windows, though not replaced.
    let old = old_path(&exe);
    let _ = fs::remove_file(&old);
    fs::rename(&exe, &old).map_err(|e| format!("Could not set the old version aside: {e}"))?;
    if let Err(e) = fs::rename(&target, &exe) {
        let _ = fs::rename(&old, &exe);
        return Err(format!("Could not put the new version in place: {e}"));
    }
    Ok(Installed::Swapped)
}

/// Managed Linux installs keep complete versions and atomically switch `current`.
pub fn linux_managed_root() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let releases = exe.parent()?.parent()?;
    if releases.file_name()? != "releases" {
        return None;
    }
    let root = releases.parent()?;
    if root.join("managed.json").is_file() && root.join("current/install.py").is_file() {
        Some(root.to_path_buf())
    } else {
        None
    }
}

pub fn linux_asset_name() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Some("atelier-linux-x64.tar.gz"),
        "aarch64" => Some("atelier-linux-arm64.tar.gz"),
        _ => None,
    }
}

fn old_path(exe: &Path) -> PathBuf {
    exe.with_extension("exe.old")
}

/// Removes the version an update set aside, once it is no longer running.
pub fn clean_up() {
    if !cfg!(windows) {
        return;
    }
    if let Ok(exe) = std::env::current_exe() {
        let _ = fs::remove_file(old_path(&exe));
    }
}

/// Starts the program again, opening `file`, so this copy can quit.
pub fn restart(file: Option<&Path>) -> Result<(), String> {
    let exe = linux_managed_root()
        .map(|root| root.join("current/atelier-viewer"))
        .map(Ok)
        .unwrap_or_else(std::env::current_exe)
        .map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    if let Some(f) = file {
        cmd.arg(f);
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Opens a web page in the browser.
pub fn open_page(url: &str) {
    if !url.starts_with("https://") {
        return;
    }
    let _ = if cfg!(windows) {
        Command::new("explorer.exe").arg(url).spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn()
    } else {
        Command::new("xdg-open").arg(url).spawn()
    };
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn curl() -> Command {
    let mut cmd = Command::new("curl");
    cmd.args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--proto", "=https", "--proto-redir", "=https"])
        .args(["--max-time", "900"])
        .args([
            "--user-agent",
            concat!("Atelier/", env!("CARGO_PKG_VERSION")),
        ])
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashes up.
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

fn fetch_text(url: &str) -> Result<String, String> {
    let out = curl()
        .args(["--header", "Accept: application/vnd.github+json"])
        .arg(url)
        .output()
        .map_err(|e| format!("Could not start curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "Could not reach GitHub: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    String::from_utf8(out.stdout).map_err(|_| "GitHub sent text that is not UTF-8.".into())
}

fn fetch_file(url: &str, to: &Path, progress: &AtomicU64) -> Result<(), String> {
    let mut child = curl()
        .arg("--output")
        .arg(to)
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start curl: {e}"))?;
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        progress.store(fs::metadata(to).map_or(0, |m| m.len()), Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(150));
    };
    progress.store(fs::metadata(to).map_or(0, |m| m.len()), Ordering::SeqCst);
    if !status.success() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        let _ = fs::remove_file(to);
        return Err(format!("The download failed: {}", err.trim()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_discovery_requires_matching_package_and_manifest() {
        let name = linux_asset_name().unwrap();
        let release = |version, assets| Release {
            version: v(version),
            notes: String::new(),
            page: String::new(),
            assets,
        };
        let packages = vec![
            release("2.0.0", vec![]),
            release(
                "1.5.0",
                vec![
                    (name.into(), "https://example/package".into(), 1),
                    (SUMS_ASSET.into(), "https://example/sums".into(), 1),
                ],
            ),
            release(
                "1.8.0",
                vec![(name.into(), "https://example/package".into(), 1)],
            ),
        ];
        assert_eq!(
            newest_for_platform(&packages, false, &v("1.0.0"))
                .unwrap()
                .version,
            v("1.5.0")
        );
        assert!(newest_for_platform(&packages, false, &v("1.5.0")).is_none());
    }

    #[test]
    fn orders_versions() {
        assert!(v("0.2.0") > v("0.1.9"));
        assert!(v("v1.0.0") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-beta.2") > v("1.0.0-beta.1"));
        assert!(v("1.0.0-beta.10") > v("1.0.0-beta.9"));
        assert!(v("1.0.0-rc.1") > v("1.0.0-beta.9"));
        assert!(Version::parse("1.0").is_none());
        assert_eq!(v("v0.2.0-beta.1").to_string(), "0.2.0-beta.1");
    }

    #[test]
    fn picks_the_newest_on_a_channel() {
        let text = r#"[
          {"tag_name":"v0.3.0-beta.1","prerelease":true,"draft":false,"body":"b","html_url":"https://x/3b",
           "assets":[{"name":"SHA256SUMS.txt","browser_download_url":"https://x/s","size":10}]},
          {"tag_name":"v0.2.0","prerelease":false,"draft":false,"body":"Two","html_url":"https://x/2",
           "assets":[{"name":"atelier-windows-x64.exe","browser_download_url":"https://x/e","size":5000}]},
          {"tag_name":"v0.4.0","draft":true,"assets":[]},
          {"tag_name":"nightly","assets":[]}
        ]"#;
        let rs = parse_releases(text).unwrap();
        assert_eq!(rs.len(), 2);
        let stable = newest(&rs, false, &v("0.1.0")).unwrap();
        assert_eq!(
            (stable.version.to_string(), stable.notes.as_str()),
            ("0.2.0".into(), "Two")
        );
        assert_eq!(stable.assets[0].2, 5000);
        assert_eq!(
            newest(&rs, true, &v("0.1.0")).unwrap().version,
            v("0.3.0-beta.1")
        );
        assert!(newest(&rs, false, &v("0.2.0")).is_none());
    }
}
