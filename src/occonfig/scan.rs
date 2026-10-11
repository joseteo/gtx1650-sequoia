//! Breakage detection: finds the config leftovers that silently kill NVIDIA
//! GPUs — the dortania `class-code=<FFFFFFFF>` fake-out, `disable-gpu`, and
//! the `nv_disable=1` / `agdpmod=pikera` boot-args.

use plist::Value;
use serde::Serialize;

use super::OcConfig;

/// The kinds of breakage [`scan`] can find.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum FindingKind {
    /// `class-code` with an all-0xFF payload on an NVIDIA DeviceProperties
    /// path — masks the real PCI class code so macOS drivers never match.
    FakeClassCode,
    /// `disable-gpu` present on an NVIDIA DeviceProperties path, regardless
    /// of payload width (real configs carry both `01` and `01 00 00 00`).
    DisableGpu,
    /// `nv_disable=1` in boot-args.
    NvDisableArg,
    /// `agdpmod=pikera` in boot-args.
    PikeraArg,
}

/// One detected breakage: what it is, where it lives, and the offending
/// value in human-readable form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub kind: FindingKind,
    pub location: String,
    pub detail: String,
}

/// Scans `cfg` for NVIDIA-killing leftovers.
///
/// DeviceProperties findings are reported only for entries whose path is in
/// `nvidia_paths`; identical properties on any other path are ignored.
/// Boot-args findings are global.
pub fn scan(cfg: &OcConfig, nvidia_paths: &[String]) -> Vec<Finding> {
    let mut findings = Vec::new();

    if let Some(add) = cfg.device_properties() {
        for (path, entry) in add {
            if !nvidia_paths.iter().any(|p| p == path) {
                continue;
            }
            let Some(props) = entry.as_dictionary() else {
                continue;
            };
            if let Some(value) = props.get("class-code") {
                if let Some(detail) = fake_class_code_detail(value) {
                    findings.push(Finding {
                        kind: FindingKind::FakeClassCode,
                        location: path.clone(),
                        detail,
                    });
                }
            }
            if let Some(value) = props.get("disable-gpu") {
                findings.push(Finding {
                    kind: FindingKind::DisableGpu,
                    location: path.clone(),
                    detail: format!(
                        "disable-gpu is present ({}), telling WhateverGreen to disable this GPU",
                        describe(value)
                    ),
                });
            }
        }
    }

    let args = cfg.boot_args();
    if args.contains("nv_disable=1") {
        findings.push(Finding {
            kind: FindingKind::NvDisableArg,
            location: "boot-args".to_string(),
            detail: "nv_disable=1 tells macOS to ignore NVIDIA GPUs".to_string(),
        });
    }
    if args.contains("agdpmod=pikera") {
        findings.push(Finding {
            kind: FindingKind::PikeraArg,
            location: "boot-args".to_string(),
            detail: "agdpmod=pikera patches board-id checks meant for AMD Navi setups".to_string(),
        });
    }

    findings
}

/// Returns a detail string when `value` is a fake all-0xFF class code, or
/// `None` when it is a genuine value. Accepts `Data` of 4 or 8 bytes of
/// 0xFF, and defensively the string forms "FFFFFFFF" / "FFFFFFFFFFFFFFFF".
fn fake_class_code_detail(value: &Value) -> Option<String> {
    if let Some(bytes) = value.as_data() {
        if matches!(bytes.len(), 4 | 8) && bytes.iter().all(|b| *b == 0xFF) {
            return Some(format!(
                "class-code is {} bytes of 0xFF, masking the real PCI class code",
                bytes.len()
            ));
        }
    }
    if let Some(s) = value.as_string() {
        if matches!(s.len(), 8 | 16) && s.bytes().all(|b| matches!(b, b'F' | b'f')) {
            return Some(format!(
                "class-code is the string \"{s}\", a fake class code"
            ));
        }
    }
    None
}

/// Renders a plist scalar compactly for `Finding::detail`.
fn describe(value: &Value) -> String {
    match value {
        Value::Data(bytes) => format!("data 0x{}", hex::encode(bytes)),
        Value::String(s) => format!("string \"{s}\""),
        Value::Boolean(b) => format!("boolean {b}"),
        Value::Integer(i) => format!("integer {i}"),
        _ => "a non-scalar value".to_string(),
    }
}
