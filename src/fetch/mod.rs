//! Hash-verified downloads of upstream components.
//!
//! Two postures, per spec §11.4:
//!
//! - **NullMoth** — always the latest release, hash-verified against the
//!   `SHA256SUMS.txt` the release itself publishes. A missing sums asset or
//!   any mismatching line is a hard abort (Review Focus 4): an unverifiable
//!   payload is never installed.
//! - **AMFIPass** — pinned tag 1.4.1. Upstream publishes no checksums, so
//!   the returned [`VerifiedDownload::sha256`] is computed for the record
//!   but has nothing to be compared against.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// GitHub API endpoint for the latest NullMoth release.
const NULLMOTH_LATEST_URL: &str =
    "https://api.github.com/repos/nullmoth/nvidia-macos-driver/releases/latest";

/// GitHub API endpoint for the pinned AMFIPass release.
const AMFIPASS_TAG_URL: &str =
    "https://api.github.com/repos/acidanthera/AMFIPass/releases/tags/1.4.1";

/// Errors produced while downloading or verifying upstream components.
#[derive(Debug, Error)]
pub enum Error {
    #[error("GET {url}: {detail}")]
    Http { url: String, detail: String },

    #[error("failed to parse release JSON from {url}: {source}")]
    ReleaseJson {
        url: String,
        source: serde_json::Error,
    },

    #[error("release at {url} has no asset matching {pattern}")]
    MissingAsset { url: String, pattern: String },

    #[error("release at {url} has no SHA256SUMS.txt asset — a NullMoth release without published sums is never installed")]
    MissingSums { url: String },

    #[error(
        "SHA256SUMS.txt from {url} does not list {name} — the payload is unverifiable, aborting"
    )]
    NotInSums { url: String, name: String },

    #[error("asset {name} from {url} is not a plain file name — refusing to write it")]
    BadAssetName { name: String, url: String },

    #[error("SHA256SUMS.txt is not valid UTF-8: {source}")]
    SumsNotUtf8 { source: std::string::FromUtf8Error },

    #[error("sums file line {line_no} is malformed: {line:?} — expected `<64 hex>  <filename>`")]
    MalformedSumsLine { line_no: usize, line: String },

    #[error("{name}: listed in the sums file but not present in {}", .dir.display())]
    MissingSummedFile { name: String, dir: PathBuf },

    #[error("checksum mismatch for {name}: sums file says {expected}, downloaded file is {actual} — refusing to install a tampered or corrupt download")]
    ChecksumMismatch {
        name: String,
        expected: String,
        actual: String,
    },

    #[error("failed to read {}: {source}", .path.display())]
    Read { path: PathBuf, source: io::Error },

    #[error("failed to write {}: {source}", .path.display())]
    Write { path: PathBuf, source: io::Error },
}

/// A downloaded file whose integrity has been established: hash-verified
/// against upstream's published sums (NullMoth), or hash-recorded with
/// nothing to compare against (AMFIPass — see the module docs).
#[derive(Debug)]
pub struct VerifiedDownload {
    pub path: PathBuf,
    pub sha256: String,
    pub version: String,
}

/// The HTTP seam: production uses [`UreqHttp`]; tests use a canned fake.
/// `get` returns the full response body.
pub trait Http {
    fn get(&self, url: &str) -> Result<Vec<u8>, Error>;
}

/// The real [`Http`] over `ureq`, with a `User-Agent` (GitHub's API rejects
/// requests without one) and a 60 s timeout so a stalled connection cannot
/// hang the CLI. Never exercised in tests.
pub struct UreqHttp {
    agent: ureq::Agent,
}

impl UreqHttp {
    pub fn new() -> Self {
        UreqHttp {
            agent: ureq::AgentBuilder::new()
                .user_agent("greenlight/0.1")
                .timeout(Duration::from_secs(60))
                .build(),
        }
    }
}

impl Default for UreqHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl Http for UreqHttp {
    fn get(&self, url: &str) -> Result<Vec<u8>, Error> {
        let response = self.agent.get(url).call().map_err(|e| Error::Http {
            url: url.to_string(),
            detail: e.to_string(),
        })?;
        let mut body = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut body)
            .map_err(|e| Error::Http {
                url: url.to_string(),
                detail: e.to_string(),
            })?;
        Ok(body)
    }
}

