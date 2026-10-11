//! Fix planning and application: turns [`scan`] findings into the exact
//! OpenCore config state the reference laptop proved correct (INSTALL-GUIDE
//! §3). `plan_fix` computes the diff; `apply_fix` applies it idempotently.

use std::collections::{BTreeMap, BTreeSet};

use plist::{Dictionary, Value};

use super::{dict_entry, scan, Error, Finding, FindingKind, OcConfig, NVRAM_GUID};

/// Boot-args every NullMoth setup needs, in plan order.
const BASE_BOOT_ARGS: [&str; 6] = [
    "nvfb=1",
    "nvaccel=1",
    "nvfbheads=4",
    "-nvkmsnosmooth",
    "amfi_get_out_of_my_way=0x1",
    "amfi=0x80",
];

/// Added only when the GPU's BAR1 failed to place (long NVRM settle).
const NVRMSETTLE_ARG: &str = "nvrmsettle=10000";

/// `csr-active-config` value that leaves SIP permissive enough for NullMoth.
const CSR_ACTIVE_CONFIG: [u8; 4] = [0x43, 0x0A, 0x00, 0x00];

/// DeviceProperties keys that silently kill the NVIDIA GPU.
const KILLING_KEYS: [&str; 2] = ["class-code", "disable-gpu"];

const IONDRV_SUPPORT: &str = "com.apple.iokit.IONDRVSupport";

/// Everything `apply_fix` will change, computed from a scan of the current
/// config. Fields carry only work that is actually needed, except `quirks`,
/// which always lists all three pinned values (applying them is a no-op when
/// they already match).
#[derive(Debug, Clone, PartialEq)]
pub struct FixPlan {
    pub findings: Vec<Finding>,
    pub boot_args_add: Vec<String>,
    pub csr_needs_set: bool,
    pub quirks: Vec<QuirkEdit>,
    pub smbios_downgrade_needed: bool,
    pub nvrmsettle: bool,
}

/// One pinned `key = value` under a dotted config path (e.g. `"UEFI.Quirks"`).
#[derive(Debug, Clone, PartialEq)]
pub struct QuirkEdit {
    pub path: &'static str,
    pub key: &'static str,
    pub value: plist::Value,
}

