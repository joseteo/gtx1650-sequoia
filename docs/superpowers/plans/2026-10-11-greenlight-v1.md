# greenlight v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `greenlight`, a Rust CLI that detects an OpenCore Hackintosh with a supported NVIDIA GPU, repairs the OpenCore config, installs the latest hash-verified NullMoth driver, and verifies the result — the Audience-A core of the approved design.

**Architecture:** Single Rust binary (`greenlight`) with library modules `detect`, `occonfig`, `esp`, `fetch`, `nullmoth`, `report`. All hardware/root interactions sit behind traits with fixture-backed fakes so the full test suite runs unprivileged and offline. `forge` (Audience B EFI generation) is a separate follow-up plan; the module seams (`HardwareSource`, platform dispatch) are laid here so plan 2 extends rather than rewrites.

**Tech Stack:** Rust stable, edition 2021. Pinned crates (substitute only with equivalent justification): `clap` 4 (derive), `plist` 1, `serde` + `serde_json`, `sha2`, `hex`, `ureq` 2 (blocking HTTP), `thiserror`; dev: `assert_cmd`, `predicates`, `tempfile`.

**Spec:** `docs/superpowers/specs/2026-10-11-greenlight-design.md` (approved 2026-10-11). Values quoted below are pinned by the spec and by `docs/INSTALL-GUIDE.md` §3/§11 — treat them as exact.

## Global Constraints

- v1 targets **macOS ≥ 15.5 on Intel+OpenCore** only; Windows/Linux support is plan 2 (the platform seams must not preclude it).
- **No NullMoth, OpenCore, or Acidanthera content is ever committed** to the repo — downloads at runtime only, hash-verified where upstream publishes sums. Our code is MIT.
- Exact config values (spec §3 / INSTALL-GUIDE §3):
  - boot-args to add: `nvfb=1 nvaccel=1 nvfbheads=4 -nvkmsnosmooth amfi_get_out_of_my_way=0x1 amfi=0x80`; add `nvrmsettle=10000` **only** when BAR1 placement failure is detected.
  - `csr-active-config` = data `<430A0000>` (base64 `QwoAAA==`), under NVRAM GUID `7C436110-AB2A-4BBB-A880-FE41995C9F82`, also listed in that GUID's `Delete` array.
  - `Kernel → Block`: entry `com.apple.iokit.IONDRVSupport`, `Strategy=Exclude`, `Enabled=true`.
  - `UEFI → Quirks → ResizeGpuBars = 13`; `Booter → Quirks → ResizeAppleGpuBars = -1`.
  - `Misc → Security → SecureBootModel = Disabled` — only with explicit user confirmation.
  - AMFIPass.kext pinned **1.4.1** (`Arch=Any`, `MinKernel=20.0.0`), from the official acidanthera release.
- Breakage patterns `scan` must find (INSTALL-GUIDE §2/§11): `class-code = <FFFFFFFF>` or `disable-gpu` on a detected NVIDIA DeviceProperties path; `nv_disable=1` or `agdpmod=pikera` in boot-args.
- **Never modify DeviceProperties entries on non-NVIDIA paths** (iGPU at `PciRoot(0x0)/Pci(0x2,0x0)`, audio at `PciRoot(0x0)/Pci(0x1F,0x3)` are sacred).
- Backup before every config write: sibling file `config.plist.bak-greenlight-YYYYMMDD-HHMMSS` (UTC).
- Unvalidated hardware: `fix`/`--check` refuse without `--force` (spec §11.2). v1 validated profile: Intel CPU + NVIDIA Turing dGPU on an Optimus laptop (reference: `10de:1f99`).
- Fixtures are serial-scrubbed (`MLB`, `SystemSerialNumber`, `SystemUUID`, `SmUUID` → `REDACTED`).
- Every root-requiring operation lives behind a trait or a `dry_run` path; `cargo test` must pass as an unprivileged user with no network.
- One commit per task, conventional-commit messages, tests green before commit.

## Review Focus

