use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn doctor_runs_and_prints_header() {
    // The fixture seam keeps this test offline: a recorded hardware dump
    // plus a tempdir ESP holding the fixed config fixture.
    let esp = tempfile::tempdir().unwrap();
    let oc_dir = esp.path().join("EFI").join("OC");
    std::fs::create_dir_all(&oc_dir).unwrap();
    std::fs::copy(
        "tests/fixtures/config-fixed.plist",
        oc_dir.join("config.plist"),
    )
    .unwrap();
    Command::cargo_bin("greenlight")
        .unwrap()
        .arg("doctor")
        .env(
            "GREENLIGHT_FIXTURE_HARDWARE",
            "tests/fixtures/hardware-lenovo.json",
        )
        .arg("--efi-path")
        .arg(esp.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("greenlight doctor"));
}

#[test]
fn forge_exits_2_with_planned_message() {
    Command::cargo_bin("greenlight")
        .unwrap()
        .arg("forge")
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "forge: not yet implemented (planned for v2)",
        ));
}

#[test]
fn unknown_subcommand_errors() {
    Command::cargo_bin("greenlight")
        .unwrap()
        .arg("bogus")
        .assert()
        .failure()
        .stderr(predicate::str::contains("error"));
}
