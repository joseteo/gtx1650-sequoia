//! Render findings for humans, or as JSON under `--json`.

use serde::Serialize;

use crate::detect::{Hardware, MachineState};
use crate::occonfig::{Finding, FindingKind};

/// The `--json` report: `{"hardware":…,"state":…,"findings":[…]}`.
#[derive(Serialize)]
pub struct DoctorReport<'a> {
    pub hardware: &'a Hardware,
    pub state: MachineState,
    pub findings: &'a [Finding],
}

/// Renders the human doctor report: the detected hardware, the
/// machine-state verdict, each finding with its plain-language fix, and
/// the next-step line for the verdict.
pub fn render_doctor(hw: &Hardware, state: &MachineState, findings: &[Finding]) -> String {
    let mut out = String::from("greenlight doctor\n\n");
    render_hardware(&mut out, hw);
    out.push('\n');
    render_verdict(&mut out, hw, state, findings.len());
    if !findings.is_empty() {
        out.push('\n');
        render_findings(&mut out, findings);
    }
    out.push('\n');
    render_next_step(&mut out, hw, state);
    out
}

fn render_hardware(out: &mut String, hw: &Hardware) {
    out.push_str("Hardware\n");
    match hw.macos_version {
        Some((major, minor, patch)) => {
            out.push_str(&format!("  macOS:  {major}.{minor}.{patch}\n"));
        }
        None => out.push_str("  macOS:  unknown\n"),
    }
    // SIP content is source-dependent (a csrutil phrase live, a hex mask in
    // fixtures) — opaque display text, never interpreted.
    let sip = hw.sip.as_deref().unwrap_or("unknown");
    out.push_str(&format!("  SIP:    {sip}\n"));
    if hw.pci.is_empty() {
        out.push_str("  PCI:    no devices detected\n");
    } else {
        out.push_str("  PCI:\n");
        for device in &hw.pci {
            let path = device.acpi_path.as_deref().unwrap_or("no ACPI path");
            out.push_str(&format!(
                "    {:04x}:{:04x}  {}  ({})\n",
                device.vendor, device.device, device.name, path
            ));
        }
    }
}

fn render_verdict(out: &mut String, hw: &Hardware, state: &MachineState, n_findings: usize) {
    let verdict = match state {
        MachineState::ReadyToFix => {
            if n_findings == 1 {
                "ready to fix — 1 config leftover blocks the NVIDIA driver".to_string()
            } else {
                format!("ready to fix — {n_findings} config leftovers block the NVIDIA driver")
            }
        }
        MachineState::NeedsSetup => {
            "needs setup — no NVIDIA-blocking leftovers, but the validated NullMoth setup is \
             not applied yet"
                .to_string()
        }
        MachineState::AlreadyWorking => {
            "already working — the OpenCore config has no NVIDIA-blocking leftovers".to_string()
        }
        MachineState::NoOpenCore => {
            "no OpenCore config found — there is nothing to scan".to_string()
        }
        MachineState::UnsupportedMacos => {
            // classify only yields this state when the version is known.
            let version = hw
                .macos_version
                .map(|(major, minor, patch)| format!("{major}.{minor}.{patch}"))
                .unwrap_or_else(|| "unknown".to_string());
            format!("unsupported macOS {version} — the driver payload needs macOS 15.5 or newer")
        }
        MachineState::NoNvidia => {
            "no NVIDIA display device detected — the NVIDIA fix does not apply to this machine"
                .to_string()
        }
    };
    out.push_str(&format!("Verdict: {verdict}\n"));
}

fn render_findings(out: &mut String, findings: &[Finding]) {
    out.push_str("Findings\n");
    for (i, finding) in findings.iter().enumerate() {
        out.push_str(&format!(
            "  {}. [{}] {}\n     fix: {}\n",
            i + 1,
            finding.location,
            finding.detail,
            fix_for(finding.kind)
        ));
    }
}

