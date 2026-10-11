//! Integration tests for `greenlight fix` — the full remediation flow.
//!
//! Three seams keep every test offline and unprivileged:
//! `GREENLIGHT_FIXTURE_HARDWARE` (recorded probe, as in `tests/doctor.rs`),
//! `--efi-path` pointing at a tempdir ESP, and `GREENLIGHT_FIXTURE_HTTP`
//! pointing at a tempdir of canned downloads (`FixtureHttp`). The AMFIPass
//! zip is a real `ditto` archive of a tiny fake kext built inside the
//! tempdir — no upstream content is committed. Tests never run as root, so
//! the driver phase must stop at its "pending" note; `hdiutil` and the real
//! NullMoth `install.sh` are never exercised.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcCommand;

use assert_cmd::Command;
use predicates::prelude::*;
use sha2::{Digest, Sha256};

use greenlight::fetch::FixtureHttp;
use greenlight::nullmoth;
use greenlight::occonfig::{amfipass_registered, scan, OcConfig};

const FIXTURE_HARDWARE: &str = "tests/fixtures/hardware-lenovo.json";
const FIXTURE_BROKEN: &str = "tests/fixtures/config-broken.plist";
const NVIDIA_PATH: &str = "PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)";

const BASE_ARGS: [&str; 6] = [
    "nvfb=1",
    "nvaccel=1",
    "nvfbheads=4",
    "-nvkmsnosmooth",
    "amfi_get_out_of_my_way=0x1",
    "amfi=0x80",
];

const NULLMOTH_LATEST_URL: &str =
    "https://api.github.com/repos/nullmoth/nvidia-macos-driver/releases/latest";
const AMFIPASS_TAG_URL: &str =
    "https://api.github.com/repos/acidanthera/AMFIPass/releases/tags/1.4.1";
const TARBALL_NAME: &str = "nullmoth-nvidia-1.10.0.tar.gz";
const TARBALL_URL: &str =
    "https://github.com/nullmoth/nvidia-macos-driver/releases/download/v1.10.0/nullmoth-nvidia-1.10.0.tar.gz";
const SUMS_URL: &str =
    "https://github.com/nullmoth/nvidia-macos-driver/releases/download/v1.10.0/SHA256SUMS.txt";
const DMG_NAME: &str = "1401-Mac-1.10.0.dmg";
const DMG_URL: &str =
    "https://github.com/nullmoth/nvidia-macos-driver/releases/download/v1.10.0/1401-Mac-1.10.0.dmg";
const ZIP_NAME: &str = "AMFIPass-1.4.1-RELEASE.zip";
const ZIP_URL: &str =
    "https://github.com/acidanthera/AMFIPass/releases/download/1.4.1/AMFIPass-1.4.1-RELEASE.zip";

const TARBALL_BYTES: &[u8] = b"fake nullmoth tarball payload";
const DMG_BYTES: &[u8] = b"fake 1401 dmg payload";

fn hex_sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn euid() -> u32 {
    let out = ProcCommand::new("id").arg("-u").output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

/// Builds a tempdir ESP (`EFI/OC/config.plist`) from a config fixture.
/// Keep the returned `TempDir` bound — dropping it deletes the tree.
fn esp_with_config(fixture: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let oc_dir = dir.path().join("EFI").join("OC");
    fs::create_dir_all(&oc_dir).unwrap();
    fs::copy(fixture, oc_dir.join("config.plist")).unwrap();
    dir
}

fn config_path(esp: &Path) -> PathBuf {
    esp.join("EFI").join("OC").join("config.plist")
}

/// `config.plist.bak-greenlight-*` siblings created by fix runs.
fn backups(oc_dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(oc_dir)
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            let name = path.file_name()?.to_str()?.to_string();
            name.starts_with("config.plist.bak-greenlight-")
                .then_some(path)
        })
        .collect();
    found.sort();
    found
}

