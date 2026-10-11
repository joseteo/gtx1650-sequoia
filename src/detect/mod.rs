//! Hardware and OS detection: PCI devices, macOS version, SIP status.
//!
//! Probing lives behind [`HardwareSource`] so tests never touch the real
//! machine: [`FixtureSource`] replays a recorded dump (the Task 2 schema),
//! while [`MacosSource`] shells out to `ioreg` / `sw_vers` / `csrutil` on a
//! live Mac. On top of the raw facts, [`Hardware`] answers the two
//! questions the fix flow asks — where the NVIDIA display device sits in
//! ACPI ([`Hardware::nvidia_display_paths`]) and whether this machine is
//! the validated laptop profile ([`Hardware::is_validated_profile`]) — and
//! [`classify`] reduces hardware + config + scan findings to one
//! [`MachineState`].

use std::fs;
use std::io::{self, Cursor};
use std::path::PathBuf;
use std::process::{Command, Output};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::occonfig::{plan_fix, plan_is_noop, Finding, OcConfig};

/// PCI vendor ID of NVIDIA.
const VENDOR_NVIDIA: u16 = 0x10de;
/// PCI vendor ID of Intel.
const VENDOR_INTEL: u16 = 0x8086;
/// PCI base class of display controllers (VGA/3D/display).
const CLASS_DISPLAY: u32 = 0x03;
/// Oldest macOS the fix flow supports: Sequoia 15.5.
const MIN_SUPPORTED_MACOS: (u32, u32, u32) = (15, 5, 0);

/// PCI device IDs of the TU117M "GTX 1650 Mobile" family — the Turing
/// mobile silicon the validated profile covers (IDs per the public PCI ID
/// database). `0x1f99` is the GTX 1650 Mobile validated on the reference
/// laptop; the rest are the same card's other documented mobile variants.
/// Pascal/Ampere, desktop Turing, and the TU117M MX-class cards are
/// deliberately absent: machines with those fail the gate and need
/// `--force`.
const TURING_MOBILE_DEVICES: [u16; 7] = [
    0x1f91, // GTX 1650 Mobile / Max-Q
    0x1f92, // GTX 1650 Mobile
    0x1f94, // GTX 1650 Mobile
    0x1f95, // GTX 1650 Ti Mobile
    0x1f96, // GTX 1650 Mobile / Max-Q
    0x1f99, // GTX 1650 Mobile — the validated reference card
    0x1f9d, // GTX 1650 Mobile / Max-Q
];

/// Errors produced while probing hardware.
#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to read {}: {source}", .path.display())]
    Read { path: PathBuf, source: io::Error },

    #[error("failed to parse {}: {source}", .path.display())]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },

    #[error("failed to run `{command}`: {source}")]
    Command { command: String, source: io::Error },

    #[error("`{command}` failed: {detail}")]
    CommandFailed { command: String, detail: String },

    #[error("failed to parse `{command}` output: {source}")]
    CommandOutput {
        command: String,
        source: plist::Error,
    },

    #[error("`{command}` returned unexpected data: {detail}")]
    CommandShape { command: String, detail: String },
}

/// One PCI device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PciDevice {
    pub vendor: u16,
    pub device: u16,
    pub class_code: u32,
    pub acpi_path: Option<String>,
    pub name: String,
}

/// Everything the doctor/fix flow needs to know about this machine.
///
/// `macos_version` and `sip` are `None` when the source could not
/// determine them (a non-macOS host, or a fixture that omits them).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hardware {
    pub pci: Vec<PciDevice>,
    pub macos_version: Option<(u32, u32, u32)>,
    pub sip: Option<String>,
}

impl Hardware {
    /// ACPI paths of the NVIDIA display devices, in the OpenCore
    /// `PciRoot(0x0)/Pci(0x1,0x0)/…` form that `DeviceProperties` uses as
    /// keys. A device must be vendor `0x10de` **and** display class
    /// (`0x03xxxx`) and have a known ACPI path — the NVIDIA HDAU audio
    /// function (class `0x04xxxx`) never appears here.
    pub fn nvidia_display_paths(&self) -> Vec<String> {
        self.pci
            .iter()
            .filter(|d| is_nvidia_display(d))
            .filter_map(|d| d.acpi_path.clone())
            .collect()
    }

