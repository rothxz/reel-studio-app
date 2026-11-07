//! App updates: new versions come from the GitHub releases of the public
//! repo (`REEL_UPDATE_REPO`), tagged `native-v<version>` by the release
//! workflow (.github/workflows/release.yml). Each one carries an
//! `update.json` naming, per platform, a zipped app with its SHA-256.
//!
//! Like the old desktop shell: it checks on launch and every 4 h, never
//! downloads by itself (Settings → App updates → Download), then asks to
//! restart; "Later" installs when the app quits. Installing swaps the
//! downloaded bundle in for the running one (same folder), so it needs an
//! app bundle in a folder the user can write to: not a `cargo run`, nor
//! an app macOS runs from a quarantine copy (open it from Applications).
//!
//! The download must match the manifest's hash, be a signed bundle with
//! our bundle id and the promised version, and, when this app is signed
//! by a team (Developer ID), be signed by the same team.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context as _, Result, bail, ensure};
use gpui::{App, AppContext as _, Context, Entity, SharedString};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// This build's version (the release workflow stamps it).
pub const VERSION: &str = env!("REEL_VERSION");
/// The public repo whose releases are the update feed.
const REPO: &str = env!("REEL_UPDATE_REPO");
/// Release tags of this app (the same repo also holds the old shell's
/// `v…` releases, which this app ignores).
pub const TAG_PREFIX: &str = "native-v";
/// The asset describing a release's downloads.
pub const MANIFEST: &str = "update.json";
const FIRST_CHECK: Duration = Duration::from_secs(5);
const CHECK_EVERY: Duration = Duration::from_secs(4 * 60 * 60);

/// Where the updater is, as Settings shows it.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Checking,
    UpToDate,
    /// A newer version (`Updater::version`) can be downloaded.
    Available,
    /// Downloading it, percent done.
    Downloading(f32),
    /// Downloaded and checked: restart (or quit) to install it.
    Downloaded,
    /// Swapped in; the next launch runs it.
    Installed,
    Failed,
    /// No updates for this copy (not an installed app).
    Unsupported,
}

pub struct Updater {
    pub status: Status,
    /// The newer version found (kept through a failed download, so "Try
    /// again" retries the download).
    pub version: Option<SharedString>,
    found: Option<Found>,
    /// The checked new bundle, waiting to be swapped in.
    staged: Option<PathBuf>,
    /// The running app bundle, when updates can replace it.
    target: Option<PathBuf>,
}

#[derive(Clone, Debug)]
struct Found {
    version: String,
    file: Asset,
}

/// One platform's download in `update.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct Asset {
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
}

/// `update.json`, written by the release workflow.
#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub minimum_system_version: Option<String>,
    pub platforms: std::collections::HashMap<String, Asset>,
}

/// The fields of a GitHub release the check reads.
#[derive(Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
}