1. **Two OpenCore configs present** (boot ESP + installer USB still plugged in): expected behavior is an error listing both paths and demanding `--efi-path`, never a silent pick — test in Task 6.
2. **A `class-code`/`disable-gpu` entry on a non-NVIDIA path** (e.g. an iGPU spoof): expected to be reported but never removed — test in Task 4.
3. **Running `fix` twice**: expected zero changes, no duplicated boot-args or NVRAM Delete entries — idempotency test in Task 5.
4. **Release with missing/mismatching SHA256SUMS**: expected hard abort before any install step — test in Task 7.
5. **`fix` install phase without root**: expected a single clean `sudo` re-exec of the install step, never a half-applied config (config edits and driver install are separate phases with separate confirmations) — test in Task 10.

---

### Task 1: Cargo scaffold + CLI skeleton

**Files:**
- Create: `Cargo.toml`, `src/main.rs`, `src/lib.rs`, `tests/cli.rs`
- Modify: `.gitignore` (add `/target`)

**Interfaces:**
- Produces: binary `greenlight` with subcommands `doctor`, `fix`, `forge`; global flags `--json`, `--yes`, `--efi-path <PATH>`. `forge` prints `"forge: not yet implemented (planned for v2)"` and exits 2. Library root exposes empty modules `detect`, `occonfig`, `esp`, `fetch`, `nullmoth`, `report`.

