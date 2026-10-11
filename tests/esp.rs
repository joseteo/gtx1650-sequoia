//! Integration tests for `esp` — locating the boot OpenCore config.
//!
//! `MacosEspMounter` is never exercised here (offline/unprivileged
//! constraint): every test drives `find_oc_configs` with a tempdir fake.

use std::fs;
use std::path::{Path, PathBuf};

use greenlight::esp::{self, Error, EspMounter};

/// Minimal valid plist — contents are irrelevant here; location is the test.
const MINIMAL_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict/></plist>"#;

/// Fake `EspMounter`: treats each immediate subdirectory of `root` as a
/// mounted ESP and collects `EFI/OC/config.plist` from each, sorted for
/// deterministic assertions.
struct FakeEspMounter {
    root: PathBuf,
}

impl EspMounter for FakeEspMounter {
    fn list_esp_configs(&self) -> Result<Vec<PathBuf>, Error> {
        let mut configs = Vec::new();
        for entry in fs::read_dir(&self.root).unwrap() {
            let candidate = entry
                .unwrap()
                .path()
                .join("EFI")
                .join("OC")
                .join("config.plist");
            if candidate.is_file() {
                configs.push(candidate);
            }
        }
        configs.sort();
        Ok(configs)
    }
}

/// Creates `<root>/<name>/EFI/OC/config.plist` and returns its path.
fn make_esp(root: &Path, name: &str) -> PathBuf {
    let oc_dir = root.join(name).join("EFI").join("OC");
    fs::create_dir_all(&oc_dir).unwrap();
    let config = oc_dir.join("config.plist");
    fs::write(&config, MINIMAL_PLIST).unwrap();
    config
}

fn fake(root: &Path) -> FakeEspMounter {
    FakeEspMounter {
        root: root.to_path_buf(),
    }
}

#[test]
fn override_path_wins() {
    // Controller ruling: with an override dir the mounter is never
    // consulted — a mounter holding two configs (would be ambiguous) must
    // be ignored.
    let dir = tempfile::tempdir().unwrap();
    let config = make_esp(dir.path(), "esp0");
    let override_dir = dir.path().join("esp0");

    let mounter_root = tempfile::tempdir().unwrap();
    make_esp(mounter_root.path(), "boot-esp");
    make_esp(mounter_root.path(), "installer-usb");

    let found = esp::find_oc_configs(Some(&override_dir), &fake(mounter_root.path()))
        .expect("override dir with EFI/OC/config.plist must resolve");
    assert_eq!(found, vec![config]);
}

#[test]
fn override_accepts_oc_dir_itself() {
    // Controller ruling: when the dir itself is the OC directory,
    // `<dir>/config.plist` counts (no EFI/OC nesting required).
    let dir = tempfile::tempdir().unwrap();
    let oc_dir = dir.path().join("OC");
    fs::create_dir_all(&oc_dir).unwrap();
    let config = oc_dir.join("config.plist");
    fs::write(&config, MINIMAL_PLIST).unwrap();

    let found = esp::find_oc_configs(Some(&oc_dir), &fake(&oc_dir.join("nonexistent")))
        .expect("an OC dir passed directly must resolve");
    assert_eq!(found, vec![config]);
}

#[test]
fn override_without_config_errors() {
    // The other half of the ruling: an override dir with no config in
    // either accepted spot is an error naming the dir, not a fallback to
    // the mounter.
    let dir = tempfile::tempdir().unwrap();
    let err = esp::find_oc_configs(Some(dir.path()), &fake(dir.path())).unwrap_err();
    assert!(
        matches!(err, Error::OverrideNoConfig { .. }),
        "expected OverrideNoConfig, got {err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains(&dir.path().display().to_string()),
        "message must name the override dir:\n{msg}"
    );
}

#[test]
fn zero_configs_errors() {
    let dir = tempfile::tempdir().unwrap();
    let err = esp::find_oc_configs(None, &fake(dir.path())).unwrap_err();
    assert!(
        matches!(err, Error::NoOpenCore),
        "expected NoOpenCore, got {err:?}"
    );
}

#[test]
fn one_config_ok() {
    let dir = tempfile::tempdir().unwrap();
    let config = make_esp(dir.path(), "esp0");
    let found =
        esp::find_oc_configs(None, &fake(dir.path())).expect("exactly one config must resolve");
    assert_eq!(found, vec![config]);
}

#[test]
fn two_configs_is_ambiguous() {
    // Review Focus 1: boot ESP + installer USB still plugged in → an error
    // listing both paths and demanding --efi-path, never a silent pick.
    let dir = tempfile::tempdir().unwrap();
    let boot = make_esp(dir.path(), "boot-esp");
    let installer = make_esp(dir.path(), "installer-usb");

    let err = esp::find_oc_configs(None, &fake(dir.path())).unwrap_err();
    match &err {
        Error::Ambiguous(paths) => {
            assert_eq!(
                paths,
                &vec![boot.clone(), installer.clone()],
                "Ambiguous must carry every candidate"
            );
        }
        other => panic!("expected Ambiguous, got {other:?}"),
    }

    let msg = err.to_string();
    assert!(
        msg.contains(&boot.display().to_string()),
        "message must list {boot:?}:\n{msg}"
    );
    assert!(
        msg.contains(&installer.display().to_string()),
        "message must list {installer:?}:\n{msg}"
    );
    assert!(
        msg.contains("--efi-path"),
        "message must mention --efi-path:\n{msg}"
    );
}