impl Updater {
    /// The app's updater; checks start on their own when updates are
    /// possible for this copy.
    pub fn new(cx: &mut App) -> Entity<Updater> {
        let target = match install_target(cx.app_path().ok()) {
            Ok(path) => Some(path),
            Err(why) => {
                eprintln!("updates off: {why}");
                None
            }
        };
        cx.new(|cx| {
            if target.is_some() {
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(FIRST_CHECK).await;
                    loop {
                        if this.update(cx, |u: &mut Updater, cx| u.check(cx)).is_err() {
                            break;
                        }
                        cx.background_executor().timer(CHECK_EVERY).await;
                    }
                })
                .detach();
                // "Later": the download is installed when the app quits.
                cx.on_app_quit(|u: &mut Updater, _| {
                    u.install_staged();
                    async {}
                })
                .detach();
            }
            Updater {
                status: if target.is_some() {
                    Status::Idle
                } else {
                    Status::Unsupported
                },
                version: None,
                found: None,
                staged: None,
                target,
            }
        })
    }

    /// Looks for a newer release (not while one is found or fetched:
    /// checking again would reset the button).
    pub fn check(&mut self, cx: &mut Context<Self>) {
        if self.target.is_none()
            || matches!(
                self.status,
                Status::Checking | Status::Downloading(_) | Status::Downloaded | Status::Installed
            )
        {
            return;
        }
        self.status = Status::Checking;
        cx.notify();
        in_thread(cx, find_update, |u, result, cx| {
            match result {
                Ok(Some(found)) => {
                    u.version = Some(found.version.clone().into());
                    u.found = Some(found);
                    u.status = Status::Available;
                }
                Ok(None) => u.status = Status::UpToDate,
                Err(e) => {
                    eprintln!("update check failed: {e:#}");
                    u.status = Status::Failed;
                }
            }
            cx.notify();
        });
    }

    /// Downloads the found version and checks it.
    pub fn download(&mut self, cx: &mut Context<Self>) {
        let Some(found) = self.found.clone() else {
            return;
        };
        if !matches!(self.status, Status::Available | Status::Failed) {
            return;
        }
        self.status = Status::Downloading(0.);
        cx.notify();
        let (tx, rx) = async_channel::unbounded::<Result<f32, Result<PathBuf>>>();
        std::thread::spawn(move || {
            let progress = |p: f32| {
                let _ = tx.send_blocking(Ok(p));
            };
            let result = fetch(&found, &progress);
            let _ = tx.send_blocking(Err(result));
        });
        cx.spawn(async move |this, cx| {
            while let Ok(msg) = rx.recv().await {
                let done = msg.is_err();
                let applied = this.update(cx, |u, cx| {
                    match msg {
                        Ok(percent) => u.status = Status::Downloading(percent),
                        Err(Ok(app)) => {
                            u.staged = Some(app);
                            u.status = Status::Downloaded;
                        }
                        Err(Err(e)) => {
                            eprintln!("update download failed: {e:#}");
                            u.status = Status::Failed;
                        }
                    }
                    cx.notify();
                });
                if done || applied.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Swaps the downloaded version in and relaunches. The caller saves
    /// the open project first.
    pub fn restart(&mut self, cx: &mut Context<Self>) {
        if self.status != Status::Downloaded {
            return;
        }
        if self.install_staged() {
            cx.restart();
        } else {
            cx.notify();
        }
    }

    /// Swaps a downloaded bundle in for the running one. False (and
    /// Failed) when that went wrong; the old app is then still in place.
    fn install_staged(&mut self) -> bool {
        let (Some(app), Some(target)) = (self.staged.take(), self.target.clone()) else {
            return false;
        };
        match install(&app, &target) {
            Ok(()) => {
                self.status = Status::Installed;
                if let Some(dir) = app.parent().and_then(Path::parent) {
                    let _ = std::fs::remove_dir_all(dir);
                }
                true
            }
            Err(e) => {
                eprintln!("installing the update failed: {e:#}");
                self.status = Status::Failed;
                false
            }
        }
    }
}

/// Runs `work` off the UI thread, then `done` with its result.
fn in_thread<T: Send + 'static>(
    cx: &mut Context<Updater>,
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(&mut Updater, T, &mut Context<Updater>) + 'static,
) {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(work());
    });
    cx.spawn(async move |this, cx| {
        if let Ok(value) = rx.recv().await {
            this.update(cx, |u, cx| done(u, value, cx)).ok();
        }
    })
    .detach();
}

// ---- finding a release ----------------------------------------------------

/// `major.minor.patch` (anything after a `-` or `+` is ignored).
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let version = (
        parts.next()??,
        parts.next()??,
        parts.next().unwrap_or(Some(0))?,
    );
    parts.next().is_none().then_some(version)
}

/// The key of this machine in `update.json` (`darwin-aarch64`, …).
pub fn platform_key() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

/// The newest of our releases (by version), with its manifest's URL.
pub fn newest_release(releases: &[Release]) -> Option<(String, String)> {
    releases
        .iter()
        .filter(|r| !r.draft)
        .filter_map(|r| {
            let version = r.tag_name.strip_prefix(TAG_PREFIX)?;
            let parsed = parse_version(version)?;
            let manifest = r.assets.iter().find(|a| a.name == MANIFEST)?;
            Some((
                parsed,
                version.to_string(),
                manifest.browser_download_url.clone(),
            ))
        })
        .max_by_key(|(parsed, ..)| *parsed)
        .map(|(_, version, url)| (version, url))
}