- [ ] **Step 1: Install the Rust toolchain (prerequisite)**

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
. "$HOME/.cargo/env" && cargo --version
```
Expected: prints `cargo 1.xx.x`. No sudo; user-local install.

- [ ] **Step 2: Write the failing CLI test** in `tests/cli.rs`

```rust
#[test] fn doctor_runs_and_prints_header()  // assert_cmd: exit 0, stdout contains "greenlight doctor"
#[test] fn forge_exits_2_with_planned_message()
#[test] fn unknown_subcommand_errors()
```

- [ ] **Step 3: Run** `cargo test` — Expected: FAIL (no target yet).

- [ ] **Step 4: Implement the scaffold** — `Cargo.toml` (package `greenlight`, edition 2021, pinned deps), clap derive parser in `src/main.rs`, module stubs in `src/lib.rs`.

- [ ] **Step 5: Run** `cargo test` — Expected: PASS (3 tests).

- [ ] **Step 6: Commit** `chore: cargo scaffold with clap CLI skeleton`

### Task 2: Real-machine test fixtures

**Files:**
- Create: `tests/fixtures/config-broken.plist`, `tests/fixtures/config-fixed.plist`, `tests/fixtures/hardware-lenovo.json`, `tests/fixtures/SHA256SUMS-good.txt`, `tests/fixtures/SHA256SUMS-bad.txt`

**Interfaces:**
- Produces: fixture paths used by Tasks 3–7 tests. `hardware-lenovo.json` schema = array of `{ "vendor": "10de", "device": "1f99", "class": "0x030000", "acpipadth": "PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)", "name": "NVIDIA GeForce GTX 1650" }` entries (includes `10de:10fa` HDAU at `.../Pci(0x0,0x1)` class `0x040300`, and iGPU `8086:3e9b` at `PciRoot(0x0)/Pci(0x2,0x0)`), plus top-level `{ "pci": [...], "macos": "15.7.4", "sip": "0x0a43" }`.

- [ ] **Step 1: Copy the real configs out of the backups** (root-owned dirs; use the elevated shell pattern from INSTALL-GUIDE §9: script in `/Users/Shared/nullmoth-stage/`, `osascript ... with administrator privileges`, then `chmod 644`). Sources: broken = `~/Documents/drivers/EFI-backup/EFI-disk1s1/OC/config.plist`; fixed = `/Users/Shared/nullmoth-stage/config-live.plist`.
- [ ] **Step 2: Scrub serials** — replace values of `MLB`, `SystemSerialNumber`, `SystemUUID`, `SmUUID` with `REDACTED` in both plists (`plutil` or plist crate round-trip; verify with `grep -c REDACTED`).
- [ ] **Step 3: Verify the fixtures carry the intended truth**: `grep` the broken fixture for `FFFFFFFF` and `disable-gpu` on the `PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)` entry (must be present); the fixed fixture must not contain them.
- [ ] **Step 4: Write the small fixtures** — `hardware-lenovo.json` (schema above), `SHA256SUMS-good.txt` (two lines, real format: `<64hex>  <filename>`), `SHA256SUMS-bad.txt` (same with one hash altered).
- [ ] **Step 5: Commit** `test: add real-machine fixtures (broken/fixed configs, hardware dump, sums)`

### Task 3: `occonfig` — load, save, backup

**Files:**
- Create: `src/occonfig/mod.rs`, `tests/occonfig_io.rs`

**Interfaces:**
- Consumes: Task 2 fixtures.
- Produces:
  - `pub struct OcConfig { doc: plist::Dictionary, path: PathBuf }`
  - `pub fn OcConfig::load(path: &Path) -> Result<OcConfig, Error>`
  - `pub fn save(&self) -> Result<(), Error>` — writes plist XML back to `self.path`
  - `pub fn backup(path: &Path) -> Result<PathBuf, Error>` — copy to `config.plist.bak-greenlight-YYYYMMDD-HHMMSS` (UTC), returns new path
  - `pub fn boot_args(&self) -> BTreeSet<String>` — parsed from NVRAM GUID `7C436110-AB2A-4BBB-A880-FE41995C9F82`
  - `pub fn set_boot_args(&mut self, args: &BTreeSet<String>)` — single space-joined string
  - `pub fn device_properties_mut(&mut self) -> Option<&mut plist::Dictionary>` — `DeviceProperties → Add`

- [ ] **Step 1: Failing tests** (`tests/occonfig_io.rs`): `loads_broken_fixture_and_reads_boot_args` (asserts `alcid=11` present); `backup_creates_timestamped_sibling` (tempdir; asserts name pattern `config.plist.bak-greenlight-` + content identical); `save_roundtrip_preserves_other_keys` (load fixed fixture, save to tempdir, reload, dict equality).
- [ ] **Step 2: Run** `cargo test occonfig_io` — Expected: FAIL (unresolved `OcConfig`).
- [ ] **Step 3: Implement** `src/occonfig/mod.rs` (signatures above; UTC timestamp via `std::time::SystemTime` → manual date formatting, no chrono dep).
- [ ] **Step 4: Run** `cargo test occonfig_io` — Expected: PASS.
- [ ] **Step 5: Commit** `feat(occonfig): load/save with timestamped backup and boot-args accessors`

### Task 4: `occonfig::scan` — breakage detection

**Files:**
- Create: `src/occonfig/scan.rs`, `tests/scan.rs`
- Modify: `src/occonfig/mod.rs` (re-export)

**Interfaces:**
- Consumes: `OcConfig` (Task 3).
- Produces:
  - `pub enum FindingKind { FakeClassCode, DisableGpu, NvDisableArg, PikeraArg }`
  - `pub struct Finding { pub kind: FindingKind, pub location: String, pub detail: String }`
  - `pub fn scan(cfg: &OcConfig, nvidia_paths: &[String]) -> Vec<Finding>` — DeviceProperties findings only on paths in `nvidia_paths`; boot-args findings global. Entries on other paths with the same properties are **not** returned.

- [ ] **Step 1: Failing tests** (`tests/scan.rs`):
  - `finds_dortania_leftover_in_broken_fixture` with `nvidia_paths = ["PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)"]` → contains `FakeClassCode` and `DisableGpu`.
  - `fixed_fixture_scans_clean` → empty.
  - `ignores_classcode_on_non_nvidia_path` — hand-built plist with `class-code=<FFFFFFFF>` on `PciRoot(0x0)/Pci(0x2,0x0)` only → empty (Review Focus 2).
  - `finds_nv_disable_and_pikera_in_boot_args` — plist with `nv_disable=1 agdpmod=pikera` in boot-args → both findings.
- [ ] **Step 2: Run** `cargo test scan` — Expected: FAIL.
- [ ] **Step 3: Implement** `scan.rs`. Compare `class-code` as plist `Data` bytes `[0xFF; 4]` (the plist crate decodes `<FFFFFFFF>` to data; also accept the string form defensively).
- [ ] **Step 4: Run** `cargo test scan` — Expected: PASS.
- [ ] **Step 5: Commit** `feat(occonfig): scan for NVIDIA-killing leftovers and bad boot-args`

### Task 5: `occonfig::fix` — plan + apply the validated config set

**Files:**
- Create: `src/occonfig/fix.rs`, `tests/fix_plan.rs`
- Modify: `src/occonfig/mod.rs` (re-export)

**Interfaces:**
- Consumes: `OcConfig`, `Finding` (Tasks 3–4).
- Produces:
  - `pub struct FixPlan { pub findings: Vec<Finding>, pub boot_args_add: Vec<String>, pub csr_needs_set: bool, pub quirks: Vec<QuirkEdit>, pub smbios_downgrade_needed: bool, pub nvrmsettle: bool }`
  - `pub struct QuirkEdit { pub path: &'static str, pub key: &'static str, pub value: plist::Value }`
  - `pub fn plan_fix(cfg: &OcConfig, nvidia_paths: &[String], bar1_unplaced: bool) -> FixPlan`
  - `pub fn apply_fix(cfg: &mut OcConfig, plan: &FixPlan, allow_smbios_downgrade: bool) -> Result<(), Error>` — returns `Err(SecureBootModelNotConfirmed)` if downgrade needed but not allowed.
  - Exact values per Global Constraints; `nvrmsettle=10000` included iff `bar1_unplaced`.

- [ ] **Step 1: Failing tests** (`tests/fix_plan.rs`):
  - `plan_on_broken_fixture_includes_all_edits` — assert plan contains both removals, all six base boot-args, `csr_needs_set=true`, the three quirks, `smbios_downgrade_needed=false` (fixture already `Disabled`), `nvrmsettle=true` when `bar1_unplaced=true`.
  - `apply_then_matches_fixed_fixture_semantics` — apply to broken fixture, then assert: NVIDIA DeviceProperties path absent; boot-args superset of fixed fixture's NVIDIA args; `csr-active-config == Data([0x43,0x0A,0,0])` and present in NVRAM `Delete`; IONDRVSupport block entry; `ResizeGpuBars==13`; `ResizeAppleGpuBars==-1`; non-NVIDIA DeviceProperties entries untouched.
  - `second_apply_is_noop` — apply twice, assert equal serialized output (Review Focus 3).
  - `nvrmsettle_omitted_when_bar1_ok` — `bar1_unplaced=false` → arg absent.
  - `smbios_downgrade_requires_confirmation` — fixture edited to `SecureBootModel=Default`; `apply_fix(.., false)` errors, `apply_fix(.., true)` succeeds.
- [ ] **Step 2: Run** `cargo test fix_plan` — Expected: FAIL.
- [ ] **Step 3: Implement** `fix.rs` (signatures above).
- [ ] **Step 4: Run** `cargo test fix_plan` — Expected: PASS.
- [ ] **Step 5: Commit** `feat(occonfig): plan/apply the validated NullMoth config set`

### Task 6: `esp` — locate the OpenCore config

**Files:**
- Create: `src/esp/mod.rs`, `tests/esp.rs`

**Interfaces:**
- Produces:
  - `pub fn find_oc_configs(override_dir: Option<&Path>, mounter: &dyn EspMounter) -> Result<Vec<PathBuf>, Error>`
  - `pub trait EspMounter { fn list_esp_configs(&self) -> Result<Vec<PathBuf>, Error>; }` — real impl `MacosEspMounter` (enumerates `diskutil list -plist`, mounts EFI partitions with `diskutil mount`, looks for `EFI/OC/config.plist`); fake impl for tests reads a tempdir tree.
  - Rule: 0 configs → `Err(NoOpenCore)`; >1 → `Err(Ambiguous(Vec<PathBuf>))`; exactly 1 → Ok.

- [ ] **Step 1: Failing tests**: `override_path_wins` (tempdir); `zero_configs_errors`; `two_configs_is_ambiguous` (Review Focus 1 — error message lists both paths and mentions `--efi-path`); `one_config_ok`.
- [ ] **Step 2: Run** `cargo test esp` — Expected: FAIL.
- [ ] **Step 3: Implement** — fake-first; `MacosEspMounter` shells `diskutil`, parsing plist output. Try unprivileged mount first, report the sudo hint on failure.
- [ ] **Step 4: Run** `cargo test esp` — Expected: PASS.
- [ ] **Step 5: Commit** `feat(esp): locate boot OpenCore config with ambiguity guard`

### Task 7: `fetch` — verified downloads

**Files:**
- Create: `src/fetch/mod.rs`, `tests/fetch.rs`

**Interfaces:**
- Produces:
  - `pub struct VerifiedDownload { pub path: PathBuf, pub sha256: String, pub version: String }`
  - `pub fn sha256_of(path: &Path) -> Result<String, Error>` — hex
  - `pub fn verify_sha256sums(dir: &Path, sums: &str) -> Result<(), Error>` — parses `<hex>  <name>` lines, verifies each file in `dir`; first mismatch/missing → `Err` naming the file (Review Focus 4).
  - `pub trait Http { fn get(&self, url: &str) -> Result<Vec<u8>, Error>; }` — `UreqHttp` real impl; fixture impl for tests.
  - `pub fn fetch_nullmoth_latest(http: &dyn Http, dest: &Path) -> Result<VerifiedDownload, Error>` — `GET https://api.github.com/repos/nullmoth/nvidia-macos-driver/releases/latest` → tag + `nullmoth-nvidia-*.tar.gz` and `SHA256SUMS.txt` asset URLs; download both, verify, return tarball path + version.
  - `pub fn fetch_amfipass(http: &dyn Http, dest: &Path) -> Result<VerifiedDownload, Error>` — pinned release tag `1.4.1` from `acidanthera/AMFIPass`, RELEASE build zip.
  - JSON parsing via `serde_json` only (no big GitHub client crate).

