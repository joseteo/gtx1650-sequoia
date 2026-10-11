//! Integration tests for `greenlight fix --check` — post-reboot verification
//! that the NullMoth driver stack is live.
//!
//! Every test is offline and unprivileged. Library tests drive
//! [`check::run_check`] with a fake [`Shell`] over the committed fixtures:
//! `kextstat-working.txt` / `displays-working.txt` were captured from the
//! reference machine while the driver was live (the display serial is
//! scrubbed — no hardware identity is committed), and the `-broken` copies
//! are edits of them (one `com.nullmoth.*` line deleted; no NVIDIA section).
//! The CLI test points the `GREENLIGHT_FIXTURE_SHELL_DIR` seam — dev/test-only,
//! the same standing as `GREENLIGHT_FIXTURE_HARDWARE` — at a tempdir holding
//! `kextstat.txt` and `displays.txt`.

use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;

use greenlight::nullmoth::check::{self, Shell};
use greenlight::nullmoth::Error;

const KEXTSTAT_WORKING: &str = include_str!("fixtures/kextstat-working.txt");
const KEXTSTAT_BROKEN: &str = include_str!("fixtures/kextstat-broken.txt");
const DISPLAYS_WORKING: &str = include_str!("fixtures/displays-working.txt");
const DISPLAYS_BROKEN: &str = include_str!("fixtures/displays-broken.txt");

/// A canned-output `Shell`: the two probes return fixture text.
struct FakeShell {
    kextstat: &'static str,
    displays: &'static str,
}

impl Shell for FakeShell {
    fn run(&self, cmd: &str, _args: &[&str]) -> Result<String, Error> {
        match cmd {
            "kextstat" => Ok(self.kextstat.to_string()),
            "system_profiler" => Ok(self.displays.to_string()),
            other => panic!("the check must only probe kextstat/system_profiler, got {other}"),
        }
    }
}

#[test]
fn check_parses_loaded_kexts_and_display() {
    let sh = FakeShell {
        kextstat: KEXTSTAT_WORKING,
        displays: DISPLAYS_WORKING,
    };
    let report = check::run_check(&sh);

    for id in check::REQUIRED_KEXTS {
        assert!(
            report.kexts_loaded.iter().any(|k| k == id),
            "{id} must be reported loaded"
        );
    }
    assert_eq!(report.kexts_loaded.len(), check::REQUIRED_KEXTS.len());
    assert!(
        report.gpu_claimed,
        "the NVIDIA section means the driver claimed the GPU"
    );
    assert!(report.passed(), "4/4 kexts + claimed GPU is a pass");

    let display = report
        .displays
        .first()
        .expect("the NVIDIA display must be parsed");
    assert!(display.contains("GS27QA"), "display name: {display}");
    assert!(display.contains("2560 x 1440"), "resolution: {display}");
    assert!(display.contains("144.00Hz"), "refresh rate: {display}");
    assert_eq!(report.metal.as_deref(), Some("Metal 3"));

    // The committed fixture must carry no hardware identity: every
    // identity line is scrubbed — asserted structurally, so the scrubbed
    // values themselves never enter the repo.
    for line in DISPLAYS_WORKING.lines() {
        if line.contains("Display Serial Number") || line.contains("UUID") {
            assert!(
                line.trim_end().ends_with("REDACTED"),
                "identity line must be scrubbed: {line}"
            );
        }
    }
}

#[test]
fn check_fails_when_kext_missing() {
    // The broken fixture is the working one with com.nullmoth.NVRMAGDC deleted.
    let sh = FakeShell {
        kextstat: KEXTSTAT_BROKEN,
        displays: DISPLAYS_WORKING,
    };
    let report = check::run_check(&sh);

    assert_eq!(report.kexts_loaded.len(), 3);
    assert!(
        !report
            .kexts_loaded
            .iter()
            .any(|k| k == "com.nullmoth.NVRMAGDC"),
        "the deleted kext must not be reported loaded"
    );
    assert_eq!(report.missing_kexts(), ["com.nullmoth.NVRMAGDC"]);
    assert!(report.gpu_claimed, "the display side still parses");
    assert!(
        !report.passed(),
        "3/4 kexts is a fail even with the GPU claimed"
    );
}

#[test]
fn check_fails_when_gpu_unclaimed() {
    // The broken displays fixture is Intel-only — no NVIDIA section at all.
    let sh = FakeShell {
        kextstat: KEXTSTAT_WORKING,
        displays: DISPLAYS_BROKEN,
    };
    let report = check::run_check(&sh);

    assert_eq!(report.kexts_loaded.len(), check::REQUIRED_KEXTS.len());
    assert!(!report.gpu_claimed);
    assert!(
        !report.passed(),
        "an unclaimed GPU is a fail even with 4/4 kexts"
    );
    assert!(report.displays.is_empty());
    assert_eq!(report.metal, None);
}

/// A tempdir for the `GREENLIGHT_FIXTURE_SHELL_DIR` seam. Keep the returned
/// `TempDir` bound — dropping it deletes the tree.
fn shell_dir(kextstat: &str, displays: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("kextstat.txt"), kextstat).unwrap();
    std::fs::write(dir.path().join("displays.txt"), displays).unwrap();
    dir
}

/// `greenlight fix --check` pinned to the fixture shell seam. The hardware
/// fixture seam is deliberately *not* set: `--check` must not probe hardware.
fn fix_check(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("greenlight").unwrap();
    cmd.arg("fix")
        .arg("--check")
        .env("GREENLIGHT_FIXTURE_SHELL_DIR", dir)
        .env_remove("GREENLIGHT_FIXTURE_HARDWARE");
    cmd
}

#[test]
fn fix_check_exit_codes() {
    // Driver stack live: exit 0 and the proof table.
    let dir = shell_dir(KEXTSTAT_WORKING, DISPLAYS_WORKING);
    fix_check(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("kexts loaded"))
        .stdout(predicate::str::contains("4/4"))
        .stdout(predicate::str::contains("GPU claimed"))
        .stdout(predicate::str::contains("GS27QA"))
        .stdout(predicate::str::contains("2560 x 1440 @ 144.00Hz"))
        .stdout(predicate::str::contains("Metal 3"))
        .stdout(predicate::str::contains("PASS"));

    // A missing kext: exit 1 with what-failed printed.
    let dir = shell_dir(KEXTSTAT_BROKEN, DISPLAYS_WORKING);
    fix_check(dir.path())
        .assert()
        .failure()
        .stdout(predicate::str::contains("3/4"))
        .stdout(predicate::str::contains("com.nullmoth.NVRMAGDC"))
        .stdout(predicate::str::contains("FAIL"));

    // The driver never claimed the GPU (Intel-only display): exit 1.
    let dir = shell_dir(KEXTSTAT_WORKING, DISPLAYS_BROKEN);
    fix_check(dir.path())
        .assert()
        .failure()
        .stdout(predicate::str::contains("GPU not claimed"))
        .stdout(predicate::str::contains("FAIL"));
}