/// Writes a tiny fake `AMFIPass.kext` (an Info.plist and a stub binary) and
/// packs it as a real PKZip archive with the macOS built-in `ditto`.
fn build_fake_amfipass_zip(dir: &Path) {
    let contents = dir
        .join("kext-build")
        .join("AMFIPass.kext")
        .join("Contents");
    fs::create_dir_all(contents.join("MacOS")).unwrap();
    fs::write(
        contents.join("Info.plist"),
        b"<?xml version=\"1.0\"?><plist><dict><key>CFBundleIdentifier</key><string>test</string></dict></plist>",
    )
    .unwrap();
    fs::write(contents.join("MacOS").join("AMFIPass"), b"fake").unwrap();
    let status = ProcCommand::new("ditto")
        .args(["-c", "-k", "--keepParent"])
        .arg(contents.parent().unwrap())
        .arg(dir.join(ZIP_NAME))
        .status()
        .unwrap();
    assert!(status.success(), "ditto must pack the fake kext zip");
}

/// A canned-downloads dir for the `GREENLIGHT_FIXTURE_HTTP` seam: the latest
/// NullMoth release JSON (tarball + sums assets), the tarball itself and its
/// SHA256SUMS.txt line, the pinned AMFIPass release JSON, and its zip.
///
/// `dmg_asset` adds the 1401 DMG to the release JSON and serves its bytes;
/// `dmg_in_sums` additionally lists it in SHA256SUMS.txt — the two switches
/// exist so the NullMothSafe.efi decision logic can be tested without ever
/// touching `hdiutil`.
fn downloads_dir(dmg_asset: bool, dmg_in_sums: bool) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    build_fake_amfipass_zip(dir.path());

    fs::write(dir.path().join(TARBALL_NAME), TARBALL_BYTES).unwrap();
    let mut sums = format!("{}  {TARBALL_NAME}\n", hex_sha256(TARBALL_BYTES));
    if dmg_in_sums {
        sums.push_str(&format!("{}  {DMG_NAME}\n", hex_sha256(DMG_BYTES)));
    }
    fs::write(dir.path().join("SHA256SUMS.txt"), sums).unwrap();

    let dmg_entry = if dmg_asset {
        fs::write(dir.path().join(DMG_NAME), DMG_BYTES).unwrap();
        format!(r#",{{"name":"{DMG_NAME}","browser_download_url":"{DMG_URL}"}}"#)
    } else {
        String::new()
    };
    let nullmoth_release = format!(
        r#"{{"tag_name":"v1.10.0","assets":[
            {{"name":"{TARBALL_NAME}","browser_download_url":"{TARBALL_URL}"}},
            {{"name":"SHA256SUMS.txt","browser_download_url":"{SUMS_URL}"}}{dmg_entry}
        ]}}"#
    );
    fs::write(dir.path().join("nullmoth-release.json"), nullmoth_release).unwrap();

    let amfipass_release = format!(
        r#"{{"tag_name":"1.4.1","assets":[
            {{"name":"{ZIP_NAME}","browser_download_url":"{ZIP_URL}"}}
        ]}}"#
    );
    fs::write(dir.path().join("amfipass-release.json"), amfipass_release).unwrap();

    let mut manifest = format!(
        r#"{{"{NULLMOTH_LATEST_URL}":"nullmoth-release.json",
"{TARBALL_URL}":"{TARBALL_NAME}",
"{SUMS_URL}":"SHA256SUMS.txt",
"{AMFIPASS_TAG_URL}":"amfipass-release.json",
"{ZIP_URL}":"{ZIP_NAME}""#
    );
    if dmg_asset {
        manifest.push_str(&format!(r#","{DMG_URL}":"{DMG_NAME}""#));
    }
    manifest.push('}');
    fs::write(dir.path().join("manifest.json"), manifest).unwrap();
    dir
}

/// A canned-downloads dir serving *only* AMFIPass: every NullMoth URL
/// errors. Proves a non-root run defers the driver phase without fetching.
fn downloads_dir_amfipass_only() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    build_fake_amfipass_zip(dir.path());
    let amfipass_release = format!(
        r#"{{"tag_name":"1.4.1","assets":[
            {{"name":"{ZIP_NAME}","browser_download_url":"{ZIP_URL}"}}
        ]}}"#
    );
    fs::write(dir.path().join("amfipass-release.json"), amfipass_release).unwrap();
    let manifest = format!(
        r#"{{"{AMFIPASS_TAG_URL}":"amfipass-release.json",
"{ZIP_URL}":"{ZIP_NAME}"}}"#
    );
    fs::write(dir.path().join("manifest.json"), manifest).unwrap();
    dir
}

