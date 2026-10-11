//! Integration tests for `greenlight doctor` — the read-only diagnostic.
//!
//! Every test drives the CLI through the fixture seam: the
//! `GREENLIGHT_FIXTURE_HARDWARE` env var replaces the live macOS probe, and
//! `--efi-path` points at a tempdir ESP holding a renamed config fixture.
//! `MacosSource` and `MacosEspMounter` are never exercised here (offline,
//! unprivileged constraint).

use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;

const FIXTURE_HARDWARE: &str = "tests/fixtures/hardware-lenovo.json";
const FIXTURE_BROKEN: &str = "tests/fixtures/config-broken.plist";
const FIXTURE_FIXED: &str = "tests/fixtures/config-fixed.plist";

/// Builds a tempdir ESP (`EFI/OC/config.plist`) from a config fixture.
/// Keep the returned `TempDir` bound — dropping it deletes the tree.
fn esp_with_config(fixture: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let oc_dir = dir.path().join("EFI").join("OC");
    fs::create_dir_all(&oc_dir).unwrap();
    fs::copy(fixture, oc_dir.join("config.plist")).unwrap();
    dir
}

/// `greenlight doctor` pinned to the Lenovo hardware fixture.
fn doctor() -> Command {
    let mut cmd = Command::cargo_bin("greenlight").unwrap();
    cmd.arg("doctor")
        .env("GREENLIGHT_FIXTURE_HARDWARE", FIXTURE_HARDWARE);
    cmd
}

#[test]
fn doctor_reports_lenovo_hardware_and_state() {
    let esp = esp_with_config(FIXTURE_BROKEN);
    doctor()
        .arg("--efi-path")
        .arg(esp.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("NVIDIA GeForce GTX 1650"))
        .stdout(predicate::str::contains("Intel UHD Graphics 630"))
        .stdout(predicate::str::contains("15.7.4"))
        .stdout(predicate::str::contains("0x0a43"))
        .stdout(predicate::str::contains("ready to fix"))
        .stdout(predicate::str::contains("greenlight fix"));
}

#[test]
fn doctor_json_is_parseable() {
    let esp = esp_with_config(FIXTURE_BROKEN);
    let assert = doctor()
        .arg("--json")
        .arg("--efi-path")
        .arg(esp.path())
        .assert()
        .success();
    let output = assert.get_output();
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("doctor --json must emit valid JSON");
    assert_eq!(value["state"], "ReadyToFix");
    let pci = value["hardware"]["pci"]
        .as_array()
        .expect("hardware.pci must be an array");
    assert_eq!(pci.len(), 3, "the lenovo fixture has three PCI devices");
    let findings = value["findings"]
        .as_array()
        .expect("findings must be an array");
    assert!(
        !findings.is_empty(),
        "the broken fixture must yield findings: {value}"
    );
}

#[test]
fn doctor_on_clean_machine_says_already_working() {
    let esp = esp_with_config(FIXTURE_FIXED);
    doctor()
        .arg("--efi-path")
        .arg(esp.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("already working"));
}

#[test]
fn doctor_on_fresh_config_says_needs_setup() {
    // A fresh OpenCore install has no leftovers to report but none of the
    // validated NullMoth set either — "already working" would be a lie.
    let dir = tempfile::tempdir().unwrap();
    let oc_dir = dir.path().join("EFI").join("OC");
    fs::create_dir_all(&oc_dir).unwrap();
    fs::write(
        oc_dir.join("config.plist"),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict/></plist>\n",
    )
    .unwrap();

    doctor()
        .arg("--efi-path")
        .arg(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("needs setup"))
        .stdout(predicate::str::contains("run `greenlight fix`"))
        .stdout(predicate::str::contains("already working").not());
}