    /// The validated laptop profile: a pinned Turing mobile dGPU
    /// ([`TURING_MOBILE_DEVICES`]) **and** an Intel (`0x8086`) display
    /// device — the TU117M dGPU beside the Intel iGPU that keeps the
    /// internal panel alive. The device-ID pin is deliberate: the driver
    /// only supports Turing, so a Pascal or Ampere laptop fails this gate
    /// and needs `--force`.
    pub fn is_validated_profile(&self) -> bool {
        self.pci.iter().any(|d| {
            d.vendor == VENDOR_NVIDIA && is_display(d) && TURING_MOBILE_DEVICES.contains(&d.device)
        }) && self
            .pci
            .iter()
            .any(|d| d.vendor == VENDOR_INTEL && is_display(d))
    }
}

/// Display-class check on the class code's top byte (`0x03xxxx`).
fn is_display(device: &PciDevice) -> bool {
    (device.class_code >> 16) & 0xFF == CLASS_DISPLAY
}

fn is_nvidia_display(device: &PciDevice) -> bool {
    device.vendor == VENDOR_NVIDIA && is_display(device)
}

/// What state this machine is in. [`classify`] checks in a fixed order;
/// the first match wins:
///
/// 1. [`NoNvidia`](MachineState::NoNvidia) — no NVIDIA display device at
///    all; the NVIDIA fix is pointless on this machine whatever the
///    config says.
/// 2. [`UnsupportedMacos`](MachineState::UnsupportedMacos) — macOS older
///    than 15.5, which the driver payload does not support. An unknown
///    version (`None`) is never "unsupported".
/// 3. [`NoOpenCore`](MachineState::NoOpenCore) — no `config.plist` was
///    loaded, so there is nothing to scan or fix.
/// 4. With a config in hand: findings non-empty →
///    [`ReadyToFix`](MachineState::ReadyToFix). Findings empty alone is
///    not "working": a fresh OpenCore config has nothing to remove but
///    none of the validated NullMoth set either, so zero findings split
///    into [`AlreadyWorking`](MachineState::AlreadyWorking) (the fix plan
///    would be a no-op — the validated set is fully applied) versus
///    [`NeedsSetup`](MachineState::NeedsSetup) (otherwise — the machine
///    still needs `greenlight fix`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MachineState {
    ReadyToFix,
    NeedsSetup,
    AlreadyWorking,
    NoOpenCore,
    UnsupportedMacos,
    NoNvidia,
}

/// Reduces hardware, an optionally loaded config, and that config's scan
/// findings to the machine's state — see [`MachineState`] for the check
/// order. When `cfg` is `Some`, `findings` is trusted as-is; callers pass
/// `scan(cfg, &hw.nvidia_display_paths())`.
pub fn classify(hw: &Hardware, cfg: Option<&OcConfig>, findings: &[Finding]) -> MachineState {
    if !hw.pci.iter().any(is_nvidia_display) {
        return MachineState::NoNvidia;
    }
    if matches!(hw.macos_version, Some(v) if v < MIN_SUPPORTED_MACOS) {
        return MachineState::UnsupportedMacos;
    }
    let Some(cfg) = cfg else {
        return MachineState::NoOpenCore;
    };
    if !findings.is_empty() {
        return MachineState::ReadyToFix;
    }
    // No leftovers is not necessarily "working": only a config whose fix
    // plan is a no-op has the validated NullMoth set actually applied.
    let plan = plan_fix(cfg, &hw.nvidia_display_paths(), false);
    if plan_is_noop(cfg, &plan) {
        MachineState::AlreadyWorking
    } else {
        MachineState::NeedsSetup
    }
}

/// A source of [`Hardware`] facts. v1 ships [`FixtureSource`] (recorded
/// dump — the offline test seam) and [`MacosSource`] (live probe).
pub trait HardwareSource {
    fn load(&self) -> Result<Hardware, Error>;
}

/// Reads a recorded hardware dump (Task 2 schema):
/// `{ "pci": [ { "vendor": "10de", "device": "1f99", "class": "0x030000",
/// "acpi_path": "…", "name": "…" }, … ], "macos": "15.7.4", "sip":
/// "0x0a43" }`. Vendor/device/class are hex strings; a `0x` prefix is
/// optional.
pub struct FixtureSource {
    pub json_path: PathBuf,
}

