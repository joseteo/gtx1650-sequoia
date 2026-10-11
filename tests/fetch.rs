//! Integration tests for `fetch` — hash-verified GitHub release downloads.
//!
//! `UreqHttp` is never exercised here (offline constraint): every test
//! drives the fetch functions through a `FakeHttp` serving canned bytes.

use std::fs;
use std::path::Path;

use greenlight::fetch::{self, Error, Http};
use sha2::{Digest, Sha256};

const NULLMOTH_LATEST_URL: &str =
    "https://api.github.com/repos/nullmoth/nvidia-macos-driver/releases/latest";
const AMFIPASS_TAG_URL: &str =
    "https://api.github.com/repos/acidanthera/AMFIPass/releases/tags/1.4.1";
const TARBALL_NAME: &str = "nullmoth-nvidia-1.10.0.tar.gz";
const TARBALL_URL: &str =
    "https://github.com/nullmoth/nvidia-macos-driver/releases/download/v1.10.0/nullmoth-nvidia-1.10.0.tar.gz";
const SUMS_URL: &str =
    "https://github.com/nullmoth/nvidia-macos-driver/releases/download/v1.10.0/SHA256SUMS.txt";
const TARBALL_BYTES: &[u8] = b"fake nullmoth tarball payload";

/// Fake `Http`: exact-URL lookup over canned responses. An unlisted URL is
/// an error, so a surprise request fails loudly instead of silently passing.
struct FakeHttp {
    responses: Vec<(String, Vec<u8>)>,
}

impl FakeHttp {
    fn new() -> Self {
        FakeHttp {
            responses: Vec::new(),
        }
    }

    fn with(mut self, url: &str, body: impl Into<Vec<u8>>) -> Self {
        self.responses.push((url.to_string(), body.into()));
        self
    }
}

impl Http for FakeHttp {
    fn get(&self, url: &str) -> Result<Vec<u8>, Error> {
        self.responses
            .iter()
            .find(|(u, _)| u == url)
            .map(|(_, body)| body.clone())
            .ok_or_else(|| Error::Http {
                url: url.to_string(),
                detail: "FakeHttp has no canned response for this URL".to_string(),
            })
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn fixtures_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Canned latest-release JSON: tag carries a leading `v` on purpose to pin
/// the normalization rule.
fn nullmoth_release_json() -> String {
    format!(
        r#"{{"tag_name":"v1.10.0","assets":[
            {{"name":"{TARBALL_NAME}","browser_download_url":"{TARBALL_URL}"}},
            {{"name":"SHA256SUMS.txt","browser_download_url":"{SUMS_URL}"}}
        ]}}"#
    )
}

/// Serves the canned release; the sums describe `sums_over`, which differs
/// from the served tarball in the tampered case.
fn nullmoth_http(tarball: &[u8], sums_over: &[u8]) -> FakeHttp {
    let sums = format!("{}  {TARBALL_NAME}\n", hex_sha256(sums_over));
    FakeHttp::new()
        .with(NULLMOTH_LATEST_URL, nullmoth_release_json())
        .with(TARBALL_URL, tarball)
        .with(SUMS_URL, sums)
}

/// Same, but the sums list three assets like the real NullMoth 1.10.0
/// release — the tarball plus the zip and dmg this fetch never downloads.
fn nullmoth_http_multi(tarball: &[u8], sums_over: &[u8]) -> FakeHttp {
    let sums = format!(
        "{}  {TARBALL_NAME}\n{}  1401-Mac-1.10.0.zip\n{}  1401-Mac-1.10.0.dmg\n",
        hex_sha256(sums_over),
        "0".repeat(64),
        "1".repeat(64),
    );
    FakeHttp::new()
        .with(NULLMOTH_LATEST_URL, nullmoth_release_json())
        .with(TARBALL_URL, tarball)
        .with(SUMS_URL, sums)
}

#[test]
fn sha256_of_known_content() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("abc.bin");
    fs::write(&file, b"abc").unwrap();

    let digest = fetch::sha256_of(&file).expect("sha256 of a readable file");
    assert_eq!(
        digest, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        "SHA-256 of \"abc\" is a published test vector"
    );
}

#[test]
fn verify_sums_good_fixture_ok() {
    let fixtures = fixtures_dir();
    let sums = fs::read_to_string(fixtures.join("SHA256SUMS-good.txt")).unwrap();
    fetch::verify_sha256sums(&fixtures, &sums).expect("good fixture sums must verify");
}

#[test]
fn verify_sums_bad_fixture_names_file() {
    let fixtures = fixtures_dir();
    let sums = fs::read_to_string(fixtures.join("SHA256SUMS-bad.txt")).unwrap();

    let err = fetch::verify_sha256sums(&fixtures, &sums).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("config-broken.plist"),
        "error must name the first offending file:\n{msg}"
    );
}

#[test]
fn fetch_nullmoth_latest_parses_release_and_verifies() {
    let dir = tempfile::tempdir().unwrap();
    let http = nullmoth_http(TARBALL_BYTES, TARBALL_BYTES);

    let got = fetch::fetch_nullmoth_latest(&http, dir.path())
        .expect("well-formed release with matching sums must fetch");

    assert_eq!(got.version, "1.10.0", "leading v must be stripped");
    assert_eq!(got.path, dir.path().join(TARBALL_NAME));
    assert_eq!(got.sha256, hex_sha256(TARBALL_BYTES));
    assert_eq!(
        fs::read(&got.path).unwrap(),
        TARBALL_BYTES,
        "the tarball bytes must land in dest"
    );

    // Anti-tamper ruling: tarball bytes that do not match the published
    // sums are a hard fail, and the error names the offending file.
    let dir2 = tempfile::tempdir().unwrap();
    let tampered = nullmoth_http(b"tampered payload", TARBALL_BYTES);
    let err = fetch::fetch_nullmoth_latest(&tampered, dir2.path()).unwrap_err();
    assert!(
        matches!(err, Error::ChecksumMismatch { .. }),
        "tampered tarball must be a ChecksumMismatch, got {err:?}"
    );
    assert!(
        err.to_string().contains(TARBALL_NAME),
        "error must name the offending file:\n{err}"
    );
}