/// Computes the diff between `cfg` and the validated NullMoth config set.
/// `nvrmsettle=10000` is planned only when `bar1_unplaced` is true.
pub fn plan_fix(cfg: &OcConfig, nvidia_paths: &[String], bar1_unplaced: bool) -> FixPlan {
    let findings = scan(cfg, nvidia_paths);

    let mut required: Vec<String> = BASE_BOOT_ARGS.iter().map(|s| s.to_string()).collect();
    if bar1_unplaced {
        required.push(NVRMSETTLE_ARG.to_string());
    }
    let current = cfg.boot_args();
    let boot_args_add: Vec<String> = required
        .into_iter()
        .filter(|arg| !current.contains(arg))
        .collect();

    let csr_needs_set = csr_needs_set(cfg);

    let quirks = vec![
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

    let smbios_downgrade_needed = secure_boot_model(cfg) != Some("Disabled");

    FixPlan {
        findings,
        boot_args_add,
        csr_needs_set,
        quirks,
        smbios_downgrade_needed,
        nvrmsettle: bar1_unplaced,
    }
}

/// Applies `plan` to `cfg`. Returns `Err(Error::SecureBootModelNotConfirmed)`
/// — before touching anything — when the plan requires downgrading
/// `SecureBootModel` but `allow_smbios_downgrade` is false.
///
/// Idempotent: applying the same plan twice, or applying a fresh plan to an
/// already-fixed config, leaves the serialized config unchanged.
pub fn apply_fix(
    cfg: &mut OcConfig,
    plan: &FixPlan,
    allow_smbios_downgrade: bool,
) -> Result<(), Error> {
    if plan.smbios_downgrade_needed && !allow_smbios_downgrade {
        return Err(Error::SecureBootModelNotConfirmed);
    }

    remove_device_properties_findings(cfg, &plan.findings);

    let mut args = cfg.boot_args();
    for finding in &plan.findings {
        match finding.kind {
            FindingKind::NvDisableArg => {
                args.remove("nv_disable=1");
            }
            FindingKind::PikeraArg => {
                args.remove("agdpmod=pikera");
            }
            _ => {}
        }
    }
    args.extend(plan.boot_args_add.iter().cloned());
    cfg.set_boot_args(&args);

    if plan.csr_needs_set {
        set_csr_active_config(cfg);
    }

    ensure_iondrv_block(cfg);

    for quirk in &plan.quirks {
        let mut node = &mut cfg.doc;
        for segment in quirk.path.split('.') {
            node = dict_entry(node, segment);
        }
        node.insert(quirk.key.to_string(), quirk.value.clone());
    }

    Ok(())
}

/// True when `csr-active-config` under the NVRAM GUID is not the target data
/// or is missing from that GUID's `Delete` array.
fn csr_needs_set(cfg: &OcConfig) -> bool {
    let guid_add = cfg
        .doc
        .get("NVRAM")
        .and_then(Value::as_dictionary)
        .and_then(|nvram| nvram.get("Add"))
        .and_then(Value::as_dictionary)
        .and_then(|add| add.get(NVRAM_GUID))
        .and_then(Value::as_dictionary);
    let value_ok = guid_add
        .and_then(|guid| guid.get("csr-active-config"))
        .and_then(Value::as_data)
        == Some(CSR_ACTIVE_CONFIG.as_slice());

    let delete_ok = cfg
        .doc
        .get("NVRAM")
        .and_then(Value::as_dictionary)
        .and_then(|nvram| nvram.get("Delete"))
        .and_then(Value::as_dictionary)
        .and_then(|delete| delete.get(NVRAM_GUID))
        .and_then(Value::as_array)
        .is_some_and(|keys| {
            keys.iter()
                .any(|k| k.as_string() == Some("csr-active-config"))
        });

    !(value_ok && delete_ok)
}

/// The current `Misc → Security → SecureBootModel` string, if present.
fn secure_boot_model(cfg: &OcConfig) -> Option<&str> {
    cfg.doc
        .get("Misc")
        .and_then(Value::as_dictionary)
        .and_then(|misc| misc.get("Security"))
        .and_then(Value::as_dictionary)
        .and_then(|security| security.get("SecureBootModel"))
        .and_then(Value::as_string)
}

/// Removes DeviceProperties findings conservatively: the whole entry at a
/// finding's path goes only when it consists solely of the killing keys;
/// otherwise just the keys the findings implicate are removed (INSTALL-GUIDE
/// §2 removed the whole dortania leftover entry).
fn remove_device_properties_findings(cfg: &mut OcConfig, findings: &[Finding]) {
    let mut by_location: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for finding in findings {
        let key = match finding.kind {
            FindingKind::FakeClassCode => "class-code",
            FindingKind::DisableGpu => "disable-gpu",
            _ => continue,
        };
        by_location
            .entry(finding.location.as_str())
            .or_default()
            .insert(key);
    }

    let Some(add) = cfg.device_properties_mut() else {
        return;
    };
    for (location, keys) in by_location {
        let only_killing_keys = add
            .get(location)
            .and_then(Value::as_dictionary)
            .is_some_and(|entry| {
                !entry.is_empty() && entry.keys().all(|k| KILLING_KEYS.contains(&k.as_str()))
            });
        if only_killing_keys {
            add.remove(location);
        } else if let Some(entry) = add.get_mut(location).and_then(Value::as_dictionary_mut) {
            for key in keys {
                entry.remove(key);
            }
        }
    }
}

/// Sets `csr-active-config` under the NVRAM GUID and lists it in that GUID's
/// `Delete` array, creating intermediate nodes as needed.
fn set_csr_active_config(cfg: &mut OcConfig) {
    let nvram = dict_entry(&mut cfg.doc, "NVRAM");
    let add = dict_entry(nvram, "Add");
    let guid = dict_entry(add, NVRAM_GUID);
    guid.insert(
        "csr-active-config".to_string(),
        Value::Data(CSR_ACTIVE_CONFIG.to_vec()),
    );

    let delete = dict_entry(nvram, "Delete");
    let keys = array_entry(delete, NVRAM_GUID);
    if !keys
        .iter()
        .any(|k| k.as_string() == Some("csr-active-config"))
    {
        keys.push(Value::String("csr-active-config".to_string()));
    }
}

/// Ensures `Kernel → Block` carries an enabled `Exclude` entry for
/// `com.apple.iokit.IONDRVSupport`, repairing an existing entry in place or
/// appending the validated one otherwise.
fn ensure_iondrv_block(cfg: &mut OcConfig) {
    let kernel = dict_entry(&mut cfg.doc, "Kernel");
    let block = array_entry(kernel, "Block");

    let existing = block.iter_mut().find_map(|entry| {
        let dict = entry.as_dictionary_mut()?;
        (dict.get("Identifier").and_then(Value::as_string) == Some(IONDRV_SUPPORT)).then_some(dict)
    });
    if let Some(entry) = existing {
        entry.insert("Strategy".to_string(), Value::String("Exclude".to_string()));
        entry.insert("Enabled".to_string(), Value::Boolean(true));
        return;
    }

    let mut entry = Dictionary::new();
    entry.insert("Arch".to_string(), Value::String("Any".to_string()));
    entry.insert(
        "Comment".to_string(),
        Value::String("NullMoth: keep firmware framebuffer off display 0".to_string()),
    );
    entry.insert("Enabled".to_string(), Value::Boolean(true));
    entry.insert(
        "Identifier".to_string(),
        Value::String(IONDRV_SUPPORT.to_string()),
    );
    entry.insert("MaxKernel".to_string(), Value::String(String::new()));
    entry.insert("MinKernel".to_string(), Value::String(String::new()));
    entry.insert("Strategy".to_string(), Value::String("Exclude".to_string()));
    block.push(Value::Dictionary(entry));
}

/// Returns `parent[key]` as an array, inserting a fresh one when the key is
/// absent and replacing the value when it is not an array.
fn array_entry<'a>(parent: &'a mut Dictionary, key: &str) -> &'a mut Vec<Value> {
    let replace = match parent.get(key) {
        Some(value) => value.as_array().is_none(),
        None => true,
    };
    if replace {
        parent.insert(key.to_string(), Value::Array(Vec::new()));
    }
    parent
        .get_mut(key)
        .and_then(Value::as_array_mut)
        .expect("just inserted an array")
}