- [ ] **Step 1: Failing tests**: `sha256_of_known_content`; `verify_sums_good_fixture_ok`; `verify_sums_bad_fixture_names_file`; `fetch_nullmoth_latest_parses_release_and_verifies` (fake `Http` serving canned JSON + tarball bytes + sums; assert returned version and that a tampered-tarball variant errors); `missing_sums_file_aborts`.
- [ ] **Step 2: Run** `cargo test fetch` — Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `cargo test fetch` — Expected: PASS.
- [ ] **Step 5: Commit** `feat(fetch): hash-verified GitHub release downloads`

### Task 8: `detect` — hardware probing behind a trait

**Files:**
- Create: `src/detect/mod.rs`, `tests/detect.rs`

**Interfaces:**
- Produces:
  - `pub struct PciDevice { pub vendor: u16, pub device: u16, pub class_code: u32, pub acpi_path: Option<String>, pub name: String }`
  - `pub struct Hardware { pub pci: Vec<PciDevice>, pub macos_version: Option<(u32,u32,u32)>, pub sip: Option<String> }`
  - `pub trait HardwareSource { fn load(&self) -> Result<Hardware, Error>; }`
  - `pub struct FixtureSource { pub json_path: PathBuf }` (reads Task 2 schema); `pub struct MacosSource` (parses `ioreg -r -c IOPCIDevice -a` plist output + `sw_vers` + `csrutil status`).
  - `impl Hardware { pub fn nvidia_display_paths(&self) -> Vec<String>` — ACPI paths of class `0x03xxxx` + vendor `0x10de`; `pub fn is_validated_profile(&self) -> bool` — Intel CPU-class laptop heuristic: has NVIDIA Turing-era dGPU **and** an Intel iGPU (`0x8086` class `0x03xxxx`) }
  - `pub enum MachineState { ReadyToFix, AlreadyWorking, NoOpenCore, UnsupportedMacos, NoNvidia }`
  - `pub fn classify(hw: &Hardware, cfg: Option<&OcConfig>, findings: &[Finding]) -> MachineState`