#[test]
fn fetch_nullmoth_latest_multi_asset_sums_ignores_undownloaded() {
    // The real 1.10.0 sums list the tarball plus the 1401-Mac zip and dmg;
    // this fetch downloads only the tarball, so its own line alone is
    // verified and the undownloaded assets must not abort the fetch.
    let dir = tempfile::tempdir().unwrap();
    let http = nullmoth_http_multi(TARBALL_BYTES, TARBALL_BYTES);
    let got = fetch::fetch_nullmoth_latest(&http, dir.path())
        .expect("tarball line verifying is enough; undownloaded assets must be ignored");
    assert_eq!(got.version, "1.10.0");
    assert_eq!(got.sha256, hex_sha256(TARBALL_BYTES));

    // Tampered multi-line variant: the tarball's own line mismatches.
    let dir2 = tempfile::tempdir().unwrap();
    let tampered = nullmoth_http_multi(b"tampered payload", TARBALL_BYTES);
    let err = fetch::fetch_nullmoth_latest(&tampered, dir2.path()).unwrap_err();
    assert!(
        matches!(err, Error::ChecksumMismatch { .. }),
        "tampered tarball must be a ChecksumMismatch, got {err:?}"
    );
    assert!(
        err.to_string().contains(TARBALL_NAME),
        "error must name the offending file:\n{err}"
    );
}

#[test]
fn tarball_absent_from_sums_aborts() {
    // A sums file that exists but does not list the tarball leaves the
    // payload unverifiable — a hard abort, never a vacuous pass.
    let sums = format!(
        "{}  1401-Mac-1.10.0.zip\n{}  1401-Mac-1.10.0.dmg\n",
        "0".repeat(64),
        "1".repeat(64),
    );
    let http = FakeHttp::new()
        .with(NULLMOTH_LATEST_URL, nullmoth_release_json())
        .with(TARBALL_URL, TARBALL_BYTES)
        .with(SUMS_URL, sums);
    let dir = tempfile::tempdir().unwrap();

    let err = fetch::fetch_nullmoth_latest(&http, dir.path()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains(TARBALL_NAME),
        "error must name the unverifiable tarball:\n{msg}"
    );
    assert!(
        msg.contains("SHA256SUMS.txt"),
        "error must name the sums file:\n{msg}"
    );
}

#[test]
fn missing_sums_file_aborts() {
    // Review Focus 4: a release without SHA256SUMS.txt is a hard abort —
    // an unverifiable payload is never installed.
    let json = format!(
        r#"{{"tag_name":"v1.10.0","assets":[
            {{"name":"{TARBALL_NAME}","browser_download_url":"{TARBALL_URL}"}}
        ]}}"#
    );
    let http = FakeHttp::new().with(NULLMOTH_LATEST_URL, json);
    let dir = tempfile::tempdir().unwrap();

    let err = fetch::fetch_nullmoth_latest(&http, dir.path()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("SHA256SUMS.txt"),
        "error must name the missing sums asset:\n{msg}"
    );
}

#[test]
fn fetch_amfipass_returns_pinned_release_zip() {
    // AMFIPass is pinned to tag 1.4.1; the zip is matched by RELEASE + .zip
    // (not an exact name) to survive upstream naming drift. DEBUG is listed
    // first so the test proves the match is not just "first .zip".
    const ZIP_NAME: &str = "AMFIPass-1.4.1-RELEASE.zip";
    const ZIP_URL: &str =
        "https://github.com/acidanthera/AMFIPass/releases/download/1.4.1/AMFIPass-1.4.1-RELEASE.zip";
    const ZIP_BYTES: &[u8] = b"fake amfipass release zip";

    let json = format!(
        r#"{{"tag_name":"1.4.1","assets":[
            {{"name":"AMFIPass-1.4.1-DEBUG.zip","browser_download_url":"https://github.com/acidanthera/AMFIPass/releases/download/1.4.1/AMFIPass-1.4.1-DEBUG.zip"}},
            {{"name":"{ZIP_NAME}","browser_download_url":"{ZIP_URL}"}}
        ]}}"#
    );
    let http = FakeHttp::new()
        .with(AMFIPASS_TAG_URL, json)
        .with(ZIP_URL, ZIP_BYTES);
    let dir = tempfile::tempdir().unwrap();

    let got = fetch::fetch_amfipass(&http, dir.path()).expect("pinned release zip must fetch");
    assert_eq!(got.version, "1.4.1");
    assert_eq!(got.path, dir.path().join(ZIP_NAME));
    // Upstream publishes no sums: the hash is informational, computed and
    // returned but with nothing to compare against (spec §11.4).
    assert_eq!(got.sha256, hex_sha256(ZIP_BYTES));
}

#[test]
fn sums_listing_absent_file_errors() {
    // Sums covering a file that is not on disk are an error naming the file
    // — a truncated download must not slip through.
    let dir = tempfile::tempdir().unwrap();
    let sums = format!("{}  not-on-disk.tar.gz\n", "0".repeat(64));

    let err = fetch::verify_sha256sums(dir.path(), &sums).unwrap_err();
    assert!(
        err.to_string().contains("not-on-disk.tar.gz"),
        "error must name the missing file:\n{err}"
    );
}