/// `greenlight fix` pinned to the fixture seams.
fn fix(esp: &Path, downloads: &Path) -> Command {
    let mut cmd = Command::cargo_bin("greenlight").unwrap();
    cmd.arg("fix")
        .env("GREENLIGHT_FIXTURE_HARDWARE", FIXTURE_HARDWARE)
        .env("GREENLIGHT_FIXTURE_HTTP", downloads)
        .arg("--efi-path")
        .arg(esp);
    cmd
}

/// How many `Kernel → Add` entries reference `AMFIPass.kext`.
fn amfipass_entry_count(config: &Path) -> usize {
    let file = fs::File::open(config).unwrap();
    let value = plist::Value::from_reader(std::io::BufReader::new(file)).unwrap();
    value
        .as_dictionary()
        .and_then(|root| root.get("Kernel"))
        .and_then(plist::Value::as_dictionary)
        .and_then(|kernel| kernel.get("Add"))
        .and_then(plist::Value::as_array)
        .map(|add| {
            add.iter()
                .filter(|entry| {
                    entry
                        .as_dictionary()
                        .and_then(|d| d.get("BundlePath"))
                        .and_then(plist::Value::as_string)
                        == Some("AMFIPass.kext")
                })
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn fix_dry_run_changes_nothing() {
    let esp = esp_with_config(FIXTURE_BROKEN);
    let downloads = downloads_dir(false, false);
    let config = config_path(esp.path());
    let before = fs::read(&config).unwrap();

    fix(esp.path(), downloads.path())
        .arg("--dry-run")
        .assert()
        .success()
        // stdout lists the planned edits…
        .stdout(predicate::str::contains("nvfb=1"))
        .stdout(predicate::str::contains("SecureBootModel"))
        .stdout(predicate::str::contains("AMFIPass.kext"))
        // …the Windows-BSOD rule the ResizeGpuBars edit makes relevant…
        .stdout(predicate::str::contains("firmware boot menu (F12)"))
        .stdout(predicate::str::contains("blue-screen"))
        // …and the planned driver-phase commands.
        .stdout(predicate::str::contains("install.sh"));

    assert_eq!(
        fs::read(&config).unwrap(),
        before,
        "--dry-run must not change the config"
    );
    assert!(
        backups(config.parent().unwrap()).is_empty(),
        "--dry-run must not create a backup"
    );
    assert!(
        !esp.path().join("EFI").join("OC").join("Kexts").exists(),
        "--dry-run must not touch the ESP"
    );
}

#[test]
fn fix_yes_applies_and_backs_up() {
    let esp = esp_with_config(FIXTURE_BROKEN);
    let downloads = downloads_dir(false, false);
    let config = config_path(esp.path());
    let before = fs::read(&config).unwrap();

    fix(esp.path(), downloads.path())
        .arg("--yes")
        .assert()
        .success()
        // Unprivileged test run: the driver phase defers with its note.
        .stdout(predicate::str::contains("driver phase pending"))
        .stdout(predicate::str::contains("sudo"))
        // Recovery block with the real backup name substituted.
        .stdout(predicate::str::contains("If macOS fails to boot"))
        .stdout(predicate::str::contains("cp config.plist.bak-greenlight-"))
        .stdout(predicate::str::contains("reboot"))
        // The applied ResizeGpuBars change resurfaces the Windows-BSOD rule.
        .stdout(predicate::str::contains("firmware boot menu (F12)"))
        .stdout(predicate::str::contains("blue-screen"))
        // The canned release has no 1401 DMG → the OpenShell fallback note.
        .stdout(predicate::str::contains("NullMothSafe.efi"));

    // The backup holds the original bytes.
    let backups = backups(config.parent().unwrap());
    assert_eq!(backups.len(), 1, "exactly one backup must be created");
    assert_eq!(
        fs::read(&backups[0]).unwrap(),
        before,
        "the backup must preserve the pre-fix bytes"
    );

    // The applied config scans clean and carries the pinned NVIDIA set.
    let cfg = OcConfig::load(&config).unwrap();
    let findings = scan(&cfg, &[NVIDIA_PATH.to_string()]);
    assert!(
        findings.is_empty(),
        "the fixed config must scan clean, got: {findings:?}"
    );
    let args = cfg.boot_args();
    for arg in BASE_ARGS {
        assert!(args.contains(arg), "boot-args missing {arg}: {args:?}");
    }
    assert!(
        amfipass_registered(&cfg),
        "AMFIPass must be registered in Kernel → Add"
    );
    assert!(
        esp.path()
            .join("EFI")
            .join("OC")
            .join("Kexts")
            .join("AMFIPass.kext")
            .join("Contents")
            .join("Info.plist")
            .is_file(),
        "AMFIPass.kext must be copied into EFI/OC/Kexts"
    );
}

#[test]
fn fix_twice_second_run_is_noop() {
    let esp = esp_with_config(FIXTURE_BROKEN);
    let downloads = downloads_dir(false, false);
    let config = config_path(esp.path());

    fix(esp.path(), downloads.path())
        .arg("--yes")
        .assert()
        .success();
    let after_first = fs::read(&config).unwrap();

    fix(esp.path(), downloads.path())
        .arg("--yes")
        .assert()
        .success()
        .stdout(predicate::str::contains("already fixed"));

    assert_eq!(
        fs::read(&config).unwrap(),
        after_first,
        "the second run must not rewrite the config"
    );
    assert_eq!(
        backups(config.parent().unwrap()).len(),
        1,
        "the second run must not create another backup"
    );
    assert_eq!(
        amfipass_entry_count(&config),
        1,
        "AMFIPass registration must be idempotent"
    );
}

#[test]
fn fix_refuses_unvalidated_without_force() {
    // An NVIDIA display device with an unknown device ID and no Intel iGPU
    // is outside the validated profile (Turing dGPU + Intel iGPU).
    let hw_dir = tempfile::tempdir().unwrap();
    let hw_path = hw_dir.path().join("hardware.json");
    fs::write(
        &hw_path,
        r#"{"pci":[
            {"vendor":"10de","device":"9999","class":"0x030000","acpi_path":"PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)","name":"Unknown NVIDIA GPU"},
            {"vendor":"10de","device":"10fa","class":"0x040300","acpi_path":"PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x1)","name":"NVIDIA HDAU"}
        ],"macos":"15.7.4","sip":"0x0a43"}"#,
    )
    .unwrap();

    let esp = esp_with_config(FIXTURE_BROKEN);
    let downloads = downloads_dir(false, false);
    let config = config_path(esp.path());
    let before = fs::read(&config).unwrap();

    let mut cmd = Command::cargo_bin("greenlight").unwrap();
    cmd.arg("fix")
        .env("GREENLIGHT_FIXTURE_HARDWARE", &hw_path)
        .env("GREENLIGHT_FIXTURE_HTTP", downloads.path())
        .arg("--efi-path")
        .arg(esp.path())
        .arg("--yes")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--force"));
    assert_eq!(
        fs::read(&config).unwrap(),
        before,
        "the refused run must not touch the config"
    );

    let mut forced = Command::cargo_bin("greenlight").unwrap();
    forced
        .arg("fix")
        .env("GREENLIGHT_FIXTURE_HARDWARE", &hw_path)
        .env("GREENLIGHT_FIXTURE_HTTP", downloads.path())
        .arg("--efi-path")
        .arg(esp.path())
        .arg("--yes")
        .arg("--force")
        .assert()
        .success();
}

#[test]
fn fix_refuses_pascal_dgpu_even_with_igpu() {
    // The gate pins the Turing mobile IDs: a Pascal dGPU (10de:1c8c, GTX
    // 1070 Mobile) WITH the Intel iGPU present is still refused without
    // --force, and still applies with it.
    let hw_dir = tempfile::tempdir().unwrap();
    let hw_path = hw_dir.path().join("hardware.json");
    fs::write(
        &hw_path,
        r#"{"pci":[
            {"vendor":"10de","device":"1c8c","class":"0x030000","acpi_path":"PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)","name":"NVIDIA GeForce GTX 1070 Mobile"},
            {"vendor":"8086","device":"3e9b","class":"0x030000","acpi_path":"PciRoot(0x0)/Pci(0x2,0x0)","name":"Intel UHD Graphics 630"}
        ],"macos":"15.7.4","sip":"0x0a43"}"#,
    )
    .unwrap();

    let esp = esp_with_config(FIXTURE_BROKEN);
    let downloads = downloads_dir(false, false);
    let config = config_path(esp.path());
    let before = fs::read(&config).unwrap();

    let mut cmd = Command::cargo_bin("greenlight").unwrap();
    cmd.arg("fix")
        .env("GREENLIGHT_FIXTURE_HARDWARE", &hw_path)
        .env("GREENLIGHT_FIXTURE_HTTP", downloads.path())
        .arg("--efi-path")
        .arg(esp.path())
        .arg("--yes")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--force"));
    assert_eq!(
        fs::read(&config).unwrap(),
        before,
        "the refused run must not touch the config"
    );

    let mut forced = Command::cargo_bin("greenlight").unwrap();
    forced
        .arg("fix")
        .env("GREENLIGHT_FIXTURE_HARDWARE", &hw_path)
        .env("GREENLIGHT_FIXTURE_HTTP", downloads.path())
        .arg("--efi-path")
        .arg(esp.path())
        .arg("--yes")
        .arg("--force")
        .assert()
        .success();
}

