//! Integration tests for `occonfig` load/save/backup against the Task 2
//! real-machine fixtures.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use greenlight::occonfig::OcConfig;

const FIXTURE_BROKEN: &str = "tests/fixtures/config-broken.plist";
const FIXTURE_FIXED: &str = "tests/fixtures/config-fixed.plist";

#[test]
fn loads_broken_fixture_and_reads_boot_args() {
    let cfg = OcConfig::load(Path::new(FIXTURE_BROKEN)).expect("broken fixture should parse");
    let args = cfg.boot_args();
    assert!(args.contains("alcid=11"), "alcid=11 missing from {args:?}");
    assert!(
        args.contains("nv_disable=1"),
        "nv_disable=1 missing from {args:?}"
    );
    assert!(args.contains("-v"), "-v missing from {args:?}");
}

#[test]
fn backup_creates_timestamped_sibling() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("config.plist");
    fs::copy(FIXTURE_BROKEN, &src).unwrap();

    let bak = OcConfig::backup(&src).expect("backup should succeed");

    assert_eq!(bak.parent(), Some(dir.path()), "backup must be a sibling");
    let name = bak.file_name().unwrap().to_str().unwrap();
    let stamp = name
        .strip_prefix("config.plist.bak-greenlight-")
        .unwrap_or_else(|| panic!("bad backup name: {name}"));
    // Exactly YYYYMMDD-HHMMSS.
    assert_eq!(stamp.len(), 15, "bad timestamp in {name}");
    assert_eq!(&stamp[8..9], "-", "bad timestamp in {name}");
    assert!(
        stamp[..8].chars().all(|c| c.is_ascii_digit()),
        "bad date in {name}"
    );
    assert!(
        stamp[9..].chars().all(|c| c.is_ascii_digit()),
        "bad time in {name}"
    );

    assert_eq!(
        fs::read(&bak).unwrap(),
        fs::read(FIXTURE_BROKEN).unwrap(),
        "backup content must match the original"
    );
    assert_eq!(
        fs::read(&src).unwrap(),
        fs::read(FIXTURE_BROKEN).unwrap(),
        "backup must not modify the source"
    );
}

#[test]
fn save_roundtrip_preserves_other_keys() {
    let dir = tempfile::tempdir().unwrap();
    let dst = dir.path().join("config.plist");
    fs::copy(FIXTURE_FIXED, &dst).unwrap();

    let before = plist::Value::from_file(&dst).unwrap();
    let cfg = OcConfig::load(&dst).unwrap();
    cfg.save().expect("save should succeed");
    let after = plist::Value::from_file(&dst).unwrap();

    assert_eq!(before, after, "save must preserve every key");

    let reloaded = OcConfig::load(&dst).unwrap();
    assert_eq!(cfg.boot_args(), reloaded.boot_args());
}

#[test]
fn save_leaves_no_temp_sibling() {
    let dir = tempfile::tempdir().unwrap();
    let dst = dir.path().join("config.plist");
    fs::copy(FIXTURE_FIXED, &dst).unwrap();

    let cfg = OcConfig::load(&dst).unwrap();
    cfg.save().expect("save should succeed");

    let names: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_str().unwrap().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["config.plist".to_string()],
        "the temp-then-rename write must clean up after itself"
    );
}

#[test]
fn set_boot_args_writes_space_joined_string() {
    let dir = tempfile::tempdir().unwrap();
    let dst = dir.path().join("config.plist");
    fs::copy(FIXTURE_BROKEN, &dst).unwrap();

    let mut cfg = OcConfig::load(&dst).unwrap();
    let args: BTreeSet<String> = ["-v", "alcid=11", "nvfb=1"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    cfg.set_boot_args(&args);
    assert_eq!(cfg.boot_args(), args);

    // BTreeSet iteration order is sorted, so the stored string is exact.
    cfg.save().unwrap();
    let xml = fs::read_to_string(&dst).unwrap();
    assert!(
        xml.contains("<string>-v alcid=11 nvfb=1</string>"),
        "boot-args not stored as a single space-joined string"
    );
}

#[test]
fn device_properties_mut_exposes_add_entries() {
    let mut cfg = OcConfig::load(Path::new(FIXTURE_BROKEN)).unwrap();
    const NVIDIA: &str = "PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)";

    {
        let add = cfg
            .device_properties_mut()
            .expect("DeviceProperties/Add must exist");
        assert!(add.contains_key(NVIDIA));
        assert!(add.contains_key("PciRoot(0x0)/Pci(0x2,0x0)"));
        add.remove(NVIDIA);
    }

    let add = cfg.device_properties_mut().unwrap();
    assert!(!add.contains_key(NVIDIA), "mutation must stick in the doc");
    assert!(
        add.contains_key("PciRoot(0x0)/Pci(0x2,0x0)"),
        "non-NVIDIA entries must be untouched"
    );
}