/// `Kernel → Add` BundlePath of the AMFIPass kext.
const AMFIPASS_BUNDLE_PATH: &str = "AMFIPass.kext";
/// `Misc → Tools` Path of the NullMothSafe recovery tool.
const NULLMOTH_SAFE_PATH: &str = "NullMothSafe.efi";

/// Ensures `Kernel → Add` carries the pinned AMFIPass entry — the kext the
/// NullMoth driver needs to get past AMFI. Idempotent: an entry whose
/// BundlePath is already `AMFIPass.kext` is repaired to the pinned fields in
/// place rather than duplicated.
pub fn register_amfipass(cfg: &mut OcConfig) {
    let kernel = dict_entry(&mut cfg.doc, "Kernel");
    let add = array_entry(kernel, "Add");
    let existing = add.iter_mut().find_map(|entry| {
        let dict = entry.as_dictionary_mut()?;
        (dict.get("BundlePath").and_then(Value::as_string) == Some(AMFIPASS_BUNDLE_PATH))
            .then_some(dict)
    });
    match existing {
        Some(entry) => {
            entry.insert("Arch".to_string(), Value::String("Any".to_string()));
            entry.insert("Enabled".to_string(), Value::Boolean(true));
            entry.insert("MinKernel".to_string(), Value::String("20.0.0".to_string()));
            entry.insert(
                "ExecutablePath".to_string(),
                Value::String("Contents/MacOS/AMFIPass".to_string()),
            );
            entry.insert(
                "PlistPath".to_string(),
                Value::String("Contents/Info.plist".to_string()),
            );
        }
        None => add.push(Value::Dictionary(amfipass_entry())),
    }
}

/// True when `Kernel → Add` already has an entry for `AMFIPass.kext`.
pub fn amfipass_registered(cfg: &OcConfig) -> bool {
    cfg.doc
        .get("Kernel")
        .and_then(Value::as_dictionary)
        .and_then(|kernel| kernel.get("Add"))
        .and_then(Value::as_array)
        .is_some_and(|add| {
            add.iter().any(|entry| {
                entry
                    .as_dictionary()
                    .and_then(|d| d.get("BundlePath"))
                    .and_then(Value::as_string)
                    == Some(AMFIPASS_BUNDLE_PATH)
            })
        })
}

