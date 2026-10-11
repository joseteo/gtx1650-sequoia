//! Integration tests for `detect` — hardware probing behind a trait.
//!
//! `MacosSource` is never exercised here (offline constraint): every test
//! drives `FixtureSource` over the Task 2 hardware dump, or synthetic
//! `Hardware` values built in place.

use std::fs;
use std::path::{Path, PathBuf};

use greenlight::detect::{
    classify, FixtureSource, Hardware, HardwareSource, MachineState, PciDevice,
};
use greenlight::occonfig::{scan, OcConfig};

const FIXTURE_HARDWARE: &str = "tests/fixtures/hardware-lenovo.json";
const FIXTURE_BROKEN: &str = "tests/fixtures/config-broken.plist";
const FIXTURE_FIXED: &str = "tests/fixtures/config-fixed.plist";
const GTX1650_PATH: &str = "PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)";

fn lenovo_hardware() -> Hardware {
    FixtureSource {
        json_path: PathBuf::from(FIXTURE_HARDWARE),
    }
    .load()
    .expect("lenovo fixture must parse")
}

#[test]
fn fixture_yields_gtx1650_path() {
    let hw = lenovo_hardware();
    assert_eq!(hw.macos_version, Some((15, 7, 4)));
    assert_eq!(hw.sip.as_deref(), Some("0x0a43"));
    // Exact-vector equality also proves the HDAU audio function
    // (10de:10fa, class 0x040300) is NOT reported as a display path.
    assert_eq!(
        hw.nvidia_display_paths(),
        vec![GTX1650_PATH.to_string()],
        "only the display-class GTX 1650 may appear"
    );
}

#[test]
fn lenovo_is_validated_profile() {
    let hw = lenovo_hardware();
    assert!(
        hw.is_validated_profile(),
        "GTX 1650 dGPU + Intel UHD 630 iGPU is the validated laptop profile"
    );
}

#[test]
fn turing_mobile_family_is_validated() {
    // Every pinned TU117M GTX 1650 Mobile variant passes the gate beside an
    // Intel iGPU, not just the reference card's 1f99.
    let igpu = PciDevice {
        vendor: 0x8086,
        device: 0x3e9b,
        class_code: 0x030000,
        acpi_path: Some("PciRoot(0x0)/Pci(0x2,0x0)".to_string()),
        name: "Intel UHD Graphics 630".to_string(),
    };
    for device in [0x1f91, 0x1f92, 0x1f94, 0x1f95, 0x1f96, 0x1f99, 0x1f9d] {
        let mut hw = lenovo_hardware();
        hw.pci.clear();
        hw.pci.push(PciDevice {
            vendor: 0x10de,
            device,
            class_code: 0x030000,
            acpi_path: Some(GTX1650_PATH.to_string()),
            name: "NVIDIA TU117M".to_string(),
        });
        hw.pci.push(igpu.clone());
        assert!(
            hw.is_validated_profile(),
            "10de:{device:04x} + Intel iGPU must pass the gate"
        );
    }
}

#[test]
fn pascal_dgpu_with_igpu_is_not_validated() {
    // The gate is a pinned Turing mobile set: a Pascal dGPU keeps the Intel
    // iGPU requirement from mattering — 10de:1c8c (GTX 1070 Mobile) with an
    // iGPU present is still outside the validated profile.
    let mut hw = lenovo_hardware();
    hw.pci.iter_mut().for_each(|d| {
        if d.vendor == 0x10de && d.class_code == 0x030000 {
            d.device = 0x1c8c;
            d.name = "NVIDIA GeForce GTX 1070 Mobile".to_string();
        }
    });
    assert!(
        !hw.is_validated_profile(),
        "a Pascal dGPU must fail the Turing-pinned gate even with an iGPU"
    );
}

#[test]
fn classify_broken_fixture_is_ready_to_fix() {
    let hw = lenovo_hardware();
    let cfg = OcConfig::load(Path::new(FIXTURE_BROKEN)).unwrap();
    let findings = scan(&cfg, &hw.nvidia_display_paths());
    assert!(!findings.is_empty(), "broken fixture must produce findings");
    assert_eq!(
        classify(&hw, Some(&cfg), &findings),
        MachineState::ReadyToFix
    );
}

