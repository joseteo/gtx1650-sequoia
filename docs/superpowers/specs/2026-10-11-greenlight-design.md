# Design: `greenlight` — NVIDIA-on-OpenCore automation tool

Date: 2026-10-11
Author: José Teo Lorente (@joseteo)
Repo: https://github.com/joseteo/nvidia-macos-opencore (keyword slug for discoverability; redirects from `greenlight` / `gtx1650-sequoia`)
Status: design — direction approved by owner 2026-10-11

> Naming decided by the owner: the CLI/brand is **greenlight**; the repo slug is searchable keywords (§8).

---

## 1. Intent and why

Getting an NVIDIA Turing card working on macOS 15 via OpenCore + the NullMoth driver took this
project two days of expert debugging, and the failure that cost the most time was silent: a leftover
dortania `class-code=<FFFFFFFF>` DeviceProperties entry that made the driver never probe the card
(see `docs/INSTALL-GUIDE.md` §2). That knowledge is currently locked in a guide humans must read
and execute by hand.

This project turns everything we proved on the reference machine into a **Rust CLI** that does it
automatically: detect the hardware and machine state, repair the OpenCore config, fetch and install
the *latest* NullMoth driver (hash-verified, straight from the official release — never bundled,
never stale), and verify the result. For machines that don't boot macOS yet, the same tool generates
a complete OpenCore EFI for their hardware.

The user's experience: **clone the repo, build, run, reboot — external monitor works.**

## 2. Audiences (decided with the owner)

- **Audience A — "macOS boots, NVIDIA is dead."** OpenCore already boots macOS (usually on the
  Intel iGPU); the NVIDIA card does nothing. This was the reference laptop's state. The tool fixes
  it end-to-end. **This is the core.**
- **Audience B — "no OpenCore yet."** The machine (typically a laptop) has never booted macOS, or
  the owner is preparing a USB installer from another OS. The tool generates a working OpenCore EFI
  from scratch for their hardware. **Decided: full EFI generation, not a handoff.**

Audience B is usually **not on macOS** (you can't run a macOS binary on a machine that doesn't boot
macOS — the owner lived this: Windows was the fallback OS on the reference laptop). Therefore the
tool is **cross-platform: macOS, Windows, Linux**.

## 3. Goals

1. `doctor` — probe hardware (PCI enumeration: CPU, iGPU, NVIDIA dGPU + its audio function,
   Ethernet/Wi-Fi, audio codec), assess machine state (which audience), print a tailored report.
2. `fix` (macOS, Audience A) — the full remediation we validated:
   - locate the OpenCore config that actually boots the machine; timestamped backup before any write;
   - detect and remove known NVIDIA-killing leftovers: `class-code=<FFFFFFFF>`, `disable-gpu`,
     `nv_disable=1`, `agdpmod=pikera` (INSTALL-GUIDE §2, §11);
   - apply the validated config set (INSTALL-GUIDE §3): NullMoth boot-args (`nvfb=1 nvaccel=1
     nvfbheads=4 -nvkmsnosmooth`, `amfi_get_out_of_my_way=0x1 amfi=0x80`, `nvrmsettle=10000`),
     `csr-active-config=0x0A43`, `Kernel→Block` IONDRVSupport (Exclude), `ResizeGpuBars=13`,
     `ResizeAppleGpuBars=-1`, AMFIPass.kext, `NullMothSafe.efi` picker recovery entry;
   - query GitHub for the latest NullMoth release, download the official tarball, verify every file
     against the published SHA-256 sums, install via the same steps as the official payload
     (`/Library/Extensions`, `/Library/GPUBundles`, `/Users/Shared/nvfw`, `kmutil` aux KC rebuild);
   - print exact reboot + recovery instructions.
3. `fix --check` (post-reboot) — verify: NullMoth kexts loaded, NVIDIA card claimed, resolution /
   refresh / Metal level per display; print the same proof table we publish.
4. `forge` (any OS, Audience B) — generate a complete, bootable OpenCore EFI for the detected
   hardware: download official OpenCore release + official kext releases (Lilu, VirtualSMC,
   WhateverGreen, AppleALC, NVMeFix, plus networking/inputs per detected hardware), generate
   `config.plist` from hardware-matched templates, include the standard SSDTs, generate SMBIOS
   serials, write everything to the target ESP/USB. The generated EFI is **installer-safe**
   (BAR/IONDRVSupport settings compatible with the macOS installer — INSTALL-GUIDE §7); the
   Audience-A NVIDIA set is applied by running `fix` once the fresh install boots, and `doctor`
   tells the user which stage they're in (§11.1).
