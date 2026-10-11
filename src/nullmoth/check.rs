//! Post-reboot verification (`greenlight fix --check`): probes the live
//! system through the [`Shell`] seam and proves the NullMoth driver stack
//! came up — the four `com.nullmoth.*` kexts loaded and the NVIDIA GPU
//! claimed, with the display and Metal level as published proof.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use super::Error;

/// The four NullMoth kexts a working post-reboot system must have loaded
/// (bundle ids exactly as `kextstat` reports them).
pub const REQUIRED_KEXTS: [&str; 4] = [
    "com.nullmoth.NVRM",
    "com.nullmoth.NVAccel",
    "com.nullmoth.NVRMFB",
    "com.nullmoth.NVRMAGDC",
];

/// Runs the unprivileged probes [`run_check`] parses — the seam that keeps
/// the check testable offline.
pub trait Shell {
    fn run(&self, cmd: &str, args: &[&str]) -> Result<String, Error>;
}

/// The real shell: runs the command and returns stdout as a `String`.
pub struct SystemShell;

impl Shell for SystemShell {
    fn run(&self, cmd: &str, args: &[&str]) -> Result<String, Error> {
        let command = command_line(cmd, args);
        let output = Command::new(cmd)
            .args(args)
            .output()
            .map_err(|source| Error::Command {
                command: command.clone(),
                source,
            })?;
        if !output.status.success() {
            return Err(Error::CommandFailed {
                command,
                detail: format!("exit status {}", output.status),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// Canned-output shell for the `GREENLIGHT_FIXTURE_SHELL_DIR` seam: reads
/// `kextstat.txt` / `displays.txt` from a directory instead of probing.
/// Dev/test-only — never set this for a real run.
pub struct FixtureShell {
    dir: PathBuf,
}

impl FixtureShell {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
}

impl Shell for FixtureShell {
    fn run(&self, cmd: &str, _args: &[&str]) -> Result<String, Error> {
        let file = match cmd {
            "kextstat" => "kextstat.txt",
            "system_profiler" => "displays.txt",
            other => {
                return Err(Error::CommandFailed {
                    command: other.to_string(),
                    detail: "no fixture for this command".to_string(),
                })
            }
        };
        let path = self.dir.join(file);
        fs::read_to_string(&path).map_err(|source| Error::Command {
            command: format!("read {}", path.display()),
            source,
        })
    }
}

/// What the probes found — the parsed proof.
pub struct CheckReport {
    /// The [`REQUIRED_KEXTS`] bundle ids actually loaded, in required order.
    pub kexts_loaded: Vec<String>,
    /// The driver claimed the NVIDIA GPU (an NVIDIA section exists in
    /// `system_profiler SPDisplaysDataType`).
    pub gpu_claimed: bool,
    /// One entry per NVIDIA-driven display: name, resolution, refresh.
    pub displays: Vec<String>,
    /// The Metal level of the NVIDIA GPU (e.g. "Metal 3").
    pub metal: Option<String>,
}

impl CheckReport {
    /// The required kexts that are not loaded (empty when all four are).
    pub fn missing_kexts(&self) -> Vec<&'static str> {
        REQUIRED_KEXTS
            .iter()
            .filter(|id| !self.kexts_loaded.iter().any(|k| k == *id))
            .copied()
            .collect()
    }

    /// The exit contract: pass iff all four kexts loaded and the GPU is
    /// claimed.
    pub fn passed(&self) -> bool {
        self.missing_kexts().is_empty() && self.gpu_claimed
    }
}

/// Probes via `sh` and parses the results. A failing probe degrades to an
/// empty/false slice of the report — the rendered table then shows the
/// failure instead of hiding it behind an error.
pub fn run_check(sh: &dyn Shell) -> CheckReport {
    let kextstat = sh.run("kextstat", &[]).unwrap_or_default();
    let displays = sh
        .run("system_profiler", &["SPDisplaysDataType"])
        .unwrap_or_default();
    let (gpu_claimed, display_list, metal) = parse_displays(&displays);
    CheckReport {
        kexts_loaded: parse_kextstat(&kextstat),
        gpu_claimed,
        displays: display_list,
        metal,
    }
}

/// The [`REQUIRED_KEXTS`] found in `kextstat` output: the bundle id is the
/// sixth whitespace-separated column of a loaded-kext row.
fn parse_kextstat(output: &str) -> Vec<String> {
    REQUIRED_KEXTS
        .iter()
        .filter(|id| {
            output
                .lines()
                .any(|line| line.split_whitespace().nth(5) == Some(*id))
        })
        .map(|id| id.to_string())
        .collect()
}

/// Parses `system_profiler SPDisplaysDataType` output: whether an NVIDIA GPU
/// section exists (the driver claimed the card), one rendered entry per
/// NVIDIA-driven display ("NAME — RESOLUTION @ REFRESH"), and the Metal
/// level. All three come from the NVIDIA section only — the Intel iGPU also
/// reports displays and Metal 3.
fn parse_displays(output: &str) -> (bool, Vec<String>, Option<String>) {
    let Some(section) = nvidia_section(output) else {
        return (false, Vec::new(), None);
    };
    let mut metal = None;
    let mut displays = Vec::new();
    let mut name: Option<String> = None;
    let mut resolution: Option<String> = None;
    let mut refresh: Option<String> = None;
    for line in section {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("Metal Support:") {
            metal = Some(value.trim().to_string());
        } else if is_display_header(line) {
            if let Some(entry) = render_display(name.take(), resolution.take(), refresh.take()) {
                displays.push(entry);
            }
            name = Some(trimmed.trim_end_matches(':').to_string());
        } else if let Some(value) = trimmed.strip_prefix("Resolution:") {
            // "2560 x 1440 (QHD/WQHD - Wide Quad High Definition)" — the
            // parenthetical is marketing text, drop it.
            resolution = Some(value.split('(').next().unwrap_or("").trim().to_string());
        } else if let Some(value) = trimmed.strip_prefix("UI Looks like:") {
            // "2560 x 1440 @ 144.00Hz" — the only line carrying the refresh.
            if let Some((_, hz)) = value.split_once('@') {
                refresh = Some(hz.trim().to_string());
            }
        }
    }
    if let Some(entry) = render_display(name, resolution, refresh) {
        displays.push(entry);
    }
    (true, displays, metal)
}

/// The lines of the NVIDIA GPU section, header included. GPU section
/// headers sit at 4-space indent ("    NVIDIA GeForce GTX 1650:"); the
/// section ends at the next non-empty line indented less than its keys
/// (6 spaces).
fn nvidia_section(output: &str) -> Option<Vec<&str>> {
    let lines: Vec<&str> = output.lines().collect();
    let start = lines.iter().position(|line| {
        line.starts_with("    ")
            && !line.starts_with("      ")
            && line.trim_end().ends_with(':')
            && line.contains("NVIDIA")
    })?;
    let end = lines[start + 1..]
        .iter()
        .position(|line| !line.is_empty() && !line.starts_with("      "))
        .map(|i| start + 1 + i)
        .unwrap_or(lines.len());
    Some(lines[start..end].to_vec())
}

/// Display headers sit at 8-space indent inside the GPU section
/// ("        GS27QA:").
fn is_display_header(line: &str) -> bool {
    line.starts_with("        ")
        && !line.starts_with("          ")
        && line.trim_end().ends_with(':')
}

/// One display row: "GS27QA — 2560 x 1440 @ 144.00Hz", degrading gracefully
/// when a value is absent from the probe output.
fn render_display(
    name: Option<String>,
    resolution: Option<String>,
    refresh: Option<String>,
) -> Option<String> {
    let mut entry = name?;
    if let Some(resolution) = resolution {
        entry.push_str(" — ");
        entry.push_str(&resolution);
        if let Some(refresh) = refresh {
            entry.push_str(" @ ");
            entry.push_str(&refresh);
        }
    }
    Some(entry)
}

/// The command line as humans type it, for error messages.
fn command_line(cmd: &str, args: &[&str]) -> String {
    std::iter::once(cmd)
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Renders the proof table (README-style rows: kexts loaded n/4, GPU
/// claimed, display + resolution@Hz, Metal level) and the PASS/FAIL verdict
/// with what failed.
pub fn render_check(report: &CheckReport) -> String {
    let mut out = String::from("greenlight fix --check\n\nProof\n");
    let missing = report.missing_kexts();
    let (loaded, total) = (report.kexts_loaded.len(), REQUIRED_KEXTS.len());
    if missing.is_empty() {
        out.push_str(&format!(
            "  kexts loaded:  {loaded}/{total} ({})\n",
            report.kexts_loaded.join(", ")
        ));
    } else {
        out.push_str(&format!(
            "  kexts loaded:  {loaded}/{total} (missing: {})\n",
            missing.join(", ")
        ));
    }
    out.push_str(&format!(
        "  GPU claimed:   {}\n",
        if report.gpu_claimed { "yes" } else { "no" }
    ));
    match report.displays.first() {
        None => out.push_str("  display:       none detected\n"),
        Some(first) => {
            out.push_str(&format!("  display:       {first}\n"));
            for display in &report.displays[1..] {
                out.push_str(&format!("                 {display}\n"));
            }
        }
    }
    out.push_str(&format!(
        "  Metal:         {}\n",
        report.metal.as_deref().unwrap_or("unknown")
    ));
    out.push('\n');
    if report.passed() {
        out.push_str("PASS — the NullMoth driver stack is live.\n");
    } else {
        let mut failed = Vec::new();
        if !missing.is_empty() {
            failed.push(format!("kexts not loaded: {}", missing.join(", ")));
        }
        if !report.gpu_claimed {
            failed.push("GPU not claimed (no NVIDIA section in system_profiler)".to_string());
        }
        out.push_str(&format!(
            "FAIL — the driver stack is not live: {}.\n",
            failed.join("; ")
        ));
    }
    out
}
