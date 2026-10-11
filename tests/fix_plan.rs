//! Integration tests for `occonfig::fix` — planning and applying the
//! validated NullMoth config set against the Task 2 fixtures.

use std::fs;
use std::path::{Path, PathBuf};

use greenlight::occonfig::{apply_fix, plan_fix, Error, FindingKind, OcConfig, QuirkEdit};
use plist::Value;

const FIXTURE_BROKEN: &str = "tests/fixtures/config-broken.plist";
const FIXTURE_FIXED: &str = "tests/fixtures/config-fixed.plist";
const NVIDIA_PATH: &str = "PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)";
const IGPU_PATH: &str = "PciRoot(0x0)/Pci(0x2,0x0)";
const AUDIO_PATH: &str = "PciRoot(0x0)/Pci(0x1F,0x3)";
const NVRAM_GUID: &str = "7C436110-AB2A-4BBB-A880-FE41995C9F82";

const BASE_ARGS: [&str; 6] = [
    "nvfb=1",
    "nvaccel=1",
    "nvfbheads=4",
    "-nvkmsnosmooth",
    "amfi_get_out_of_my_way=0x1",
    "amfi=0x80",
];

fn nvidia_paths() -> Vec<String> {
    vec![NVIDIA_PATH.to_string()]
}

/// Copies `fixture` into a fresh tempdir and loads the copy, so tests never
/// mutate the committed fixtures. The TempDir must stay alive for `save()`.
fn load_temp_copy(fixture: &str) -> (tempfile::TempDir, PathBuf, OcConfig) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.plist");
    fs::copy(fixture, &path).unwrap();
    let cfg = OcConfig::load(&path).expect("fixture copy should parse");
    (dir, path, cfg)
}

/// Navigates `value` down `path` (dictionary keys only), panicking with a
/// useful message when a node is missing or of the wrong type.
fn get<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
    let mut cur = value;
    for key in path {
        cur = cur
            .as_dictionary()
            .unwrap_or_else(|| panic!("{key:?}: parent is not a dictionary"))
            .get(key)
            .unwrap_or_else(|| panic!("missing key {key:?}"));
    }
    cur
}

#[test]
fn plan_on_broken_fixture_includes_all_edits() {
    let cfg = OcConfig::load(Path::new(FIXTURE_BROKEN)).unwrap();
    let plan = plan_fix(&cfg, &nvidia_paths(), true);

    // Both DeviceProperties removals on the NVIDIA path.
    let kinds: Vec<FindingKind> = plan.findings.iter().map(|f| f.kind).collect();
    assert!(
        kinds.contains(&FindingKind::FakeClassCode),
        "FakeClassCode missing from plan findings: {:?}",
        plan.findings
    );
    assert!(
        kinds.contains(&FindingKind::DisableGpu),
        "DisableGpu missing from plan findings: {:?}",
        plan.findings
    );

    // All six base boot-args, plus nvrmsettle when BAR1 placement failed.
    for arg in BASE_ARGS {
        assert!(
            plan.boot_args_add.iter().any(|a| a == arg),
            "{arg} missing from boot_args_add: {:?}",
            plan.boot_args_add
        );
    }
    assert!(plan.nvrmsettle, "nvrmsettle flag must be set");
    assert!(
        plan.boot_args_add.iter().any(|a| a == "nvrmsettle=10000"),
        "nvrmsettle=10000 missing from boot_args_add: {:?}",
        plan.boot_args_add
    );

    assert!(plan.csr_needs_set, "csr-active-config must need setting");

    // The three pinned quirks.
    let expected_quirks = [
        QuirkEdit {
            path: "UEFI.Quirks",
            key: "ResizeGpuBars",
            value: Value::Integer(13.into()),
        },
        QuirkEdit {
            path: "Booter.Quirks",
            key: "ResizeAppleGpuBars",
            value: Value::Integer((-1).into()),
        },
        QuirkEdit {
            path: "Misc.Security",
            key: "SecureBootModel",
            value: Value::String("Disabled".to_string()),
        },
    ];
    for q in &expected_quirks {
        assert!(
            plan.quirks.contains(q),
            "quirk {q:?} missing from plan: {:?}",
            plan.quirks
        );
    }

    // Fixture already has SecureBootModel=Disabled.
    assert!(
        !plan.smbios_downgrade_needed,
        "fixture is already Disabled, no downgrade needed"
    );
}

