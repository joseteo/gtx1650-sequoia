//! greenlight — automate NVIDIA-on-OpenCore setup on macOS Hackintoshes.

use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use greenlight::detect::{self, FixtureSource, HardwareSource, MachineState, MacosSource};
use greenlight::esp::{self, MacosEspMounter};
use greenlight::fetch::{self, FixtureHttp, Http, UreqHttp};
use greenlight::nullmoth;
use greenlight::nullmoth::check::{self, FixtureShell, Shell, SystemShell};
use greenlight::occonfig::{
    amfipass_registered, apply_fix, plan_fix, plan_is_noop, register_amfipass,
    register_nullmoth_safe, scan, Finding, FindingKind, FixPlan, OcConfig,
};
use greenlight::report::{self, DoctorReport};

#[derive(Parser)]
#[command(name = "greenlight", version, about)]
struct Cli {
    /// Emit machine-readable JSON output.
    #[arg(long, global = true)]
    json: bool,

    /// Assume "yes" at confirmation prompts.
    #[arg(long, global = true)]
    yes: bool,

    /// Operate on the OpenCore EFI at PATH instead of auto-detecting it.
    #[arg(long, global = true, value_name = "PATH")]
    efi_path: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Inspect this machine and its OpenCore config; report what needs changing.
    Doctor,
    /// Apply the validated OpenCore config changes and install the drivers.
    Fix(FixArgs),
    /// Generate a complete OpenCore EFI for this machine (planned for v2).
    Forge,
}

#[derive(clap::Args)]
struct FixArgs {
    /// Apply the changes even though this machine is not the validated
    /// hardware profile (Turing dGPU + Intel iGPU).
    #[arg(long)]
    force: bool,

    /// Print the planned changes and commands without writing anything.
    #[arg(long)]
    dry_run: bool,

    /// Post-reboot: verify the driver stack is live and print the proof
    /// table. Runs only the check — the remediation flow is skipped.
    #[arg(long)]
    check: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match &cli.command {
        Command::Doctor => doctor(&cli),
        Command::Fix(args) => fix(&cli, args),
        Command::Forge => {
            eprintln!("forge: not yet implemented (planned for v2)");
            ExitCode::from(2)
        }
    }
}