5. Safety everywhere: nothing is written without a timestamped backup; dry-run diff shown before
   applying; every operation has a printed undo path; the Windows-BSOD rule (ResizeGpuBars applies
   hardware-wide — boot Windows via firmware menu, not the OC picker) is surfaced whenever relevant.

## 4. Non-goals and anti-goals

- **No NullMoth content in the repo.** No code, kexts, bundles, GSP firmware, or app copies — ever.
  Everything NullMoth is downloaded at runtime from the official release and hash-verified. Our repo
  is 100% MIT-owned code. The README states users must comply with upstream's PolyForm Noncommercial
  terms (noncommercial use).
- No AMD GPUs. No macOS < 15.5. No Apple Silicon. No Optimus display-mux promises (the internal
  panel is wired to the iGPU — documented limitation, not a bug).
- Not a dortania-guide replacement for exotic hardware. `forge` targets the hardware classes we can
  validate (see §5); anything else gets the tailored report + guide links, not a guess-EFI.
- **Anti-goal: bricking a boot.** If backups can't be made or verified, the tool refuses to proceed.
- **Anti-goal: stale fork.** We never vendor upstream releases; users always get the latest driver.

## 5. Hardware scope (author's call — flagged, not the owner's decision)

v1 validates **Intel laptops/desktops with NVIDIA Turing dGPUs** — the class of the reference
machine (Comet Lake + GTX 1650 Mobile). The detection/report layer is written generically so more
hardware can be added with data, not rewrites, but the tool only *automates* what has been validated
on real hardware, and says so. Rationale: shipping untested EFI generation for arbitrary hardware is
how solo projects drown in support.

## 6. Architecture (proposed — builder refines)

Single Rust binary, three subcommands (`doctor`, `fix`, `forge`), wizard-style with confirmations
by default and a `--yes` unattended mode. Library modules:

| Module | Job |
|---|---|
| `detect` | PCI/hardware enumeration per OS (macOS: `ioreg`/`system_profiler`; Linux: sysfs/lspci; Windows: WMI/SetupAPI) |
| `occonfig` | `config.plist` model + safe edit ops (backup → diff → apply), NVRAM/boot-arg merging |
| `fetch` | GitHub releases API + SHA-256 verification (NullMoth, OpenCore, kexts) |
| `esp` | find/mount EFI system partition per OS (macOS `diskutil`, Linux mount, Windows `mountvol`) |
| `kextdb` | hardware → required kexts/SSDTs/templates mapping, data-driven |
| `efi` | EFI assembly: OpenCore base + kexts + SSDTs + generated config + SMBIOS serials |
| `report` | the human-readable tailored reports and proof tables |
| `nullmoth` | NullMoth release discovery, verification, install/uninstall, `--check` verification |

Key design points:

- **Validation by diff.** The reference laptop has a known-good live EFI. `forge` output for that
  machine is diffed against it in tests; the Audience-A `fix` path is tested against the broken
  config we actually started with.
- **EFI generation strategy:** download stock official components + template the config per detected
  hardware + include standard known-good SSDTs (EC/PLUG/PNLF class). No ACPI compiler in v1; if a
  machine needs something beyond the standard set, `forge` says so and scopes down to a report.
- **Privilege:** macOS driver install and ESP writes need root — the tool re-execs with `sudo` and
  says why; on Windows it asks for an elevated shell; plain `doctor` runs unprivileged everywhere.
- **Rollback parity with the manual playbook** (INSTALL-GUIDE §6): the tool's backups + printed
  OpenShell/firmware-menu recovery steps are part of every run's output.
- Rust toolchain required to build (`cargo build --release`); prebuilt binaries via GitHub Releases
  come after v1 proves out.

## 7. Optional modules (post-v1 hooks, kept out of the core)

- HDMI/DP audio: depends on tonight's reboot test (`built-in` property on the HDAU path) — if it
  works, `fix` offers it as a one-click extra; if not, the codeless injector kext becomes a module.
- External-monitor brightness: offer MonitorControl install (software dimming) until NVRMFB exposes
  DDC/CI.
- The 1401 redesign CSS stays a separate upstream PR — not bundled.

## 8. Repo reorientation