/// The plain-language fix for one finding kind — what `greenlight fix`
/// will do, spelled out for someone who never edited an OpenCore config.
fn fix_for(kind: FindingKind) -> &'static str {
    match kind {
        FindingKind::FakeClassCode => {
            "delete the fake `class-code` property from this DeviceProperties entry"
        }
        FindingKind::DisableGpu => "delete `disable-gpu` from this DeviceProperties entry",
        FindingKind::NvDisableArg => "remove `nv_disable=1` from boot-args",
        FindingKind::PikeraArg => "remove `agdpmod=pikera` from boot-args",
    }
}

fn render_next_step(out: &mut String, hw: &Hardware, state: &MachineState) {
    let next_step = match state {
        MachineState::ReadyToFix | MachineState::NeedsSetup => {
            if hw.is_validated_profile() {
                "run `greenlight fix` to apply the validated config changes and install the drivers"
                    .to_string()
            } else {
                // This machine is outside the validated profile; fix will
                // refuse unless the user explicitly overrides.
                "this machine is not the validated hardware profile (Turing dGPU + Intel iGPU), \
                 so `greenlight fix` will refuse unless you override it:\n  greenlight fix --force"
                    .to_string()
            }
        }
        MachineState::AlreadyWorking => "none — nothing to do".to_string(),
        MachineState::NoOpenCore => {
            "install OpenCore first (or point at it with --efi-path <PATH>); generating an EFI \
             from scratch (`greenlight forge`) is not yet implemented (planned for v2)"
                .to_string()
        }
        MachineState::UnsupportedMacos => {
            "none — upgrade to macOS 15.5 or newer first; report-only".to_string()
        }
        MachineState::NoNvidia => "none — report-only; there is no NVIDIA GPU to fix".to_string(),
    };
    out.push_str(&format!("Next step: {next_step}\n"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::PciDevice;

    /// The validated laptop profile: GTX 1650 dGPU + Intel iGPU.
    fn lenovo() -> Hardware {
        Hardware {
            pci: vec![
                PciDevice {
                    vendor: 0x10de,
                    device: 0x1f99,
                    class_code: 0x030000,
                    acpi_path: Some("PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)".to_string()),
                    name: "NVIDIA GeForce GTX 1650".to_string(),
                },
                PciDevice {
                    vendor: 0x8086,
                    device: 0x3e9b,
                    class_code: 0x030000,
                    acpi_path: Some("PciRoot(0x0)/Pci(0x2,0x0)".to_string()),
                    name: "Intel UHD Graphics 630".to_string(),
                },
            ],
            macos_version: Some((15, 7, 4)),
            sip: Some("0x0a43".to_string()),
        }
    }

    fn nv_disable_finding() -> Finding {
        Finding {
            kind: FindingKind::NvDisableArg,
            location: "boot-args".to_string(),
            detail: "nv_disable=1 tells macOS to ignore NVIDIA GPUs".to_string(),
        }
    }

    #[test]
    fn ready_to_fix_report_points_at_fix_command() {
        let hw = lenovo();
        let findings = vec![nv_disable_finding()];
        let out = render_doctor(&hw, &MachineState::ReadyToFix, &findings);
        assert!(out.contains("greenlight doctor"), "header:\n{out}");
        assert!(out.contains("NVIDIA GeForce GTX 1650"), "hardware:\n{out}");
        assert!(out.contains("15.7.4"), "macOS version:\n{out}");
        assert!(out.contains("0x0a43"), "SIP display text:\n{out}");
        assert!(out.contains("ready to fix"), "verdict:\n{out}");
        assert!(out.contains("1 config leftover blocks"), "count:\n{out}");
        assert!(out.contains("nv_disable=1"), "finding:\n{out}");
        assert!(out.contains("run `greenlight fix`"), "next step:\n{out}");
        assert!(
            !out.contains("--force"),
            "the validated profile gets no force hint:\n{out}"
        );
    }

    #[test]
    fn unvalidated_machine_gets_force_hint() {
        let mut hw = lenovo();
        hw.pci.retain(|d| d.vendor != 0x8086); // drop the iGPU
        assert!(!hw.is_validated_profile());
        let findings = vec![nv_disable_finding()];
        let out = render_doctor(&hw, &MachineState::ReadyToFix, &findings);
        assert!(out.contains("not the validated hardware profile"), "{out}");
        assert!(out.contains("greenlight fix --force"), "{out}");
    }

    #[test]
    fn already_working_report_has_no_findings_section() {
        let hw = lenovo();
        let out = render_doctor(&hw, &MachineState::AlreadyWorking, &[]);
        assert!(out.contains("already working"), "{out}");
        assert!(!out.contains("Findings"), "{out}");
        assert!(out.contains("nothing to do"), "{out}");
    }

    #[test]
    fn needs_setup_report_points_at_fix_command() {
        // A fresh OpenCore machine: nothing to remove, but the validated
        // NullMoth set is not applied — "already working" would be a lie.
        let hw = lenovo();
        let out = render_doctor(&hw, &MachineState::NeedsSetup, &[]);
        assert!(out.contains("needs setup"), "{out}");
        assert!(out.contains("not applied yet"), "{out}");
        assert!(!out.contains("already working"), "{out}");
        assert!(out.contains("run `greenlight fix`"), "{out}");
        assert!(
            !out.contains("--force"),
            "the validated profile gets no force hint:\n{out}"
        );
    }

    #[test]
    fn needs_setup_unvalidated_machine_gets_force_hint() {
        let mut hw = lenovo();
        hw.pci.retain(|d| d.vendor != 0x8086); // drop the iGPU
        assert!(!hw.is_validated_profile());
        let out = render_doctor(&hw, &MachineState::NeedsSetup, &[]);
        assert!(out.contains("needs setup"), "{out}");
        assert!(out.contains("not the validated hardware profile"), "{out}");
        assert!(out.contains("greenlight fix --force"), "{out}");
    }

    #[test]
    fn no_opencore_points_at_forge_v2() {
        let hw = lenovo();
        let out = render_doctor(&hw, &MachineState::NoOpenCore, &[]);
        assert!(out.contains("no OpenCore config found"), "{out}");
        assert!(out.contains("planned for v2"), "{out}");
        assert!(out.contains("--efi-path"), "{out}");
    }

    #[test]
    fn unsupported_macos_states_the_floor() {
        let mut hw = lenovo();
        hw.macos_version = Some((15, 4, 1));
        let out = render_doctor(&hw, &MachineState::UnsupportedMacos, &[]);
        assert!(out.contains("unsupported macOS 15.4.1"), "{out}");
        assert!(out.contains("15.5 or newer"), "{out}");
        assert!(out.contains("report-only"), "{out}");
    }

    #[test]
    fn no_nvidia_is_report_only() {
        let hw = lenovo();
        let out = render_doctor(&hw, &MachineState::NoNvidia, &[]);
        assert!(out.contains("no NVIDIA display device"), "{out}");
        assert!(out.contains("report-only"), "{out}");
        assert!(!out.contains("greenlight fix"), "{out}");
    }

    #[test]
    fn json_report_serializes_the_pinned_shape() {
        let hw = lenovo();
        let findings = vec![nv_disable_finding()];
        let report = DoctorReport {
            hardware: &hw,
            state: MachineState::ReadyToFix,
            findings: &findings,
        };
        let value: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
        assert_eq!(value["state"], "ReadyToFix");
        assert_eq!(
            value["hardware"]["macos_version"],
            serde_json::json!([15, 7, 4])
        );
        assert_eq!(value["hardware"]["sip"], "0x0a43");
        assert_eq!(value["findings"][0]["kind"], "NvDisableArg");
        assert_eq!(value["findings"][0]["location"], "boot-args");
    }
}