/// This machine's download from a manifest, if it is newer than
/// `current` and this macOS can run it.
pub fn pick(
    manifest: &Manifest,
    tag_version: &str,
    current: &str,
    os_version: Option<&str>,
) -> Result<Option<Asset>> {
    ensure!(
        parse_version(&manifest.version) == parse_version(tag_version),
        "{MANIFEST} says {} in release {tag_version}",
        manifest.version
    );
    let (Some(new), Some(now)) = (parse_version(&manifest.version), parse_version(current)) else {
        bail!("unreadable version {} / {current}", manifest.version);
    };
    if new <= now {
        return Ok(None);
    }
    if let (Some(min), Some(os)) = (
        manifest
            .minimum_system_version
            .as_deref()
            .and_then(parse_version),
        os_version.and_then(parse_version),
    ) && os < min
    {
        return Ok(None);
    }
    Ok(manifest.platforms.get(&platform_key()).cloned())
}

/// The release list: GitHub's API, or REEL_UPDATE_FEED (a URL serving the
/// same JSON, for trying the updater against a local server).
fn feed_url() -> String {
    std::env::var("REEL_UPDATE_FEED")
        .ok()
        .filter(|s| !s.is_empty())
        // The repo also holds the old shell's releases: a full page (the
        // API's most) keeps ours in reach between them.
        .unwrap_or_else(|| format!("https://api.github.com/repos/{REPO}/releases?per_page=100"))
}

fn find_update() -> Result<Option<Found>> {
    let agent = crate::assets::agent();
    let releases: Vec<Release> = serde_json::from_str(
        &agent
            .get(&feed_url())
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", &format!("ReelStudio/{VERSION}"))
            .call()?
            .body_mut()
            .read_to_string()?,
    )?;
    let Some((version, manifest_url)) = newest_release(&releases) else {
        return Ok(None);
    };
    if parse_version(&version) <= parse_version(VERSION) {
        return Ok(None);
    }
    let manifest: Manifest = serde_json::from_str(
        &agent
            .get(&manifest_url)
            .header("User-Agent", &format!("ReelStudio/{VERSION}"))
            .call()?
            .body_mut()
            .read_to_string()?,
    )?;
    let os = macos_version();
    Ok(pick(&manifest, &version, VERSION, os.as_deref())?.map(|file| Found { version, file }))
}

fn macos_version() -> Option<String> {
    let out = Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    Some(String::from_utf8(out.stdout).ok()?.trim().to_string())
}

// ---- downloading ----------------------------------------------------------