/// The validated AMFIPass entry, mirroring the reference config.
fn amfipass_entry() -> Dictionary {
    let mut entry = Dictionary::new();
    entry.insert("MaxKernel".to_string(), Value::String(String::new()));
    entry.insert("MinKernel".to_string(), Value::String("20.0.0".to_string()));
    entry.insert(
        "BundlePath".to_string(),
        Value::String(AMFIPASS_BUNDLE_PATH.to_string()),
    );
    entry.insert(
        "Comment".to_string(),
        Value::String("AMFI pass-through, needed by NullMoth NVIDIA driver".to_string()),
    );
    entry.insert("Enabled".to_string(), Value::Boolean(true));
    entry.insert(
        "ExecutablePath".to_string(),
        Value::String("Contents/MacOS/AMFIPass".to_string()),
    );
    entry.insert(
        "PlistPath".to_string(),
        Value::String("Contents/Info.plist".to_string()),
    );
    entry.insert("Arch".to_string(), Value::String("Any".to_string()));
    entry
}

/// Ensures `Misc → Tools` carries the NullMothSafe picker entry ("1401:
/// Remove NVIDIA driver", Auxiliary) — the boot-picker recovery path from
/// INSTALL-GUIDE §3. Idempotent: an entry whose Path is already
/// `NullMothSafe.efi` is left untouched.
pub fn register_nullmoth_safe(cfg: &mut OcConfig) {
    let misc = dict_entry(&mut cfg.doc, "Misc");
    let tools = array_entry(misc, "Tools");
    let already = tools.iter().any(|entry| {
        entry
            .as_dictionary()
            .and_then(|d| d.get("Path"))
            .and_then(Value::as_string)
            == Some(NULLMOTH_SAFE_PATH)
    });
    if already {
        return;
    }

    let mut entry = Dictionary::new();
    entry.insert("Flavour".to_string(), Value::String("Auto".to_string()));
    entry.insert("FullNvramAccess".to_string(), Value::Boolean(false));
    entry.insert("Arguments".to_string(), Value::String(String::new()));
    entry.insert(
        "Comment".to_string(),
        Value::String("Remove the NullMoth NVIDIA driver and restore this config".to_string()),
    );
    entry.insert(
        "Path".to_string(),
        Value::String(NULLMOTH_SAFE_PATH.to_string()),
    );
    entry.insert("Enabled".to_string(), Value::Boolean(true));
    entry.insert(
        "Name".to_string(),
        Value::String("1401: Remove NVIDIA driver".to_string()),
    );
    entry.insert("Auxiliary".to_string(), Value::Boolean(true));
    entry.insert("RealPath".to_string(), Value::Boolean(false));
    entry.insert("TextMode".to_string(), Value::Boolean(false));
    tools.push(Value::Dictionary(entry));
}

/// True when applying `plan` would leave `cfg` byte-identical: no findings,
/// no boot-args to add, `csr-active-config` already set, SecureBootModel
/// already Disabled, every pinned quirk already at its value, and the
/// IONDRVSupport block already in place. (`plan.quirks` always lists the
/// pinned values, so their presence alone says nothing about the diff.)
pub fn plan_is_noop(cfg: &OcConfig, plan: &FixPlan) -> bool {
    if !plan.findings.is_empty()
        || !plan.boot_args_add.is_empty()
        || plan.csr_needs_set
        || plan.smbios_downgrade_needed
    {
        return false;
    }
    plan.quirks.iter().all(|quirk| quirk_matches(cfg, quirk)) && iondrv_blocked(cfg)
}

/// True when the value at `quirk.path` / `quirk.key` already equals
/// `quirk.value`.
fn quirk_matches(cfg: &OcConfig, quirk: &QuirkEdit) -> bool {
    let mut node = &cfg.doc;
    for segment in quirk.path.split('.') {
        let Some(next) = node.get(segment).and_then(Value::as_dictionary) else {
            return false;
        };
        node = next;
    }
    node.get(quirk.key) == Some(&quirk.value)
}

/// True when `Kernel → Block` already carries an enabled `Exclude` entry
/// for `com.apple.iokit.IONDRVSupport`.
fn iondrv_blocked(cfg: &OcConfig) -> bool {
    cfg.doc
        .get("Kernel")
        .and_then(Value::as_dictionary)
        .and_then(|kernel| kernel.get("Block"))
        .and_then(Value::as_array)
        .is_some_and(|block| {
            block.iter().any(|entry| {
                entry.as_dictionary().is_some_and(|d| {
                    d.get("Identifier").and_then(Value::as_string) == Some(IONDRV_SUPPORT)
                        && d.get("Strategy").and_then(Value::as_string) == Some("Exclude")
                        && d.get("Enabled").and_then(Value::as_boolean) == Some(true)
                })
            })
        })
}