`gtx1650-sequoia` becomes the tool's repo: CLI at the root, the validated knowledge retained as
`docs/INSTALL-GUIDE.md`, `docs/UPSTREAM-ISSUE.md`, `1401-redesign/` (unchanged), and this design
under `docs/superpowers/specs/`. README rewritten around the tool, keeping the proof table and the
Lenovo validation report as the credibility section. **Decided: the CLI/brand name is `greenlight`;
the repo slug is `nvidia-macos-opencore`** — searchable keywords for discoverability, brand kept on
the binary (GitHub keeps redirects from `gtx1650-sequoia` and `greenlight` indefinitely).

## 9. Success criteria

1. On the reference laptop: fresh macOS + clone + `cargo run -- fix` + reboot → 2560×1440 @ 144 Hz
   external with Metal 3, verified by `fix --check`.
2. On Windows on the same laptop: `forge` produces an EFI that boots macOS on the Intel iGPU with
   the NVIDIA path already clean — validated against the known-good EFI.
3. `doctor` on an unsupported machine produces an honest, useful report and changes nothing.
4. Every mutation is reversible from the tool's own printed instructions, without this document.

## 10. Left to the builder

Crate choices (CLI parsing, plist, HTTP, JSON, PCI binding per OS), module/file layout, test
fixtures, CI, error-handling style, naming internals. The builder should prefer well-maintained
crates and keep the data-driven tables (`kextdb`, boot-arg sets, template configs) easy to extend
without code changes.

---

## 11. Builder-review settlements (author's calls)

Reviewed by a fresh builder subagent on 2026-10-11. Its eight planning questions are settled as
follows. Naming was the owner's remaining call: **greenlight** for the CLI/brand, with the repo
renamed to the searchable slug **`nvidia-macos-opencore`**.

1. **`forge` is two-stage.** INSTALL-GUIDE §7 proves a macOS installer needs the *opposite*
   BAR/IONDRVSupport settings from the NVIDIA runtime set, and Audience B's first boot *is* the
   installer. So `forge` emits an installer-safe EFI; after macOS is installed and booted, the user
   runs `fix` to apply the NVIDIA set. `doctor` announces which stage a machine is in. One config
   never tries to serve both installer and runtime.
2. **The automation gate is a validated-profile match.** `fix`/`forge` act automatically only for
   hardware profiles validated on real machines (v1: the Comet Lake + GTX 1650 Mobile Optimus
   profile of the reference laptop). Same-class but unvalidated hardware (other Turing laptops)
   → warn and require `--force`. Unknown hardware, and Turing **desktops** (different display/boot
   path — no iGPU-driven boot screen, the BAR1/`nvrmsettle` reasoning doesn't transfer) →
   report-only. This is §5 made mechanical.
3. **`NullMothSafe.efi` is not in the NullMoth release tarball** (verified on disk: it ships inside
   the 1401.app DMG). `fix` adds the picker recovery entry only when it can fetch the official DMG
   and verify it; otherwise it installs the printed OpenShell recovery steps (INSTALL-GUIDE §6
   Tier 2) and notes the gap. Worth flagging upstream: the tarball should ship the .efi.
4. **Version policy: NullMoth = always latest; OpenCore and the support kexts = pinned, validated
   versions with hashes curated in `kextdb`.** Those upstreams publish no SHA-256 sums, and a
   silently shifting EFI base violates the anti-brick anti-goal. Pins move forward via repo PRs
   after validation — curation cadence owned by this project.
5. **`fix` is hardware-parameterized, not a fixed patch.** `SecureBootModel=Disabled` is required
   per the upstream README, but the tool asks explicitly before downgrading a machine that has it
   enabled. `nvrmsettle=10000` is applied only when the BAR1-placement failure is detected (as on
   the reference machine), not unconditionally.
6. **Install execution: shell out to the official, hash-verified `install.sh` as root.** No Rust
   reimplementation — that would be a standing drift risk against upstream, and the script already
   provides the rollback snapshot machinery (INSTALL-GUIDE §6 Tier 1). Dry-run prints what the
   script will do rather than simulating it.
7. **`forge` delivers the EFI folder only.** Creating the macOS installer itself (gibMacOS-class)
   is out of scope; the tool prints the correct dortania-style installer steps for the OS it's
   running on.
8. **Live validation runs on the reference laptop are executed by the builder with the owner
   present and consenting per reboot** — the same working mode this whole project has used.