#[test]
fn apply_then_matches_fixed_fixture_semantics() {
    let (_dir, path, mut cfg) = load_temp_copy(FIXTURE_BROKEN);
    let plan = plan_fix(&cfg, &nvidia_paths(), true);
    apply_fix(&mut cfg, &plan, false).expect("apply should succeed");
    cfg.save().unwrap();

    let doc = Value::from_file(&path).unwrap();

    // NVIDIA DeviceProperties path removed entirely.
    let dp_add = get(&doc, &["DeviceProperties", "Add"])
        .as_dictionary()
        .unwrap();
    assert!(
        !dp_add.contains_key(NVIDIA_PATH),
        "NVIDIA path must be removed: {dp_add:?}"
    );

    // Non-NVIDIA entries untouched, byte-for-byte against the broken fixture.
    let broken = Value::from_file(FIXTURE_BROKEN).unwrap();
    let broken_add = get(&broken, &["DeviceProperties", "Add"])
        .as_dictionary()
        .unwrap();
    for path in [IGPU_PATH, AUDIO_PATH] {
        assert_eq!(
            dp_add.get(path),
            broken_add.get(path),
            "non-NVIDIA entry {path} must be untouched"
        );
    }

    // Boot-args: superset of the fixed fixture's NVIDIA args, killers gone,
    // pre-existing laptop args retained.
    let reloaded = OcConfig::load(&path).unwrap();
    let args = reloaded.boot_args();
    let fixed = OcConfig::load(Path::new(FIXTURE_FIXED)).unwrap();
    let fixed_args = fixed.boot_args();
    for arg in BASE_ARGS.iter().chain(&["nvrmsettle=10000"]) {
        assert!(
            fixed_args.contains(*arg),
            "test bug: {arg} not in the fixed fixture's boot-args"
        );
        assert!(args.contains(*arg), "{arg} missing after apply: {args:?}");
    }
    assert!(
        !args.contains("nv_disable=1"),
        "nv_disable=1 must be removed"
    );
    assert!(args.contains("alcid=11"), "pre-existing args must be kept");

    // csr-active-config set and listed in NVRAM Delete.
    let guid_add = get(&doc, &["NVRAM", "Add", NVRAM_GUID])
        .as_dictionary()
        .unwrap();
    assert_eq!(
        guid_add.get("csr-active-config"),
        Some(&Value::Data(vec![0x43, 0x0A, 0x00, 0x00])),
        "csr-active-config must be Data 43 0A 00 00"
    );
    let delete = get(&doc, &["NVRAM", "Delete", NVRAM_GUID])
        .as_array()
        .expect("NVRAM Delete entry for the GUID must be an array");
    assert!(
        delete
            .iter()
            .any(|v| v.as_string() == Some("csr-active-config")),
        "csr-active-config must be listed in NVRAM Delete: {delete:?}"
    );

    // Kernel → Block entry for IONDRVSupport.
    let block = get(&doc, &["Kernel", "Block"]).as_array().unwrap();
    let entry = block.iter().find(|e| {
        e.as_dictionary()
            .and_then(|d| d.get("Identifier"))
            .and_then(Value::as_string)
            == Some("com.apple.iokit.IONDRVSupport")
    });
    let entry = entry
        .unwrap_or_else(|| panic!("IONDRVSupport block entry missing: {block:?}"))
        .as_dictionary()
        .unwrap();
    assert_eq!(
        entry.get("Strategy").and_then(Value::as_string),
        Some("Exclude"),
        "Strategy must be Exclude: {entry:?}"
    );
    assert_eq!(
        entry.get("Enabled").and_then(Value::as_boolean),
        Some(true),
        "entry must be enabled: {entry:?}"
    );

    // Quirks.
    assert_eq!(
        get(&doc, &["UEFI", "Quirks", "ResizeGpuBars"]).as_signed_integer(),
        Some(13),
        "ResizeGpuBars must be 13"
    );
    assert_eq!(
        get(&doc, &["Booter", "Quirks", "ResizeAppleGpuBars"]).as_signed_integer(),
        Some(-1),
        "ResizeAppleGpuBars must be -1"
    );
    assert_eq!(
        get(&doc, &["Misc", "Security", "SecureBootModel"]).as_string(),
        Some("Disabled"),
        "SecureBootModel must be Disabled"
    );
}