impl HardwareSource for FixtureSource {
    fn load(&self) -> Result<Hardware, Error> {
        let text = fs::read_to_string(&self.json_path).map_err(|source| Error::Read {
            path: self.json_path.clone(),
            source,
        })?;
        let raw: FixtureHardware = serde_json::from_str(&text).map_err(|source| Error::Parse {
            path: self.json_path.clone(),
            source,
        })?;
        Ok(Hardware {
            pci: raw
                .pci
                .into_iter()
                .map(|d| PciDevice {
                    vendor: d.vendor,
                    device: d.device,
                    class_code: d.class_code,
                    acpi_path: d.acpi_path,
                    name: d.name,
                })
                .collect(),
            macos_version: raw.macos.as_deref().and_then(parse_version),
            sip: raw.sip,
        })
    }
}

/// The Task 2 fixture schema, as JSON. Hex strings are decoded during
/// deserialization so malformed values surface as fixture parse errors.
#[derive(Deserialize)]
struct FixtureHardware {
    #[serde(default)]
    pci: Vec<FixturePci>,
    macos: Option<String>,
    sip: Option<String>,
}

#[derive(Deserialize)]
struct FixturePci {
    #[serde(deserialize_with = "hex_u16")]
    vendor: u16,
    #[serde(deserialize_with = "hex_u16")]
    device: u16,
    #[serde(rename = "class", deserialize_with = "hex_u32")]
    class_code: u32,
    acpi_path: Option<String>,
    #[serde(default)]
    name: String,
}

fn hex_u16<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u16, D::Error> {
    let s = String::deserialize(d)?;
    u16::from_str_radix(hex_digits(&s), 16).map_err(serde::de::Error::custom)
}

fn hex_u32<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let s = String::deserialize(d)?;
    u32::from_str_radix(hex_digits(&s), 16).map_err(serde::de::Error::custom)
}

/// Strips an optional `0x`/`0X` prefix so both `"10de"` and `"0x030000"`
/// parse.
fn hex_digits(s: &str) -> &str {
    let s = s.trim();
    s.strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s)
}

/// Parses `"15.7.4"` or `"15.7"` into `(major, minor, patch)`, missing
/// trailing components defaulting to 0. Returns `None` when the string is
/// not dotted numbers.
fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// The live macOS probe: PCI devices from `ioreg -r -c IOPCIDevice -a`
/// (plist XML on stdout), the OS version from `sw_vers -productVersion`,
/// and the SIP status from `csrutil status`.
///
/// Never exercised in integration tests (offline constraint); the pure
/// parsers it feeds — [`pci_devices_from_ioreg`], [`opencore_acpi_path`],
/// [`parse_version`], [`sip_from_csrutil`] — are unit-tested below.
pub struct MacosSource;

impl HardwareSource for MacosSource {
    fn load(&self) -> Result<Hardware, Error> {
        Ok(Hardware {
            pci: ioreg_pci_devices()?,
            macos_version: sw_vers_product_version()?,
            sip: csrutil_sip()?,
        })
    }
}

const IOREG_PCI: &str = "ioreg -r -c IOPCIDevice -a";
const SW_VERS: &str = "sw_vers -productVersion";
const CSRUTIL_STATUS: &str = "csrutil status";

fn ioreg_pci_devices() -> Result<Vec<PciDevice>, Error> {
    let output = run("ioreg", &["-r", "-c", "IOPCIDevice", "-a"], IOREG_PCI)?;
    let value = plist::Value::from_reader(Cursor::new(&output.stdout)).map_err(|source| {
        Error::CommandOutput {
            command: IOREG_PCI.to_string(),
            source,
        }
    })?;
    let entries = value.as_array().ok_or_else(|| Error::CommandShape {
        command: IOREG_PCI.to_string(),
        detail: "root is not an array".to_string(),
    })?;
    Ok(pci_devices_from_ioreg(entries))
}

fn sw_vers_product_version() -> Result<Option<(u32, u32, u32)>, Error> {
    let output = run("sw_vers", &["-productVersion"], SW_VERS)?;
    Ok(parse_version(&String::from_utf8_lossy(&output.stdout)))
}

fn csrutil_sip() -> Result<Option<String>, Error> {
    let output = run("csrutil", &["status"], CSRUTIL_STATUS)?;
    Ok(sip_from_csrutil(&String::from_utf8_lossy(&output.stdout)))
}