#[test]
fn fix_without_root_still_completes_config_phase() {
    if euid() == 0 {
        eprintln!("skipping: this test must run unprivileged");
        return;
    }
    let esp = esp_with_config(FIXTURE_BROKEN);
    let downloads = downloads_dir(false, false);
    let config = config_path(esp.path());

    fix(esp.path(), downloads.path())
        .arg("--yes")
        .assert()
        .success()
        // The exact re-run command is printed: the canonicalized absolute
        // binary path, every argument single-quoted, paste-ready…
        .stdout(predicate::str::contains("driver phase pending"))
        .stdout(predicate::str::contains("sudo '/"))
        .stdout(predicate::str::contains("greenlight' 'fix'"))
        // …and no install happened.
        .stdout(predicate::str::contains("NullMoth driver installed").not());

    // No partial state: the config phase completed fully…
    let cfg = OcConfig::load(&config).unwrap();
    assert!(scan(&cfg, &[NVIDIA_PATH.to_string()]).is_empty());
    assert!(amfipass_registered(&cfg));
    assert_eq!(backups(config.parent().unwrap()).len(), 1);
    // …and the driver phase left nothing behind on the ESP.
    assert!(
        !esp.path().join("EFI").join("OC").join("Drivers").exists(),
        "the driver phase must not touch the ESP"
    );
}