/// Downloads the zip, checks its hash, unpacks it and checks the bundle.
/// Returns the new app bundle (inside the data folder's `updates/`).
fn fetch(found: &Found, progress: &dyn Fn(f32)) -> Result<PathBuf> {
    let dir = crate::prefs::data_dir()
        .join("updates")
        .join(&found.version);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let zip = dir.join("update.zip");

    let mut response = crate::assets::download_agent()
        .get(&found.file.url)
        .header("User-Agent", &format!("ReelStudio/{VERSION}"))
        .call()?;
    let total = match found.file.size {
        0 => response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok()?.parse().ok())
            .unwrap_or(0),
        size => size,
    };
    let mut reader = response.body_mut().with_config().limit(2 << 30).reader();
    let mut file = std::fs::File::create(&zip)?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let (mut done, mut shown) = (0u64, -1.);
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        file.write_all(&buf[..n])?;
        done += n as u64;
        if total > 0 {
            let percent = (done as f32 / total as f32 * 100.).min(100.).floor();
            if percent > shown {
                shown = percent;
                progress(percent);
            }
        }
    }
    file.sync_all()?;
    drop(file);
    let digest = hex(&hash.finalize());
    ensure!(
        digest.eq_ignore_ascii_case(found.file.sha256.trim()),
        "the download's SHA-256 is {digest}, the release says {}",
        found.file.sha256
    );

    let unpacked = dir.join("app");
    run(Command::new("ditto")
        .arg("-x")
        .arg("-k")
        .arg(&zip)
        .arg(&unpacked))?;
    let _ = std::fs::remove_file(&zip);
    let mut apps = std::fs::read_dir(&unpacked)?
        .filter_map(|e| Some(e.ok()?.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "app"));
    let app = apps.next().context("no app in the download")?;
    ensure!(apps.next().is_none(), "more than one app in the download");
    verify_bundle(&app, &found.version)?;
    Ok(app)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn run(cmd: &mut Command) -> Result<String> {
    let out = cmd.output().with_context(|| format!("{cmd:?}"))?;
    ensure!(
        out.status.success(),
        "{cmd:?}: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A key of a bundle's Info.plist.
fn plist_value(app: &Path, key: &str) -> Result<String> {
    run(Command::new("plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(app.join("Contents/Info.plist")))
}

/// The Team ID a bundle is signed by (None: ad-hoc or unsigned).
fn team_id(app: &Path) -> Option<String> {
    let out = Command::new("codesign").arg("-dv").arg(app).output().ok()?;
    // codesign -d prints its details on stderr.
    let text = String::from_utf8_lossy(&out.stderr).into_owned();
    text.lines()
        .find_map(|l| l.strip_prefix("TeamIdentifier="))
        .map(str::trim)
        .filter(|t| !t.is_empty() && *t != "not set")
        .map(str::to_string)
}

/// The new bundle is ours (bundle id), the promised version, intact
/// (signature), and from the same team as this one when it has a team.
fn verify_bundle(app: &Path, version: &str) -> Result<()> {
    let current = std::env::current_exe()?
        .ancestors()
        .nth(3)
        .map(Path::to_path_buf)
        .context("not in a bundle")?;
    let id = plist_value(&current, "CFBundleIdentifier")?;
    ensure!(
        plist_value(app, "CFBundleIdentifier")? == id,
        "the download is another app"
    );
    let new_version = plist_value(app, "CFBundleShortVersionString")?;
    ensure!(
        new_version == version,
        "the download is version {new_version}, not {version}"
    );
    run(Command::new("codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(app))?;
    if let Some(team) = team_id(&current) {
        let requirement =
            format!("=anchor apple generic and certificate leaf[subject.OU] = \"{team}\"");
        run(Command::new("codesign")
            .arg("--verify")
            .arg(format!("-R{requirement}"))
            .arg(app))
        .context("the download isn't signed by this app's team")?;
    }
    Ok(())
}

// ---- installing -----------------------------------------------------------

/// The running bundle when updates can replace it: an `.app`, not a copy
/// macOS runs from a quarantine folder (App Translocation), in a folder
/// this user can write to.
pub fn install_target(app_path: Option<PathBuf>) -> Result<PathBuf, &'static str> {
    if !cfg!(target_os = "macos") {
        return Err("updates are macOS only for now");
    }
    let app = app_path
        .filter(|p| p.extension().is_some_and(|e| e == "app"))
        .ok_or("not running from an app bundle")?;
    if app
        .components()
        .any(|c| c.as_os_str() == "AppTranslocation")
    {
        return Err("running from a quarantine copy: move the app to Applications");
    }
    let parent = app.parent().ok_or("no folder")?;
    let probe = parent.join(format!(".reel-update-probe-{}", std::process::id()));
    std::fs::File::create(&probe).map_err(|_| "the app's folder isn't writable")?;
    let _ = std::fs::remove_file(&probe);
    Ok(app)
}

/// Puts `new_app` where `target` is: a copy next to it first (same
/// volume), then two renames, rolled back when the second fails.
pub fn install(new_app: &Path, target: &Path) -> Result<()> {
    let parent = target.parent().context("no folder")?;
    let name = target
        .file_name()
        .context("no name")?
        .to_string_lossy()
        .into_owned();
    let incoming = parent.join(format!(".{name}.incoming"));
    let outgoing = parent.join(format!(".{name}.outgoing"));
    for stale in [&incoming, &outgoing] {
        if stale.exists() {
            std::fs::remove_dir_all(stale)?;
        }
    }
    // ditto keeps the signature, symlinks and extended attributes.
    run(Command::new("ditto").arg(new_app).arg(&incoming))?;
    std::fs::rename(target, &outgoing).context("moving the old app aside")?;
    if let Err(e) = std::fs::rename(&incoming, target) {
        let _ = std::fs::rename(&outgoing, target);
        let _ = std::fs::remove_dir_all(&incoming);
        return Err(e).context("moving the new app in");
    }
    // The running binary stays readable until it exits.
    let _ = std::fs::remove_dir_all(&outgoing);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, draft: bool, manifest: bool) -> Release {
        Release {
            tag_name: tag.into(),
            draft,
            assets: if manifest {
                vec![ReleaseAsset {
                    name: MANIFEST.into(),
                    browser_download_url: format!("https://example.test/{tag}/{MANIFEST}"),
                }]
            } else {
                vec![]
            },
        }
    }

    #[test]
    fn versions_parse_and_order() {
        assert_eq!(parse_version("0.2.15"), Some((0, 2, 15)));
        assert_eq!(parse_version("v1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("1.2.3-beta.1"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.x.3"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert!(parse_version("0.2.10") > parse_version("0.2.9"));
    }

    #[test]
    fn newest_release_skips_other_tags_drafts_and_missing_manifests() {
        let releases = vec![
            // The old shell's releases share the repo.
            release("v9.9.9", false, true),
            release("native-v0.2.9", false, true),
            release("native-v0.2.12", true, true),
            release("native-v0.2.11", false, false),
            release("native-v0.2.10", false, true),
        ];
        let (version, url) = newest_release(&releases).unwrap();
        assert_eq!(version, "0.2.10");
        assert!(url.ends_with("native-v0.2.10/update.json"));
        assert!(newest_release(&[release("v1.0.0", false, true)]).is_none());
    }

    #[test]
    fn pick_wants_newer_matching_and_runnable() {
        let manifest: Manifest = serde_json::from_value(serde_json::json!({
            "version": "0.2.10",
            "minimum_system_version": "12.0",
            "platforms": {
                platform_key(): { "url": "https://example.test/a.zip", "sha256": "ab", "size": 3 }
            }
        }))
        .unwrap();
        let asset = pick(&manifest, "0.2.10", "0.2.9", Some("15.1"))
            .unwrap()
            .unwrap();
        assert_eq!(asset.url, "https://example.test/a.zip");
        // Not newer.
        assert!(pick(&manifest, "0.2.10", "0.2.10", None).unwrap().is_none());
        // This macOS is too old for it.
        assert!(
            pick(&manifest, "0.2.10", "0.2.9", Some("11.7"))
                .unwrap()
                .is_none()
        );
        // The manifest must describe its own release.
        assert!(pick(&manifest, "0.2.11", "0.2.9", None).is_err());
    }

    #[test]
    fn install_swaps_the_bundle_in_place() {
        let root = std::env::temp_dir().join(format!("reel-update-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let target = root.join("Applications/Reel Studio.app");
        let new_app = root.join("download/Reel Studio.app");
        for (app, v) in [(&target, "old"), (&new_app, "new")] {
            std::fs::create_dir_all(app.join("Contents")).unwrap();
            std::fs::write(app.join("Contents/v"), v).unwrap();
        }
        install(&new_app, &target).unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("Contents/v")).unwrap(),
            "new"
        );
        // Nothing left beside it.
        let left: Vec<_> = std::fs::read_dir(root.join("Applications"))
            .unwrap()
            .collect();
        assert_eq!(left.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_a_bundle_can_be_updated() {
        assert!(install_target(None).is_err());
        assert!(install_target(Some("/usr/local/bin".into())).is_err());
        assert!(
            install_target(Some(
                "/private/var/folders/x/AppTranslocation/ABC/d/Reel Studio.app".into()
            ))
            .is_err()
        );
    }
}
