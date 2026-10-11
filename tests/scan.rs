//! Integration tests for `occonfig::scan` breakage detection.

use std::fs;
use std::path::Path;

use greenlight::occonfig::{scan, FindingKind, OcConfig};

const FIXTURE_BROKEN: &str = "tests/fixtures/config-broken.plist";
const FIXTURE_FIXED: &str = "tests/fixtures/config-fixed.plist";
const NVIDIA_PATH: &str = "PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)";

fn nvidia_paths() -> Vec<String> {
    vec![NVIDIA_PATH.to_string()]
}

fn kinds(findings: &[greenlight::occonfig::Finding]) -> Vec<FindingKind> {
    findings.iter().map(|f| f.kind).collect()
}

/// Loads a minimal XML plist from a string.
fn load_xml(xml: &str) -> OcConfig {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.plist");
    fs::write(&path, xml).unwrap();
    // The document is fully in memory, so the tempdir may drop.
    OcConfig::load(&path).expect("test plist should parse")
}

#[test]
fn finds_dortania_leftover_in_broken_fixture() {
    let cfg = OcConfig::load(Path::new(FIXTURE_BROKEN)).unwrap();
    let findings = scan(&cfg, &nvidia_paths());
    let kinds = kinds(&findings);
    assert!(
        kinds.contains(&FindingKind::FakeClassCode),
        "FakeClassCode missing from {findings:?}"
    );
    assert!(
        kinds.contains(&FindingKind::DisableGpu),
        "DisableGpu missing from {findings:?}"
    );
    for f in findings
        .iter()
        .filter(|f| matches!(f.kind, FindingKind::FakeClassCode | FindingKind::DisableGpu))
    {
        assert_eq!(f.location, NVIDIA_PATH, "wrong location in {f:?}");
        assert!(!f.detail.is_empty(), "detail must explain the finding");
    }
}

#[test]
fn fixed_fixture_scans_clean() {
    let cfg = OcConfig::load(Path::new(FIXTURE_FIXED)).unwrap();
    let findings = scan(&cfg, &nvidia_paths());
    assert!(
        findings.is_empty(),
        "fixed fixture must scan clean, got {findings:?}"
    );
}

#[test]
fn ignores_classcode_on_non_nvidia_path() {
    // Review Focus 2: the same leftover on a path that is not in
    // `nvidia_paths` must NOT be flagged.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
	<key>DeviceProperties</key>
	<dict>
		<key>Add</key>
		<dict>
			<key>PciRoot(0x0)/Pci(0x2,0x0)</key>
			<dict>
				<key>class-code</key>
				<data>/////w==</data>
			</dict>
		</dict>
	</dict>
</dict>
</plist>"#;
    let cfg = load_xml(xml);
    let findings = scan(&cfg, &nvidia_paths());
    assert!(
        findings.is_empty(),
        "non-NVIDIA path must be ignored, got {findings:?}"
    );
}

#[test]
fn finds_nv_disable_and_pikera_in_boot_args() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
	<key>NVRAM</key>
	<dict>
		<key>Add</key>
		<dict>
			<key>7C436110-AB2A-4BBB-A880-FE41995C9F82</key>
			<dict>
				<key>boot-args</key>
				<string>-v keepsyms=1 nv_disable=1 agdpmod=pikera</string>
			</dict>
		</dict>
	</dict>
</dict>
</plist>"#;
    let cfg = load_xml(xml);
    let findings = scan(&cfg, &nvidia_paths());
    let kinds = kinds(&findings);
    assert!(
        kinds.contains(&FindingKind::NvDisableArg),
        "NvDisableArg missing from {findings:?}"
    );
    assert!(
        kinds.contains(&FindingKind::PikeraArg),
        "PikeraArg missing from {findings:?}"
    );
}

#[test]
fn detects_ruling_variants_on_nvidia_paths() {
    // Controller rulings: class-code counts as fake for any all-0xFF Data of
    // 4 or 8 bytes and for the string "FFFFFFFF"; disable-gpu counts by key
    // presence regardless of payload width.
    let mut add = plist::Dictionary::new();
    let mut wide = plist::Dictionary::new();
    wide.insert("class-code".to_string(), plist::Value::Data(vec![0xFF; 8]));
    add.insert(
        "PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)".to_string(),
        plist::Value::Dictionary(wide),
    );
    let mut stringy = plist::Dictionary::new();
    stringy.insert(
        "class-code".to_string(),
        plist::Value::String("FFFFFFFF".to_string()),
    );
    add.insert(
        "PciRoot(0x0)/Pci(0x3,0x0)/Pci(0x0,0x0)".to_string(),
        plist::Value::Dictionary(stringy),
    );
    let mut narrow = plist::Dictionary::new();
    narrow.insert("disable-gpu".to_string(), plist::Value::Data(vec![0x01]));
    add.insert(
        "PciRoot(0x0)/Pci(0x2,0x0)/Pci(0x0,0x0)".to_string(),
        plist::Value::Dictionary(narrow),
    );

    let mut dp = plist::Dictionary::new();
    dp.insert("Add".to_string(), plist::Value::Dictionary(add));
    let mut root = plist::Dictionary::new();
    root.insert("DeviceProperties".to_string(), plist::Value::Dictionary(dp));

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.plist");
    plist::Value::Dictionary(root)
        .to_writer_xml(fs::File::create(&path).unwrap())
        .unwrap();
    let cfg = OcConfig::load(&path).unwrap();

    let paths: Vec<String> = [
        "PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)",
        "PciRoot(0x0)/Pci(0x3,0x0)/Pci(0x0,0x0)",
        "PciRoot(0x0)/Pci(0x2,0x0)/Pci(0x0,0x0)",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let findings = scan(&cfg, &paths);

    assert_eq!(
        findings.len(),
        3,
        "each variant must be flagged exactly once: {findings:?}"
    );
    assert!(findings
        .iter()
        .any(|f| f.kind == FindingKind::FakeClassCode && f.location == paths[0]));
    assert!(findings
        .iter()
        .any(|f| f.kind == FindingKind::FakeClassCode && f.location == paths[1]));
    assert!(findings
        .iter()
        .any(|f| f.kind == FindingKind::DisableGpu && f.location == paths[2]));
}

#[test]
fn ignores_benign_values_on_nvidia_path() {
    // A real class-code (e.g. VGA 0x030000) and no disable-gpu key must not
    // be flagged; disable-gpu present but on a path NOT in the list is out of
    // scope by ruling.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
	<key>DeviceProperties</key>
	<dict>
		<key>Add</key>
		<dict>
			<key>PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)</key>
			<dict>
				<key>class-code</key>
				<data>AAADAA==</data>
			</dict>
		</dict>
	</dict>
</dict>
</plist>"#;
    let cfg = load_xml(xml);
    let findings = scan(&cfg, &nvidia_paths());
    assert!(
        findings.is_empty(),
        "real class-code must not be flagged, got {findings:?}"
    );
}