#[test]
fn fix_non_root_defers_driver_phase_without_fetching() {
    if euid() == 0 {
        eprintln!("skipping: this test must run unprivileged");
        return;
    }
    let esp = esp_with_config(FIXTURE_BROKEN);
    let downloads = downloads_dir_amfipass_only();
    let config = config_path(esp.path());

    fix(esp.path(), downloads.path())
        .arg("--yes")
        .assert()
        .success()
        .stdout(predicate::str::contains("driver phase pending"))
        .stdout(predicate::str::contains("sudo '/"))
        .stdout(predicate::str::contains("greenlight' 'fix'"))
        // The recovery block belongs to the config phase: it prints even
        // though the driver phase deferred.
        .stdout(predicate::str::contains("If macOS fails to boot"));

    let cfg = OcConfig::load(&config).unwrap();
    assert!(scan(&cfg, &[NVIDIA_PATH.to_string()]).is_empty());
    assert_eq!(backups(config.parent().unwrap()).len(), 1);
}

#[test]
fn nullmoth_safe_none_when_release_has_no_dmg() {
    let downloads = downloads_dir(false, false);
    let http = FixtureHttp::new(downloads.path().to_path_buf());
    let got = nullmoth::try_fetch_nullmoth_safe(&http, downloads.path())
        .expect("fetch failures degrade to Ok(None), never Err");
    assert!(got.is_none(), "no DMG asset → no NullMothSafe.efi");
}

#[test]
fn nullmoth_safe_none_when_dmg_not_in_sums() {
    // The DMG is served but SHA256SUMS.txt does not list it: unverifiable,
    // so the flow must fall back to the printed OpenShell recovery steps.
    let downloads = downloads_dir(true, false);
    let http = FixtureHttp::new(downloads.path().to_path_buf());
    let got = nullmoth::try_fetch_nullmoth_safe(&http, downloads.path())
        .expect("an unverifiable DMG degrades to Ok(None), never Err");
    assert!(got.is_none(), "an unlisted DMG is never trusted");
}