/// Maps `ioreg -a` entries to [`PciDevice`]s, skipping entries without a
/// usable vendor/device/class triple. `acpi-path` values arrive in
/// IOACPIPlane form and are converted to OpenCore form; devices whose
/// path does not convert keep `acpi_path: None`.
fn pci_devices_from_ioreg(entries: &[plist::Value]) -> Vec<PciDevice> {
    entries
        .iter()
        .filter_map(plist::Value::as_dictionary)
        .filter_map(|d| {
            let vendor = le_u16(d.get("vendor-id")?)?;
            let device = le_u16(d.get("device-id")?)?;
            let class_code = le_u32(d.get("class-code")?)? & 0x00FF_FFFF;
            let acpi_path = d
                .get("acpi-path")
                .and_then(plist::Value::as_string)
                .and_then(opencore_acpi_path);
            let name = d
                .get("model")
                .or_else(|| d.get("name"))
                .map(stringy)
                .unwrap_or_default();
            Some(PciDevice {
                vendor,
                device,
                class_code,
                acpi_path,
                name,
            })
        })
        .collect()
}

/// Reads a little-endian `Data` scalar — ioreg encodes `vendor-id` /
/// `device-id` as 4-byte data, of which the low two bytes are the ID.
fn le_u16(value: &plist::Value) -> Option<u16> {
    let bytes = value.as_data()?;
    Some(u16::from_le_bytes([*bytes.first()?, *bytes.get(1)?]))
}

/// Reads a little-endian `Data` scalar — ioreg encodes `class-code` as
/// 4-byte data (the class code proper is the low three bytes).
fn le_u32(value: &plist::Value) -> Option<u32> {
    let bytes = value.as_data()?;
    Some(u32::from_le_bytes([
        *bytes.first()?,
        *bytes.get(1)?,
        *bytes.get(2)?,
        *bytes.get(3)?,
    ]))
}