#[test]
fn second_apply_is_noop() {
    let (_dir, path, mut cfg) = load_temp_copy(FIXTURE_BROKEN);
    let plan = plan_fix(&cfg, &nvidia_paths(), true);
    apply_fix(&mut cfg, &plan, false).unwrap();
    cfg.save().unwrap();
    let once = fs::read_to_string(&path).unwrap();

    // Re-apply the same plan: must not change the serialized output.
    apply_fix(&mut cfg, &plan, false).unwrap();
    cfg.save().unwrap();
    let twice = fs::read_to_string(&path).unwrap();
    assert_eq!(once, twice, "applying the same plan twice must be a no-op");

    // Review Focus 3: no duplicated NVRAM Delete entries or Block entries.
    let doc = Value::from_file(&path).unwrap();
    let delete = get(&doc, &["NVRAM", "Delete", NVRAM_GUID])
        .as_array()
        .unwrap();
    let csr_entries = delete
        .iter()
        .filter(|v| v.as_string() == Some("csr-active-config"))
        .count();
    assert_eq!(csr_entries, 1, "csr-active-config duplicated in Delete");
    let block = get(&doc, &["Kernel", "Block"]).as_array().unwrap();
    let iondrv_entries = block
        .iter()
        .filter(|e| {
            e.as_dictionary()
                .and_then(|d| d.get("Identifier"))
                .and_then(Value::as_string)
                == Some("com.apple.iokit.IONDRVSupport")
        })
        .count();
    assert_eq!(iondrv_entries, 1, "IONDRVSupport block entry duplicated");

    // A fresh plan against the fixed config finds nothing left to do.
    let replan = plan_fix(&cfg, &nvidia_paths(), true);
    assert!(
        replan.findings.is_empty(),
        "fixed config must scan clean: {:?}",
        replan.findings
    );
    assert!(
        replan.boot_args_add.is_empty(),
        "fixed config needs no new boot-args: {:?}",
        replan.boot_args_add
    );
    assert!(!replan.csr_needs_set, "fixed config needs no csr change");
}

#[test]
fn nvrmsettle_omitted_when_bar1_ok() {
    let (_dir, _path, mut cfg) = load_temp_copy(FIXTURE_BROKEN);
    let plan = plan_fix(&cfg, &nvidia_paths(), false);
    assert!(!plan.nvrmsettle, "nvrmsettle flag must be clear");
    assert!(
        !plan.boot_args_add.iter().any(|a| a == "nvrmsettle=10000"),
        "nvrmsettle=10000 must not be planned when BAR1 placed fine: {:?}",
        plan.boot_args_add
    );
    for arg in BASE_ARGS {
        assert!(
            plan.boot_args_add.iter().any(|a| a == arg),
            "{arg} missing from boot_args_add: {:?}",
            plan.boot_args_add
        );
    }

    apply_fix(&mut cfg, &plan, false).unwrap();
    let args = cfg.boot_args();
    assert!(
        !args.contains("nvrmsettle=10000"),
        "nvrmsettle=10000 must not be applied: {args:?}"
    );
}

#[test]
fn smbios_downgrade_requires_confirmation() {
    let (_dir, path, _cfg) = load_temp_copy(FIXTURE_BROKEN);

    // Edit the tempdir copy (never the committed fixture): SecureBootModel
    // back to the SMBIOS default.
    let mut doc = Value::from_file(&path).unwrap();
    doc.as_dictionary_mut()
        .unwrap()
        .get_mut("Misc")
        .and_then(Value::as_dictionary_mut)
        .and_then(|m| m.get_mut("Security"))
        .and_then(Value::as_dictionary_mut)
        .unwrap()
        .insert(
            "SecureBootModel".to_string(),
            Value::String("Default".to_string()),
        );
    doc.to_file_xml(&path).unwrap();

    let mut cfg = OcConfig::load(&path).unwrap();
    let plan = plan_fix(&cfg, &nvidia_paths(), true);
    assert!(
        plan.smbios_downgrade_needed,
        "SecureBootModel=Default must need a downgrade"
    );

    // Refused without confirmation, and nothing is mutated on refusal.
    let err = apply_fix(&mut cfg, &plan, false).expect_err("apply without confirmation must fail");
    assert!(
        matches!(err, Error::SecureBootModelNotConfirmed),
        "wrong error: {err:?}"
    );
    assert!(
        cfg.boot_args().contains("nv_disable=1"),
        "a refused apply must leave the config untouched"
    );

    // Allowed with confirmation.
    apply_fix(&mut cfg, &plan, true).expect("apply with confirmation should succeed");
    cfg.save().unwrap();
    let doc = Value::from_file(&path).unwrap();
    assert_eq!(
        get(&doc, &["Misc", "Security", "SecureBootModel"]).as_string(),
        Some("Disabled"),
        "SecureBootModel must be downgraded to Disabled"
    );
}