/// `greenlight doctor`: probe hardware, locate the boot OpenCore config,
/// scan it, classify the machine, render the report. Read-only — every
/// verdict exits 0; only operational failures (a probe error, an explicit
/// `--efi-path` without a config, an unreadable config) exit non-zero.
fn doctor(cli: &Cli) -> ExitCode {
    let hw = match hardware_source().load() {
        Ok(hw) => hw,
        Err(err) => {
            eprintln!("doctor: hardware probe failed: {err}");
            return ExitCode::FAILURE;
        }
    };

    // A config that cannot be located is never fatal to a diagnostic: zero
    // configs is the NoOpenCore verdict, several is a warning.
    let mut warnings = Vec::new();
    let mut cfg = None;
    let mut findings = Vec::new();
    match esp::find_oc_configs(cli.efi_path.as_deref(), &MacosEspMounter) {
        Ok(configs) => {
            let path = configs
                .into_iter()
                .next()
                .expect("find_oc_configs returns exactly one config on Ok");
            match OcConfig::load(&path) {
                Ok(loaded) => {
                    findings = scan(&loaded, &hw.nvidia_display_paths());
                    cfg = Some(loaded);
                }
                Err(err) => {
                    eprintln!("doctor: {err}");
                    return ExitCode::FAILURE;
                }
            }
        }
        Err(esp::Error::NoOpenCore) => {}
        Err(err @ esp::Error::Ambiguous(_)) => warnings.push(err.to_string()),
        Err(err) => {
            eprintln!("doctor: {err}");
            return ExitCode::FAILURE;
        }
    }

    let state = detect::classify(&hw, cfg.as_ref(), &findings);
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    if cli.json {
        let doctor_report = DoctorReport {
            hardware: &hw,
            state,
            findings: &findings,
        };
        match serde_json::to_string_pretty(&doctor_report) {
            Ok(json) => println!("{json}"),
            Err(err) => {
                eprintln!("doctor: failed to encode JSON report: {err}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        print!("{}", report::render_doctor(&hw, &state, &findings));
    }
    ExitCode::SUCCESS
}

/// The hardware probe. The `GREENLIGHT_FIXTURE_HARDWARE` env var swaps in a
/// recorded dump — a dev/test-only seam (documented in README) that keeps
/// the test suite offline; otherwise the live macOS probe runs.
fn hardware_source() -> Box<dyn HardwareSource> {
    match std::env::var_os("GREENLIGHT_FIXTURE_HARDWARE") {
        Some(path) => Box::new(FixtureSource {
            json_path: PathBuf::from(path),
        }),
        None => Box::new(MacosSource),
    }
}

/// The download client. `GREENLIGHT_FIXTURE_HTTP` swaps in a directory of
/// canned responses — a dev/test-only seam of the same standing as
/// `GREENLIGHT_FIXTURE_HARDWARE`; otherwise the real GitHub client runs.
fn http_source() -> Box<dyn Http> {
    match std::env::var_os("GREENLIGHT_FIXTURE_HTTP") {
        Some(dir) => Box::new(FixtureHttp::new(PathBuf::from(dir))),
        None => Box::new(UreqHttp::new()),
    }
}

/// The probe runner for `fix --check`. `GREENLIGHT_FIXTURE_SHELL_DIR` swaps
/// in a directory of canned probe outputs (`kextstat.txt`, `displays.txt`) —
/// a dev/test-only seam of the same standing as `GREENLIGHT_FIXTURE_HARDWARE`;
/// otherwise the live `kextstat` / `system_profiler` probes run.
fn shell_source() -> Box<dyn Shell> {
    match std::env::var_os("GREENLIGHT_FIXTURE_SHELL_DIR") {
        Some(dir) => Box::new(FixtureShell::new(PathBuf::from(dir))),
        None => Box::new(SystemShell),
    }
}

/// `greenlight fix`: the full remediation flow. The config phase (gates →
/// scan/plan → confirm → backup → apply → AMFIPass + NullMothSafe → save)
/// completes fully and independently; the driver phase (verified fetch →
/// install.sh via sudo) runs after it and defers with a printed sudo
/// command when the process is not root (Review Focus 5).
fn fix(cli: &Cli, args: &FixArgs) -> ExitCode {
    match fix_inner(cli, args) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("fix: {err}");
            ExitCode::FAILURE
        }
    }
}

fn fix_inner(cli: &Cli, args: &FixArgs) -> Result<ExitCode, String> {
    // `--check` is the post-reboot verification: it runs ONLY the check —
    // no hardware probe, no gates, no remediation.
    if args.check {
        let shell = shell_source();
        let report = check::run_check(shell.as_ref());
        print!("{}", check::render_check(&report));
        return Ok(if report.passed() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }

    let hw = hardware_source()
        .load()
        .map_err(|err| format!("hardware probe failed: {err}"))?;

    // Hard gates, in ruling order: no NVIDIA GPU, too-old macOS.
    match detect::classify(&hw, None, &[]) {
        MachineState::NoNvidia => {
            return Err(
                "no NVIDIA display device detected — the NVIDIA fix does not apply to this machine"
                    .to_string(),
            );
        }
        MachineState::UnsupportedMacos => {
            let version = hw
                .macos_version
                .map(|(major, minor, patch)| format!("{major}.{minor}.{patch}"))
                .unwrap_or_else(|| "unknown".to_string());
            return Err(format!(
                "unsupported macOS {version} — the driver payload needs macOS 15.5 or newer"
            ));
        }
        _ => {}
    }

    // Validated-profile gate (spec §5): anything outside the validated
    // laptop profile requires an explicit --force.
    if !hw.is_validated_profile() && !args.force {
        eprintln!(
            "fix: this machine is not the validated hardware profile (Turing dGPU + Intel iGPU)"
        );
        eprintln!("fix: re-run with --force to apply the changes anyway");
        return Ok(ExitCode::from(3));
    }

    // SIP: warn-only, never blocks — the config phase sets csr-active-config.
    if !nullmoth::sip_ok(&hw) {
        eprintln!(
            "warning: SIP does not look permissive enough (want csr-active-config 0x0a43 or SIP disabled); \
             the fix sets it, but if the driver refuses to load check `csrutil status` from recoveryOS"
        );
    }

    let configs = esp::find_oc_configs(cli.efi_path.as_deref(), &MacosEspMounter)
        .map_err(|err| err.to_string())?;
    let config_path = configs
        .into_iter()
        .next()
        .expect("find_oc_configs returns exactly one config on Ok");
    let mut cfg = OcConfig::load(&config_path).map_err(|err| err.to_string())?;

    // v1 has no live BAR1-placement detector, so `nvrmsettle=10000` is not
    // added here; an existing nvrmsettle boot-arg is never removed.
    let plan = plan_fix(&cfg, &hw.nvidia_display_paths(), false);

    // Nothing to do: the config already matches the validated set and
    // AMFIPass is registered. The driver phase still applies (it is
    // idempotent as root, a printed pending note otherwise).
    if plan_is_noop(&cfg, &plan) && amfipass_registered(&cfg) {
        println!(
            "already fixed — the OpenCore config already matches the validated NullMoth setup."
        );
        println!("after the next reboot, verify the driver with: greenlight fix --check");
        if args.dry_run {
            return Ok(ExitCode::SUCCESS);
        }
        let http = http_source();
        let downloads = download_dir()?;
        driver_phase(http.as_ref(), &downloads)?;
        return Ok(ExitCode::SUCCESS);
    }

    print_plan(&config_path, &plan);
    if args.dry_run {
        nullmoth::install_nullmoth(Path::new("nullmoth-nvidia-<latest>.tar.gz"), true)
            .map_err(|err| err.to_string())?;
        println!("dry-run — no changes written.");
        return Ok(ExitCode::SUCCESS);
    }
    if !confirm(cli.yes) {
        eprintln!("fix: aborted — no changes written");
        return Ok(ExitCode::FAILURE);
    }

    let backup = OcConfig::backup(&config_path).map_err(|err| err.to_string())?;
    // The confirmation above covers the SecureBootModel downgrade — it is
    // listed in the printed plan the user just approved.
    apply_fix(&mut cfg, &plan, true).map_err(|err| err.to_string())?;

    let http = http_source();
    let downloads = download_dir()?;
    let oc_dir = config_path
        .parent()
        .expect("config.plist always has a parent directory");

    // AMFIPass: pinned zip, unpacked with the built-in `ditto -x -k`, kext
    // copied onto the ESP and registered in Kernel → Add (idempotent).
    let zip = fetch::fetch_amfipass(http.as_ref(), &downloads).map_err(|err| err.to_string())?;
    let kext = extract_amfipass(&zip.path, &downloads)?;
    let kext_dest = oc_dir.join("Kexts").join("AMFIPass.kext");
    if kext_dest.exists() {
        fs::remove_dir_all(&kext_dest)
            .map_err(|err| format!("failed to replace {}: {err}", kext_dest.display()))?;
    }
    copy_dir(&kext, &kext_dest)?;
    println!("AMFIPass.kext copied to {}", kext_dest.display());
    register_amfipass(&mut cfg);

    // NullMothSafe.efi picker entry — only when the 1401 DMG fetches *and*
    // verifies against its published sums line (spec §11.3); otherwise the
    // printed OpenShell steps below are the recovery path.
    let openshell_note = match nullmoth::try_fetch_nullmoth_safe(http.as_ref(), &downloads)
        .map_err(|err| err.to_string())?
    {
        Some(efi) => {
            let tools_dir = oc_dir.join("Tools");
            fs::create_dir_all(&tools_dir)
                .map_err(|err| format!("failed to create {}: {err}", tools_dir.display()))?;
            let dest = tools_dir.join("NullMothSafe.efi");
            fs::copy(&efi, &dest)
                .map_err(|err| format!("failed to copy NullMothSafe.efi: {err}"))?;
            register_nullmoth_safe(&mut cfg);
            println!(
                "NullMothSafe.efi installed at {} (picker entry \"1401: Remove NVIDIA driver\")",
                dest.display()
            );
            false
        }
        None => true,
    };

    cfg.save().map_err(|err| err.to_string())?;
    println!(
        "config written; the pre-fix config is backed up at {}",
        backup.display()
    );
    if plan_resizes_gpu_bars(&plan) {
        print_windows_bsod_warning();
    }

    // The recovery block belongs to the config phase: once the boot config
    // is rewritten, the user needs the restore steps no matter what the
    // driver phase does after this.
    print_recovery_block(&backup);
    if openshell_note {
        println!();
        println!("note: NullMothSafe.efi was not installed (the 1401 DMG could not be fetched and");
        println!("verified), so the manual OpenShell steps above are the recovery path.");
    }

    driver_phase(http.as_ref(), &downloads)?;
    Ok(ExitCode::SUCCESS)
}

/// The driver phase: as root, fetch the latest NullMoth release —
/// hash-verified against the sums the release itself publishes — and hand
/// it to [`nullmoth::install_nullmoth`]. A non-root process defers *before
/// any fetch*: the pending note and the exact sudo command need no network,
/// so an offline unprivileged run still exits 0 (Review Focus 5).
fn driver_phase(http: &dyn Http, downloads: &Path) -> Result<(), String> {
    if !nullmoth::running_as_root() {
        nullmoth::print_driver_pending();
        return Ok(());
    }
    let payload = fetch::fetch_nullmoth_latest(http, downloads).map_err(|err| err.to_string())?;
    nullmoth::install_nullmoth(&payload.path, false).map_err(|err| err.to_string())
}

/// The one confirmation gate. `--yes` answers it; without a terminal there
/// is no one to ask, so the run aborts *before any write* — piped stdin
/// counts as "no".
fn confirm(yes: bool) -> bool {
    if yes {
        return true;
    }
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "fix: confirmation required but stdin is not a terminal — re-run with --yes to apply"
        );
        return false;
    }
    eprint!("Apply these changes? [y/N] ");
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Prints the diff summary: what the config phase will change, in order.
fn print_plan(config_path: &Path, plan: &FixPlan) {
    let mut steps: Vec<String> = plan.findings.iter().map(planned_removal).collect();
    if !plan.boot_args_add.is_empty() {
        steps.push(format!("boot-args += {}", plan.boot_args_add.join(" ")));
    }
    if plan.csr_needs_set {
        steps.push(
            "csr-active-config → 0x0A43 (and keep it across NVRAM resets via Delete)".to_string(),
        );
    }
    for quirk in &plan.quirks {
        steps.push(format!(
            "{}.{} = {}",
            quirk.path,
            quirk.key,
            render_value(&quirk.value)
        ));
    }
    if plan.smbios_downgrade_needed {
        steps.push(
            "note: the SecureBootModel downgrade above is required by the NullMoth driver"
                .to_string(),
        );
    }
    steps.push("Kernel → Block: exclude com.apple.iokit.IONDRVSupport".to_string());
    steps.push("copy AMFIPass.kext into EFI/OC/Kexts and register it in Kernel → Add".to_string());
    steps.push(
        "install the NullMothSafe.efi picker recovery entry when its DMG fetches and verifies"
            .to_string(),
    );
    steps.push(
        "fetch the latest NullMoth driver (SHA-256 verified) and run its install.sh as root"
            .to_string(),
    );

    println!("Plan for {}:", config_path.display());
    for (i, step) in steps.iter().enumerate() {
        println!("  {}. {step}", i + 1);
    }
    println!();
    if plan_resizes_gpu_bars(plan) {
        print_windows_bsod_warning();
    }
}

/// True when the plan writes `UEFI.Quirks.ResizeGpuBars` — the change that
/// makes the Windows dual-boot rule relevant (spec §3 goal 5).
fn plan_resizes_gpu_bars(plan: &FixPlan) -> bool {
    plan.quirks.iter().any(|quirk| quirk.key == "ResizeGpuBars")
}

/// The Windows dual-boot warning (spec §3 goal 5, INSTALL-GUIDE §7):
/// `ResizeGpuBars` is applied hardware-wide by OpenCore before any OS
/// starts, so Windows booted through the OpenCore picker blue-screens.
/// Shown both with the plan and again once the change is applied.
fn print_windows_bsod_warning() {
    println!("warning: ResizeGpuBars applies hardware-wide before any OS starts — after this");
    println!("change, boot Windows only via the firmware boot menu (F12), NOT via the OpenCore");
    println!("picker, or Windows will blue-screen.");
}

/// One-line description of the removal a finding implies.
fn planned_removal(finding: &Finding) -> String {
    match finding.kind {
        FindingKind::FakeClassCode => format!(
            "remove the fake class-code from DeviceProperties entry {}",
            finding.location
        ),
        FindingKind::DisableGpu => format!(
            "remove disable-gpu from DeviceProperties entry {}",
            finding.location
        ),
        FindingKind::NvDisableArg => "remove nv_disable=1 from boot-args".to_string(),
        FindingKind::PikeraArg => "remove agdpmod=pikera from boot-args".to_string(),
    }
}

/// Compact display form of a pinned plist value, for the plan listing.
fn render_value(value: &plist::Value) -> String {
    match value {
        plist::Value::String(s) => format!("\"{s}\""),
        plist::Value::Integer(i) => i.to_string(),
        plist::Value::Boolean(b) => b.to_string(),
        other => format!("{other:?}"),
    }
}

/// INSTALL-GUIDE §6 Tier 2, verbatim, with the actual backup name
/// substituted.
fn recovery_block(backup_name: &str) -> String {
    format!(
        "\nIf macOS fails to boot: OpenCore picker → OpenShell →\n  fs0:\n  cd EFI\\OC\n  cp {backup_name} config.plist\n  reboot\n"
    )
}

/// Prints the recovery block for `backup` (see [`recovery_block`]).
fn print_recovery_block(backup: &Path) {
    let name = backup
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.plist.bak-greenlight-<timestamp>".to_string());
    print!("{}", recovery_block(&name));
}

/// Unpacks the AMFIPass zip with the macOS built-in `ditto -x -k` (no new
/// crate) and returns the path of the extracted `AMFIPass.kext`.
fn extract_amfipass(zip: &Path, dir: &Path) -> Result<PathBuf, String> {
    let out = dir.join("amfipass-extract");
    fs::create_dir_all(&out).map_err(|err| format!("failed to create {}: {err}", out.display()))?;
    let status = std::process::Command::new("ditto")
        .args(["-x", "-k"])
        .arg(zip)
        .arg(&out)
        .status()
        .map_err(|err| format!("failed to run `ditto -x -k`: {err}"))?;
    if !status.success() {
        return Err(format!("ditto -x -k {} failed: {status}", zip.display()));
    }
    let direct = out.join("AMFIPass.kext");
    if direct.is_dir() {
        return Ok(direct);
    }
    // Tolerate one wrapping directory level inside the archive.
    let entries =
        fs::read_dir(&out).map_err(|err| format!("failed to read {}: {err}", out.display()))?;
    for entry in entries {
        let candidate = entry
            .map_err(|err| err.to_string())?
            .path()
            .join("AMFIPass.kext");
        if candidate.is_dir() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "{}: no AMFIPass.kext inside the zip",
        zip.display()
    ))
}

/// Recursive directory copy (`fs::copy` handles plain files only).
fn copy_dir(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|err| format!("failed to create {}: {err}", dst.display()))?;
    let entries =
        fs::read_dir(src).map_err(|err| format!("failed to read {}: {err}", src.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| err.to_string())?;
        let target = dst.join(entry.file_name());
        if entry.file_type().map_err(|err| err.to_string())?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)
                .map_err(|err| format!("failed to copy to {}: {err}", target.display()))?;
        }
    }
    Ok(())
}

/// A per-run scratch dir under the system temp dir for downloaded payloads.
fn download_dir() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("greenlight-{}", std::process::id()));
    fs::create_dir_all(&dir).map_err(|err| format!("failed to create {}: {err}", dir.display()))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_block_substitutes_backup_name_verbatim() {
        // INSTALL-GUIDE §6 Tier 2, character for character.
        assert_eq!(
            recovery_block("config.plist.bak-greenlight-20261011-150625"),
            "\nIf macOS fails to boot: OpenCore picker → OpenShell →\n  fs0:\n  cd EFI\\OC\n  cp config.plist.bak-greenlight-20261011-150625 config.plist\n  reboot\n"
        );
    }
}