/// Dev/test-only [`Http`] behind the `GREENLIGHT_FIXTURE_HTTP` env var (the
/// download counterpart of `GREENLIGHT_FIXTURE_HARDWARE`): serves canned
/// responses from a directory whose `manifest.json` maps each URL to a bare
/// file name in that directory. Keeps the fix-flow integration tests
/// offline. An unlisted URL is an error, so a surprise request fails loudly.
pub struct FixtureHttp {
    dir: PathBuf,
}

impl FixtureHttp {
    pub fn new(dir: PathBuf) -> Self {
        FixtureHttp { dir }
    }

    fn manifest(&self) -> Result<HashMap<String, String>, Error> {
        let path = self.dir.join("manifest.json");
        let bytes = fs::read(&path).map_err(|source| Error::Read {
            path: path.clone(),
            source,
        })?;
        serde_json::from_slice(&bytes).map_err(|source| Error::ReleaseJson {
            url: path.display().to_string(),
            source,
        })
    }
}

impl Http for FixtureHttp {
    fn get(&self, url: &str) -> Result<Vec<u8>, Error> {
        let name = self
            .manifest()?
            .get(url)
            .cloned()
            .ok_or_else(|| Error::Http {
                url: url.to_string(),
                detail: "fixture manifest has no canned response for this URL".to_string(),
            })?;
        // Manifest entries name files inside the fixture dir — nothing else.
        let rel = Path::new(&name);
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return Err(Error::BadAssetName {
                name,
                url: url.to_string(),
            });
        }
        let path = self.dir.join(rel);
        fs::read(&path).map_err(|source| Error::Read { path, source })
    }
}

/// Returns the lowercase hex SHA-256 of the file at `path`.
pub fn sha256_of(path: &Path) -> Result<String, Error> {
    let bytes = fs::read(path).map_err(|source| Error::Read {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

/// Verifies every file listed in `sums` — classic `<64 hex>  <filename>`
/// lines — against the contents of `dir`. The first mismatching or missing
/// file is an error naming that file. Files present in `dir` but absent
/// from `sums` are not checked: the sums list defines the payload.
pub fn verify_sha256sums(dir: &Path, sums: &str) -> Result<(), Error> {
    for (index, line) in sums.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let line_no = index + 1;
        let malformed = || Error::MalformedSumsLine {
            line_no,
            line: line.to_string(),
        };
        let Some((expected, name)) = line.split_once("  ") else {
            return Err(malformed());
        };
        if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(malformed());
        }
        let name = name.trim();
        // Sums entries must stay inside `dir`: bare names or subpaths, never
        // absolute or parent-traversing.
        let rel = Path::new(name);
        if name.is_empty()
            || rel.components().any(|c| {
                matches!(
                    c,
                    Component::Prefix(_) | Component::RootDir | Component::ParentDir
                )
            })
        {
            return Err(malformed());
        }

        let path = dir.join(rel);
        if !path.is_file() {
            return Err(Error::MissingSummedFile {
                name: name.to_string(),
                dir: dir.to_path_buf(),
            });
        }
        let actual = sha256_of(&path)?;
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(Error::ChecksumMismatch {
                name: name.to_string(),
                expected: expected.to_string(),
                actual,
            });
        }
    }
    Ok(())
}

/// Downloads the latest NullMoth release tarball into `dest` and verifies
/// it against the release's own `SHA256SUMS.txt`. Returns the tarball path,
/// its verified hash, and the tag-derived version (leading `v` stripped).
///
/// Hard aborts: sums asset missing, tarball asset missing, the tarball not
/// listed in the sums, or its sums line mismatching — the anti-tamper
/// posture of this module.
pub fn fetch_nullmoth_latest(http: &dyn Http, dest: &Path) -> Result<VerifiedDownload, Error> {
    fetch_nullmoth_asset(http, dest, "nullmoth-nvidia-*.tar.gz", |name| {
        name.starts_with("nullmoth-nvidia-") && name.ends_with(".tar.gz")
    })
}

/// Downloads the 1401 DMG asset of the latest NullMoth release into `dest`,
/// verified against its own SHA256SUMS.txt line with the same hard-abort
/// posture as the tarball (the sums file does list the dmg). The DMG carries
/// `1401.app`, which ships `NullMothSafe.efi` — the picker recovery entry
/// (spec §11.3).
pub fn fetch_nullmoth_safe_dmg(http: &dyn Http, dest: &Path) -> Result<VerifiedDownload, Error> {
    fetch_nullmoth_asset(http, dest, "1401 DMG (*.dmg)", |name| {
        name.contains("1401") && name.ends_with(".dmg")
    })
}