- [ ] **Step 1: Failing tests**: `fixture_yields_gtx1650_path` (`nvidia_display_paths == ["PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)"]`); `lenovo_is_validated_profile`; `classify_broken_fixture_is_ready_to_fix`; `classify_fixed_fixture_is_already_working`; `no_nvidia_machine_reports_so`.
- [ ] **Step 2: Run** `cargo test detect` — Expected: FAIL.
- [ ] **Step 3: Implement** (fixture first; `MacosSource` last).
- [ ] **Step 4: Run** `cargo test detect` — Expected: PASS.
- [ ] **Step 5: Commit** `feat(detect): hardware probe with fixture + macOS sources`

### Task 9: `doctor` command + report

**Files:**
- Create: `src/report/mod.rs`, `tests/doctor.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `HardwareSource`, `MachineState`, `OcConfig`, `scan` (Tasks 3–8).
- Produces:
  - `pub fn render_doctor(hw: &Hardware, state: &MachineState, findings: &[Finding]) -> String` — human report: detected hardware table, state verdict, findings with plain-language fixes, next-step line (`greenlight fix` / report-only explanation / `--force` hint for unvalidated hardware).
  - CLI: `greenlight doctor [--json] [--efi-path P]` — JSON mode emits `{"hardware":…,"state":…,"findings":[…]}`.

- [ ] **Step 1: Failing tests** (`tests/doctor.rs`): drive the CLI with env var `GREENLIGHT_FIXTURE_HARDWARE=tests/fixtures/hardware-lenovo.json` + `--efi-path tests/fixtures` (fixture config renamed per test): `doctor_reports_lenovo_hardware_and_state`; `doctor_json_is_parseable`; `doctor_on_clean_machine_says_already_working`.
- [ ] **Step 2: Run** `cargo test doctor` — Expected: FAIL.
- [ ] **Step 3: Implement** report renderer + wiring; the env-var fixture hook is the test seam (documented in README as dev-only).
- [ ] **Step 4: Run** `cargo test doctor` — Expected: PASS.
- [ ] **Step 5: Commit** `feat(doctor): hardware report with machine-state verdict`

### Task 10: `fix` command — the full flow

**Files:**
- Modify: `src/main.rs`
- Create: `src/nullmoth/mod.rs`, `tests/fix_flow.rs`

**Interfaces:**
- Consumes: everything above.
- Produces:
  - `pub fn install_nullmoth(payload: &Path, dry_run: bool) -> Result<(), Error>` — extracts tarball to staging dir, runs `sudo ./install.sh` from staging (spec §11.6); if process is not root and `dry_run == false`, re-exec **only the install step** via `sudo` (Review Focus 5). Dry-run prints the exact commands.
  - `pub fn sip_ok(hw: &Hardware) -> bool` — `csr-active-config` value `0x0a43` means permissive enough; warn otherwise.
  - CLI flow (`greenlight fix [--yes] [--force] [--efi-path P] [--dry-run]`): detect → gate on validated profile / macOS ≥ 15.5 / SIP warning → `scan` + `plan_fix` → print diff summary → confirm (unless `--yes`) → `backup` → `apply_fix` (SecureBootModel prompt if needed) → `save` → `fetch_amfipass` → copy kext into `EFI/OC/Kexts` + register in `Kernel → Add` (idempotent) → `fetch_nullmoth_latest` → `install_nullmoth` → print recovery block.
  - Recovery block printed verbatim (backup path substituted), from INSTALL-GUIDE §6 Tier 2:
    ```
    If macOS fails to boot: OpenCore picker → OpenShell →
      fs0:
      cd EFI\OC
      cp <BACKUP_NAME> config.plist
      reboot
    ```
  - NullMothSafe.efi picker entry: only if the 1401 DMG is fetched **and** a published hash exists for it; otherwise print a note that the manual OpenShell steps above are the recovery path (spec §11.3).

- [ ] **Step 1: Failing tests** (`tests/fix_flow.rs`, tempdir ESP + fake HTTP + fixture hardware env):
  - `fix_dry_run_changes_nothing` — tempdir copy of broken fixture; `--dry-run` exits 0, file bytes unchanged, stdout lists planned edits.
  - `fix_yes_applies_and_backs_up` — backup file exists with original bytes; config now passes `scan` clean; boot-args contain the six NVIDIA args.
  - `fix_twice_second_run_is_noop`.
  - `fix_refuses_unvalidated_without_force` — fixture with device `1f99` renamed to unknown `9999` → exit 3 without `--force`, succeeds with `--force`.
  - `fix_without_root_still_completes_config_phase` — install step runs with `dry_run` behavior and prints the `sudo` command when euid != 0 (test asserts no partial state: config applied, driver not).
- [ ] **Step 2: Run** `cargo test fix_flow` — Expected: FAIL.
- [ ] **Step 3: Implement** `nullmoth/mod.rs` + `main.rs` wiring.
- [ ] **Step 4: Run** `cargo test fix_flow` — Expected: PASS.
- [ ] **Step 5: Commit** `feat(fix): end-to-end remediation flow with verified NullMoth install`

### Task 11: `fix --check` — post-reboot verification

**Files:**
- Create: `src/nullmoth/check.rs`, `tests/check.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Produces:
  - `pub trait Shell { fn run(&self, cmd: &str, args: &[&str]) -> Result<String, Error>; }` — real impl wraps `std::process::Command`; fixture impl returns canned outputs.
  - `pub struct CheckReport { pub kexts_loaded: Vec<String>, pub gpu_claimed: bool, pub displays: Vec<String>, pub metal: Option<String> }`
  - `pub fn run_check(sh: &dyn Shell) -> CheckReport` — parses `kextstat` (four `com.nullmoth.*` bundles), `system_profiler SPDisplaysDataType` (NVIDIA name, Metal, resolution@refresh lines).
  - CLI: `greenlight fix --check` renders the proof table (like README's) and exits 0 only if all four kexts loaded + GPU claimed.

- [ ] **Step 1: Failing tests** with canned outputs captured from the real machine (`kextstat`, `system_profiler SPDisplaysDataType` saved as fixtures now, while the driver is working): `check_parses_loaded_kexts_and_display`; `check_fails_when_kext_missing` (edited fixture); CLI test `fix_check_exit_codes`.
- [ ] **Step 2: Run** `cargo test check` — Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `cargo test check` — Expected: PASS.
- [ ] **Step 5: Commit** `feat(fix): --check post-reboot verification with proof table`

### Task 12: Repo polish — README rewrite + CI

**Files:**
- Modify: `README.md`, `.gitignore`
- Create: `.github/workflows/ci.yml`, `docs/PLAN-2-FORGE.md` (one-paragraph pointer to the design doc §3 goal 4, marked "next")

**Interfaces:**
- Consumes: all tasks.

- [ ] **Step 1: Rewrite README** around the tool: name/badges (keep PayPal), one-line pitch, `cargo build --release` + `greenlight doctor` quickstart, the proof table + Lenovo validation story (link `docs/INSTALL-GUIDE.md`), redesign section (unchanged), license boundary paragraph (our MIT code; NullMoth fetched at runtime, PolyForm NC applies to it), credit to NullMoth + dortania.
- [ ] **Step 2: Add CI** — `cargo test` + `cargo clippy -- -D warnings` on `macos-latest` (fixture tests need no network/root).
- [ ] **Step 3: Verify locally** — `cargo test && cargo clippy -- -D warnings` both green; render README preview.
- [ ] **Step 4: Commit** `docs: reorient repo around greenlight CLI + CI`

---

## Self-review notes (done by the planner)

- **Spec coverage:** §3 goals 1–3 (doctor/fix/--check) → Tasks 9, 10, 11. §3 goal 4 (forge) → deferred to plan 2 (stated in Architecture + Task 12 pointer doc); success criterion 2 likewise. §5 scope gate → Task 10 `--force` flow + Task 8 `is_validated_profile`. §6 module table → all modules present. §11.3 (NullMothSafe.efi conditional) → Task 10. §11.4 (pinned AMFIPass; NullMoth latest) → Task 7. §11.5 (parameterized `nvrmsettle`, SecureBootModel confirm) → Task 5. §11.6 (shell out to install.sh) → Task 10. §8 README reorientation → Task 12. Cross-platform detection (§2) → seam laid (`HardwareSource`, `EspMounter`, `Shell`), impl deferred to plan 2 with forge.
- **Live validation** (success criteria 1, 3) happens at execution time on the reference laptop with the owner present — it is a runbook step, not a code task: after Task 11, run `greenlight doctor` and `greenlight fix --check` live, and (owner-consented) a full `--dry-run` of `fix` against the live ESP.
- **Type consistency:** `OcConfig`, `Finding`/`FindingKind`, `FixPlan`, `Hardware`/`HardwareSource`, `EspMounter`, `Http`, `Shell`, `VerifiedDownload` names are used identically across tasks.