#[test]
fn classify_fixed_fixture_is_already_working() {
    let hw = lenovo_hardware();
    let cfg = OcConfig::load(Path::new(FIXTURE_FIXED)).unwrap();
    let findings = scan(&cfg, &hw.nvidia_display_paths());
    assert!(
        findings.is_empty(),
        "fixed fixture must scan clean: {findings:?}"
    );
    assert_eq!(
        classify(&hw, Some(&cfg), &findings),
        MachineState::AlreadyWorking
    );
}

#[test]
fn classify_fresh_config_is_needs_setup_not_already_working() {
    // A fresh OpenCore config: zero leftovers AND zero of the validated
    // NullMoth set. Reporting AlreadyWorking here would tell someone whose
    // GPU is still dead that everything is fine.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.plist");
    fs::write(
        &path,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict/></plist>\n",
    )
    .unwrap();
    let cfg = OcConfig::load(&path).unwrap();

    let hw = lenovo_hardware();
    let findings = scan(&cfg, &hw.nvidia_display_paths());
    assert!(findings.is_empty(), "an empty config has no leftovers");
    assert_eq!(
        classify(&hw, Some(&cfg), &findings),
        MachineState::NeedsSetup
    );
}

#[test]
fn no_nvidia_machine_reports_so() {
    // Ruling: NoNvidia is checked FIRST — even a config full of
    // NVIDIA-killing leftovers (the broken fixture's global nv_disable=1
    // is found without any NVIDIA path) does not change the verdict.
    let hw = Hardware {
        pci: vec![PciDevice {
            vendor: 0x8086,
            device: 0x3e9b,
            class_code: 0x030000,
            acpi_path: Some("PciRoot(0x0)/Pci(0x2,0x0)".to_string()),
            name: "Intel UHD Graphics 630".to_string(),
        }],
        macos_version: Some((15, 7, 4)),
        sip: None,
    };
    assert!(
        !hw.is_validated_profile(),
        "an Intel-only machine is not the validated profile"
    );
    let cfg = OcConfig::load(Path::new(FIXTURE_BROKEN)).unwrap();
    let findings = scan(&cfg, &hw.nvidia_display_paths());
    assert!(
        !findings.is_empty(),
        "broken fixture's global nv_disable=1 must still be found"
    );
    assert_eq!(classify(&hw, Some(&cfg), &findings), MachineState::NoNvidia);
}

#[test]
fn unsupported_macos_is_checked_before_config_state() {
    // Ruling: the check order is NoNvidia → UnsupportedMacos → NoOpenCore →
    // findings. A pre-15.5 machine is unsupported even when the config is
    // broken, and even when no config was loaded at all.
    let mut hw = lenovo_hardware();
    hw.macos_version = Some((15, 4, 1));
    let cfg = OcConfig::load(Path::new(FIXTURE_BROKEN)).unwrap();
    let findings = scan(&cfg, &hw.nvidia_display_paths());
    assert!(!findings.is_empty());
    assert_eq!(
        classify(&hw, Some(&cfg), &findings),
        MachineState::UnsupportedMacos,
        "an old macOS beats a broken config"
    );
    assert_eq!(
        classify(&hw, None, &[]),
        MachineState::UnsupportedMacos,
        "an old macOS beats a missing config"
    );
}

#[test]
fn missing_config_reports_no_opencore() {
    // Ruling: NoOpenCore comes before the findings check — a config that
    // was never loaded cannot make the machine "ready to fix".
    let hw = lenovo_hardware();
    let cfg = OcConfig::load(Path::new(FIXTURE_BROKEN)).unwrap();
    let findings = scan(&cfg, &hw.nvidia_display_paths());
    assert!(!findings.is_empty());
    assert_eq!(classify(&hw, None, &findings), MachineState::NoOpenCore);
    assert_eq!(classify(&hw, None, &[]), MachineState::NoOpenCore);
}

#[test]
fn unknown_macos_version_is_not_unsupported() {
    // macos_version is None when the source could not determine it (e.g. a
    // non-macOS host); "unknown" must not read as "too old".
    let mut hw = lenovo_hardware();
    hw.macos_version = None;
    let cfg = OcConfig::load(Path::new(FIXTURE_FIXED)).unwrap();
    let findings = scan(&cfg, &hw.nvidia_display_paths());
    assert_eq!(
        classify(&hw, Some(&cfg), &findings),
        MachineState::AlreadyWorking
    );
}