/// Shared core of the NullMoth fetches: GET the latest release, pick the
/// first asset matching `pred`, download it into `dest`, and verify it
/// against its own line(s) in the release's SHA256SUMS.txt.
fn fetch_nullmoth_asset(
    http: &dyn Http,
    dest: &Path,
    pattern: &str,
    pred: impl Fn(&str) -> bool,
) -> Result<VerifiedDownload, Error> {
    let release = get_release(http, NULLMOTH_LATEST_URL)?;
    let version = release.tag_name.trim_start_matches('v').to_string();

    let asset = release.asset_matching(NULLMOTH_LATEST_URL, pattern, pred)?;
    let sums_asset = release
        .assets
        .iter()
        .find(|a| a.name == "SHA256SUMS.txt")
        .ok_or_else(|| Error::MissingSums {
            url: NULLMOTH_LATEST_URL.to_string(),
        })?;

    let path = download_asset(http, asset, NULLMOTH_LATEST_URL, dest)?;
    let sums_bytes = http.get(&sums_asset.browser_download_url)?;
    let sums = String::from_utf8(sums_bytes).map_err(|source| Error::SumsNotUtf8 { source })?;

    // The upstream sums list every release asset (tarball, zip, dmg) but
    // each fetch downloads only one — verify just its own line(s).
    // Full-file strictness stays in `verify_sha256sums` for other callers.
    let mut asset_sums = String::new();
    for line in sums.lines() {
        let is_asset_line = line
            .split_once("  ")
            .is_some_and(|(_, name)| name.trim() == asset.name);
        if is_asset_line {
            asset_sums.push_str(line);
            asset_sums.push('\n');
        }
    }
    if asset_sums.is_empty() {
        return Err(Error::NotInSums {
            url: NULLMOTH_LATEST_URL.to_string(),
            name: asset.name.clone(),
        });
    }
    verify_sha256sums(dest, &asset_sums)?;

    let sha256 = sha256_of(&path)?;
    Ok(VerifiedDownload {
        path,
        sha256,
        version,
    })
}

/// Downloads the pinned AMFIPass 1.4.1 RELEASE zip into `dest`. The asset
/// is matched by `RELEASE` + `.zip` rather than an exact name, to survive
/// upstream naming drift between DEBUG/RELEASE builds. AMFIPass publishes
/// no checksums, so the returned hash is informational only.
pub fn fetch_amfipass(http: &dyn Http, dest: &Path) -> Result<VerifiedDownload, Error> {
    let release = get_release(http, AMFIPASS_TAG_URL)?;
    let version = release.tag_name.trim_start_matches('v').to_string();
    let zip = release.asset_matching(AMFIPASS_TAG_URL, "RELEASE zip (*.zip)", |name| {
        name.contains("RELEASE") && name.ends_with(".zip")
    })?;

    let path = download_asset(http, zip, AMFIPASS_TAG_URL, dest)?;
    let sha256 = sha256_of(&path)?;
    Ok(VerifiedDownload {
        path,
        sha256,
        version,
    })
}

/// The subset of the GitHub release JSON this module reads.
#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

impl Release {
    /// The first asset whose name satisfies `pred`; `pattern` is the
    /// human-readable shape used in the error when nothing matches.
    fn asset_matching(
        &self,
        url: &str,
        pattern: &str,
        pred: impl Fn(&str) -> bool,
    ) -> Result<&Asset, Error> {
        self.assets
            .iter()
            .find(|a| pred(&a.name))
            .ok_or_else(|| Error::MissingAsset {
                url: url.to_string(),
                pattern: pattern.to_string(),
            })
    }
}

/// GETs `url` and parses the release JSON.
fn get_release(http: &dyn Http, url: &str) -> Result<Release, Error> {
    let body = http.get(url)?;
    serde_json::from_slice(&body).map_err(|source| Error::ReleaseJson {
        url: url.to_string(),
        source,
    })
}

/// Downloads `asset` into `dest` under its bare file name and returns the
/// path written.
fn download_asset(
    http: &dyn Http,
    asset: &Asset,
    release_url: &str,
    dest: &Path,
) -> Result<PathBuf, Error> {
    let file_name = Path::new(&asset.name)
        .file_name()
        .ok_or_else(|| Error::BadAssetName {
            name: asset.name.clone(),
            url: release_url.to_string(),
        })?;
    let bytes = http.get(&asset.browser_download_url)?;
    let path = dest.join(file_name);
    fs::write(&path, &bytes).map_err(|source| Error::Write {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}