/// Renders an ioreg name-ish property: strings pass through, data is
/// decoded as UTF-8 with NUL padding trimmed.
fn stringy(value: &plist::Value) -> String {
    match value {
        plist::Value::String(s) => s.trim().to_string(),
        plist::Value::Data(bytes) => String::from_utf8_lossy(bytes)
            .trim_matches('\0')
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

/// Converts an ioreg `acpi-path` like
/// `IOACPIPlane:/_SB/PCI0@0/PEG0@10000/EGP0@0` to OpenCore form
/// `PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)`. The number after `@` packs
/// `(device << 16) | function`. Returns `None` for anything that does not
/// fit that shape — a partial path is worse than no path, since the
/// result keys `DeviceProperties` entries.
fn opencore_acpi_path(ioreg_path: &str) -> Option<String> {
    let path = ioreg_path.strip_prefix("IOACPIPlane:/")?;
    let mut segments = path.split('/').filter(|s| !s.is_empty());

    // Skip leading namespace segments ("_SB") up to the PCI root bridge.
    let found_root = segments.any(|s| match s.split_once('@') {
        Some((name, addr)) => name.starts_with("PCI") && addr == "0",
        None => false,
    });
    if !found_root {
        return None;
    }

    let mut out = "PciRoot(0x0)".to_string();
    for segment in segments {
        let (_, addr) = segment.split_once('@')?;
        let packed = u32::from_str_radix(addr, 16).ok()?;
        out.push_str(&format!(
            "/Pci(0x{:x},0x{:x})",
            packed >> 16,
            packed & 0xffff
        ));
    }
    Some(out)
}

/// Extracts the SIP status phrase from `csrutil status` output:
/// `"System Integrity Protection status: disabled."` → `Some("disabled")`.
fn sip_from_csrutil(stdout: &str) -> Option<String> {
    let line = stdout.lines().next()?.trim();
    let status = line
        .strip_prefix("System Integrity Protection status:")?
        .trim()
        .trim_end_matches('.');
    if status.is_empty() {
        None
    } else {
        Some(status.to_string())
    }
}

/// Runs a probe command; spawn failure and non-zero exit are errors.
fn run(bin: &str, args: &[&str], label: &str) -> Result<Output, Error> {
    let output = Command::new(bin)
        .args(args)
        .output()
        .map_err(|source| Error::Command {
            command: label.to_string(),
            source,
        })?;
    if !output.status.success() {
        return Err(Error::CommandFailed {
            command: label.to_string(),
            detail: stderr_of(&output),
        });
    }
    Ok(output)
}

/// Returns trimmed stderr for diagnostics, falling back to stdout when
/// the failure was reported there.
fn stderr_of(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        stderr
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ioreg_entry(
        vendor: [u8; 4],
        device: [u8; 4],
        class: [u8; 4],
        acpi_path: Option<&str>,
        model: Option<&str>,
    ) -> plist::Value {
        let mut d = plist::Dictionary::new();
        d.insert("vendor-id".to_string(), plist::Value::Data(vendor.to_vec()));
        d.insert("device-id".to_string(), plist::Value::Data(device.to_vec()));
        d.insert("class-code".to_string(), plist::Value::Data(class.to_vec()));
        if let Some(path) = acpi_path {
            d.insert(
                "acpi-path".to_string(),
                plist::Value::String(path.to_string()),
            );
        }
        if let Some(model) = model {
            let mut bytes = model.as_bytes().to_vec();
            bytes.push(0); // ioreg NUL-pads string data
            d.insert("model".to_string(), plist::Value::Data(bytes));
        }
        plist::Value::Dictionary(d)
    }

    #[test]
    fn parses_dotted_versions() {
        assert_eq!(parse_version("15.7.4"), Some((15, 7, 4)));
        assert_eq!(parse_version("15.7"), Some((15, 7, 0)));
        assert_eq!(parse_version(" 15.7.4\n"), Some((15, 7, 4)));
        assert_eq!(parse_version("fifteen"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn converts_ioreg_acpi_paths() {
        assert_eq!(
            opencore_acpi_path("IOACPIPlane:/_SB/PCI0@0/PEG0@10000/EGP0@0"),
            Some("PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)".to_string())
        );
        // A non-zero function number lands in the second Pci slot.
        assert_eq!(
            opencore_acpi_path("IOACPIPlane:/_SB/PCI0@0/PEG0@10000/HDEF@1"),
            Some("PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x1)".to_string())
        );
        // Anything off-shape yields no path rather than a wrong one.
        assert_eq!(opencore_acpi_path("IOACPIPlane:/_SB"), None);
        assert_eq!(opencore_acpi_path("PCI0@0/PEG0@10000"), None);
        assert_eq!(opencore_acpi_path("IOACPIPlane:/_SB/PCI0@0/PEG0"), None);
    }

    #[test]
    fn parses_ioreg_entries() {
        let entries = vec![
            ioreg_entry(
                [0xde, 0x10, 0x00, 0x00], // vendor 0x10de
                [0x99, 0x1f, 0x00, 0x00], // device 0x1f99
                [0x00, 0x00, 0x03, 0x00], // class 0x030000
                Some("IOACPIPlane:/_SB/PCI0@0/PEG0@10000/EGP0@0"),
                Some("NVIDIA GeForce GTX 1650"),
            ),
            // HDAU audio function: parsed, but not a display device.
            ioreg_entry(
                [0xde, 0x10, 0x00, 0x00],
                [0xfa, 0x10, 0x00, 0x00], // device 0x10fa
                [0x00, 0x03, 0x04, 0x00], // class 0x040300
                Some("IOACPIPlane:/_SB/PCI0@0/PEG0@10000/HDAU@1"),
                None,
            ),
            // Entries lacking the ID triple are skipped, not fatal.
            plist::Value::Dictionary(plist::Dictionary::new()),
        ];
        let devices = pci_devices_from_ioreg(&entries);
        assert_eq!(devices.len(), 2);
        assert_eq!(
            devices[0],
            PciDevice {
                vendor: 0x10de,
                device: 0x1f99,
                class_code: 0x030000,
                acpi_path: Some("PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)".to_string()),
                name: "NVIDIA GeForce GTX 1650".to_string(),
            }
        );
        assert_eq!(devices[1].class_code, 0x040300);
        assert_eq!(devices[1].name, "");
    }

    #[test]
    fn parses_csrutil_status() {
        assert_eq!(
            sip_from_csrutil("System Integrity Protection status: disabled.\n"),
            Some("disabled".to_string())
        );
        assert_eq!(
            sip_from_csrutil("System Integrity Protection status: enabled.\n"),
            Some("enabled".to_string())
        );
        assert_eq!(sip_from_csrutil("garbage"), None);
    }
}
