# GTX 1650 Mobile + NullMoth `nvidia-macos-driver` — Install Log & Replication Guide

> Canonical home: https://github.com/joseteo/nvidia-macos-opencore · upstream hardware report: [nullmoth/nvidia-macos-driver#104](https://github.com/nullmoth/nvidia-macos-driver/issues/104)

Date: 2026-10-10
Driver: **NullMoth NVIDIA driver 1.10.0** (`nullmoth-nvidia-1.10.0.tar.gz`, GSP firmware `610.57.04`)
Repo: https://github.com/nullmoth/nvidia-macos-driver
Result: **External monitor 2560×1440 @ 144 Hz over HDMI, Metal 3, driven by the GTX 1650 — fully working.**

Keep this file. It is both the report for upstream and the playbook to rebuild this machine from scratch.

---

## 1. Machine & software inventory (verified)

| Item | Value |
|---|---|
| Laptop | Lenovo Optimus/MSHybrid laptop (PCI subsystem `17aa:3fa2`) |
| CPU | Intel Core i7-10750H (Comet Lake) |
| iGPU | Intel UHD Graphics 630 (`8086:3e9b`) — drives the internal 1080p panel (hardware wiring, always Intel) |
| dGPU | NVIDIA GeForce GTX 1650 Mobile, TU117M, `10de:1f99` rev a1, 4 GB VRAM, PCIe Gen3 x16 at `01:00.0` (`PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)`, ACPI name `GFX0`) |
| dGPU audio fn | `10de:10fa` HD-audio controller at `01:00.1` (`.../Pci(0x0,0x1)`, ACPI name `HDAU`) |
| BAR1 | 256 MB; ReBAR capability exists but the card offers no 4–8 GB size ("not placed" — expected) |
| External monitor | Gigabyte GS27QA, 2560×1440 @ 144 Hz over HDMI; its speakers/jack verified working in Windows and Ubuntu |
| macOS | 15.7.4 (24G517), x86_64 |
| OpenCore | 1.0.6 (REL-106-2025-11-03), on EFI partition `disk1s1` (label "SYSTEM") |
| SMBIOS | MacBookPro16,4 |
| Boot stack kexts | Lilu 1.7.2, VirtualSMC, WhateverGreen 1.7.0, AppleALC 1.9.7, AMFIPass 1.4.1, + the usual laptop set (VoodooI2C/PS2, itlwm, Intel BT, RealtekRTL8111, NVMeFix, CPUFriend, ECEnabler, BrightnessKeys) |
| Other OSes | Windows (disk0, NVMe) and Ubuntu — see §7 for the Windows boot rule |

---

## 2. The one finding that made it work (read this first)

This machine's OpenCore config contained a leftover dortania "disable the dGPU" block:

```
DeviceProperties → Add → PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)
    class-code   = <FFFFFFFF>
    disable-gpu  = <01000000>
```

Effect: the faked `class-code` made NVRM's `IOPCIClassMatch` never match the card. **Every NullMoth kext was correctly installed and the config was otherwise perfect, yet NVRM never even probed** — silent failure, no error anywhere. Deleting that DeviceProperties entry (and nothing else) took the machine from "driver never loads" to "1440p@144 on HDMI".

- The upstream README already says *"No DeviceProperties are needed for the NVIDIA card"* — this confirms it, including for properties added long before the driver.
- The 1401 app neither detects nor remediates this leftover. Flagged to upstream, see `UPSTREAM-ISSUE-DRAFT.md`.
- Same class of bug as upstream issues #38 (TU117 desktop) and #45 (TU116): driver present, never loads.

---

## 3. Final OpenCore config delta (the exact working state)

Everything below is **verified against the live `EFI/OC/config.plist`** on 2026-10-10. Base config = the pre-driver Hackintosh config (backup `config.plist.bak-amfipass-20261010-100229` on the EFI).

### NVRAM → Add → `7C436110-AB2A-4BBB-A880-FE41995C9F82`

| Key | Value | Notes |
|---|---|---|
| `boot-args` | `-v keepsyms=1 debug=0x100 alcid=11 npci=0x3000 igfxonln=1 dart=0 igfxfw=2 agdpmod=vit9696 nvfb=1 nvaccel=1 nvfbheads=4 -nvkmsnosmooth amfi_get_out_of_my_way=0x1 amfi=0x80 nvrmsettle=10000` | see per-arg table below |
| `csr-active-config` | data `<430A0000>` (bytes `43 0A 00 00` = 0x0A43) | unsigned/unapproved kexts + unauthenticated root + unrestricted NVRAM/FS — the README's tested value |

Both keys are also listed under `NVRAM → Delete` (rewritten every boot).

boot-args, piece by piece:

| Arg | From | Why |
|---|---|---|
| `nvfb=1 nvaccel=1 nvfbheads=4 -nvkmsnosmooth` | NullMoth README | enable framebuffer + accelerator, 4 display heads |
| `amfi_get_out_of_my_way=0x1 amfi=0x80` | NullMoth README | lets WindowServer load the unsigned GPUBundle |
| `nvrmsettle=10000` | this machine | 10 s settle before display arm, because BAR1 (256 MB) can't be moved off the boot screen; matches kernel log `auto-go: go(2) in 10000 ms` |
| `-v keepsyms=1 debug=0x100` | pre-existing | verbose + panic debugging (keep while experimenting) |
| `alcid=11` | pre-existing | AppleALC layout 11 for the onboard Realtek audio |
| `npci=0x3000 igfxonln=1 dart=0 igfxfw=2 agdpmod=vit9696` | pre-existing | laptop/iGPU baseline; do not add `agdpmod=pikera` or any NVIDIA WhateverGreen patch (README: remove them) |

### Other config changes vs. the pre-driver backup

| Where | Setting | Value |
|---|---|---|
| `Kernel → Add` | `AMFIPass.kext` v1.4.1 | Enabled, `Arch=Any`, `MinKernel=20.0.0` (file in `EFI/OC/Kexts`) |
| `Kernel → Block` | `com.apple.iokit.IONDRVSupport`, Strategy `Exclude` | keeps the firmware framebuffer off display 0 so NVRMFB takes it (README). Note: IONDRVSupport still loads on macOS 15.7 — harmless; `NVRMFBClaim` handles the ordering |
| `UEFI → Quirks` | `ResizeGpuBars` | `13` |
| `Booter → Quirks` | `ResizeAppleGpuBars` | `-1` (was already) |
| `Misc → Security` | `SecureBootModel` | `Disabled` (was already) |
| `Misc → Tools` | entry **"1401: Remove NVIDIA driver"** → `NullMothSafe.efi` (Auxiliary) | picker recovery entry; the .efi sits in `EFI/OC/Tools` |
| `DeviceProperties → Add` | **deleted** `PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)` (class-code/disable-gpu) | THE fix, see §2 |
| `DeviceProperties → Add` | kept: `PciRoot(0x0)/Pci(0x2,0x0)` (iGPU platform/stolenmem) and `PciRoot(0x0)/Pci(0x1F,0x3)` (`layout-id` 11) | Intel/iGPU + onboard audio, untouched |

Do **not** inject the NullMoth kexts from `EFI/OC/Kexts` — they must load from the Auxiliary Kernel Collection (they link Apple frameworks that only exist in macOS's own kernel collections).

---

## 4. How it was actually installed (manual "prebuilt" path)

The 1401.app GUI was **not usable here** (see §8): it only enumerates the Intel iGPU on this Optimus laptop and refuses install with "needs a GeForce RTX 20 series or newer". So the README's manual path was used:

1. Downloaded `nullmoth-nvidia-1.10.0.tar.gz` + `SHA256SUMS.txt` → `~/Documents/drivers/`; verified the tarball (`47f9cd65…ccb5`) and extracted `pkgroot/`; every file in `SHA256SUMS` checked `OK`.
2. Copied `pkgroot/` to `/Users/Shared/nullmoth-stage/pkgroot` and ran `install.sh` **as root** (see §9 for why scripts run from `/Users/Shared`, not `~/Documents`). It copies:
   - `/Library/Extensions/NVRM.kext`, `NVAccel.kext`, `NVRMFB.kext`, `NVRMAGDC.kext` (all bundle-version 0.1)
   - `/Library/GPUBundles/` (NVMTLDriver.bundle, `nvmtl/`, NVIDIAShared.bundle)
   - `/Users/Shared/nvfw/nvidia/610.57.04/` (GSP firmware, NVIDIA's, unmodified)
   then rebuilds the Auxiliary Kernel Collection with `kmutil`.
3. Made the §3 config edits on the EFI (each change backed up first), added `AMFIPass.kext` + `NullMothSafe.efi`.
4. Reboot → first boot pauses ~10 s at PCI configuration end (expected: BAR1 not placed → settle timer) → desktop on the external monitor.
5. macOS never showed a "allow extensions" prompt here (SIP `0x0A43` already permits it).

### Verification evidence (this machine, 2026-10-10)

```
kextstat: com.nullmoth.NVRM / NVAccel / NVRMFB / NVRMAGDC  all loaded
system_profiler SPDisplaysDataType:
  NVIDIA GeForce GTX 1650, 4 GB, Metal 3, Bus PCIe x16
  GS27QA: 2560 x 1440 @ 144.00Hz, Main Display: Yes
kernel log (boot):
  NVRM-xnu: probe 10de:1f99 class 03.00 subsystem 17aa:3fa2
  NVRM-xnu: rm_is_supported_pci_device OK; RM names 10de:1f99 "NVIDIA GeForce GTX 1650"
  NVRM-xnu: bar1: BAR1 is 256 MB and the card offers no 4-8 GB size — not placing
  NVRM-xnu: armed on 01:00.0 10de:1f99
  NVRM-xnu: auto-go: go(2) in 10000 ms ... (BAR1 not placed)
  NVAccel: probe(GFX0) score 100 / claiming / starting
```

Full boot log saved at `~/Documents/drivers/bootlog-20261010.txt`.

---

## 5. Known limitations & caveats

- **Internal panel stays on the Intel iGPU** (Optimus: panel is wired to Intel; upstream explicitly does not implement an Optimus display mux). The dGPU drives only the external HDMI output.
- **Turing is "validation pending" upstream** — our report is the first GTX 1650 Mobile data point; expect rough edges.
- **Battery/heat**: with RM keeping the dGPU armed, idle power is worse than the dGPU-disabled config.
- **macOS updates**: remove the driver (§6) before any macOS update; re-verify before re-enabling.
- **Manual-install gap**: because the 1401 app wasn't used, `/Library/NullMoth/state` and the `com.nullmoth.recover` LaunchDaemon do **not** exist. Consequence: the picker's "1401: Remove NVIDIA driver" entry still adds `-nvoff` for the next boot, but the *automatic* config restore won't run — use the manual steps in §6.
- **HDMI/DP audio: not working yet** — see §10 (root cause found, fix in testing).

---

## 6. Recovery / rollback playbook (keep!)

Tier 1 — driver misbehaves but macOS boots: boot picker → **1401: Remove NVIDIA driver** → macOS starts with `-nvoff` (driver parked). Then, booted:

```bash
sudo bash /Users/Shared/nullmoth-stage/pkgroot/uninstall.sh
# rollback snapshot made by the installer: /Library/NullMoth/backup-20261010-195043.USAdSW
```

Tier 2 — macOS won't boot: boot picker → **OpenShell** →

```
fs0:
cd EFI\OC
cp config.plist.bak-amfipass-20261010-100229 config.plist
reboot
```

(that backup is the last known-good pre-driver config; the intermediate backups `…-nvdriver-…` and `…-nvclasscode-…` are next to it, plus older `bak-preNVramReset-20260824`, `bak-preagdp-20260824`.)

Tier 3 — can't reach the picker: boot **Windows via the F12 firmware menu**, `mountvol S: /S`, copy the same backup over `S:\EFI\OC\config.plist`.

Full EFI copies: `~/Documents/drivers/EFI-backup/` (session start) and `~/Documents/drivers/EFI-backup-live-20261010/` (post-install).

---

## 7. Windows dual-boot warning (blue screen already happened once)

`UEFI.Quirks.ResizeGpuBars=13` is applied **hardware-wide by OpenCore before any OS starts**. Booting Windows **through the OpenCore picker** therefore blue-screens this machine. Rule:

- **Windows: only via the F12 firmware boot menu** (BIOS boots Windows directly, no OpenCore quirks).
- **macOS: via the OpenCore picker → "Macintosh"** (5 s timeout, default).
- If a real macOS *installer/recovery* is ever run from this EFI, remember the README: installers need the opposite BAR settings (`ResizeAppleGpuBars=0`, `ResizeGpuBars=-1`, IONDRVSupport not excluded).

---

## 8. 1401.app gaps observed (for upstream)

1. On this Optimus laptop the app enumerates only the Intel iGPU ("This Mac: Graphics = Intel UHD Graphics 630") and gates on "needs a GeForce RTX 20 series or newer" → GTX 16 laptops can never install through the GUI.
2. No detection of a `class-code=<FFFFFFFF>` / `disable-gpu` DeviceProperties leftover, which silently nullifies the driver (§2).
3. USB mapping / crash-report features were not exercised.

---

## 9. Environment notes for anyone scripting on this machine

- Privileged changes are done via GUI-elevated shells:
  `osascript -e 'do shell script "/bin/bash /Users/Shared/nullmoth-stage/SCRIPT 2>&1" with administrator privileges'`
- Scripts must live in **`/Users/Shared/nullmoth-stage/`**, not `~/Documents` (TCC blocks script execution from Documents with exit 126).
- PlistBuddy `Set … <430A0000>` writes a literal ASCII string, not data bytes. For data values use `plutil -replace KEY -data QwoAAA==` (base64).
- Boot EFI is `disk1s1` ("SYSTEM"); mount/unmount with `diskutil mount disk1s1` / `diskutil unmount disk1s1` (as root). Files copied off FAT32 land mode 700 root — `chmod 644` before reading them as the user.

---

## 10. HDMI audio — status and root cause (2026-10-10)

Symptom: no HDMI/DP output device in `system_profiler SPAudioDataType`; only onboard Realtek.

Findings (all verified this session):

- The NVIDIA audio function is healthy at PCI level: `HDAU@0,1` (`10de:10fa`, class `0x040300`) is registered/powered, WhateverGreen already injected `hda-gfx="onboard-2"` and `layout-id=7`.
- **No driver attaches to HDAU at all** (leaf node in IOService). `AppleHDAController` attaches only to onboard `HDEF@1F,3`.
- `AppleGFXHDA.kext` (Apple's GPU HDMI-audio driver) has personalities for AMD + Intel device IDs only — nothing for NVIDIA.
- `AppleHDAController`'s "BuiltInHDA" personality class-matches HDA controllers (`0x04010000&0xFFFD0000` covers `0x040300`), so it *should* probe HDAU — but doesn't attach ⇒ rejected at probe time on macOS 15.7 ("BuiltIn*" naming: Apple only ships built-in-audio Macs now).
- A kernel class `NVHDAEnabler` is registered (0 instances) but its string exists in **no kext on disk** (`/Library` or `/System`) — dormant NullMoth-side code; asked upstream.
- Monitor speakers/jack are proven good in Windows and Ubuntu over the same cable, so this is purely macOS-side.

Next experiment (planned, one reboot, fully revertible): add
`DeviceProperties → PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x1) → built-in = <01000000>`
to test whether `AppleHDAController::probe` accepts the HDAU when marked built-in. Fallbacks if that fails: a codeless injector kext matching `10de:10fa` with `IOClass=AppleHDAController` (then `AppleGFXHDAEGController`), tested via runtime `kmutil load` before persisting.

---

## 11. From-scratch replication (fresh macOS install → working driver)

1. Working OpenCore 1.0.6 + macOS 15.7.x on the Intel iGPU first (SMBIOS MacBookPro16,4). BIOS: Above 4G Decoding **ON**, CSM **OFF**.
2. **Purge any dGPU-kill remnants**: no `class-code`/`disable-gpu` DeviceProperties on the dGPU path, no `nv_disable=1`, no `agdpmod=pikera` (§2).
3. Apply §3 (boot-args, csr `0x0A43` + NVRAM.Delete, Kernel.Block IONDRVSupport Exclude, ResizeGpuBars 13, ResizeAppleGpuBars -1, SecureBootModel Disabled, AMFIPass.kext in Kexts+Add, NullMothSafe.efi + Tools entry).
4. `shasum -a 256 -c SHA256SUMS` on the extracted `pkgroot`, then `sudo ./install.sh` from `/Users/Shared/...`.
5. Reboot. Expect a ~10 s pause at "PCI configuration end" (BAR1 settle) the first times.
6. Verify with the commands in §4. Keep §6 backups current before every future change.

## 12. File inventory (this folder + staging)

| Path | What |
|---|---|
| `~/Documents/drivers/nullmoth-nvidia-1.10.0.tar.gz`, `SHA256SUMS.txt` | verified payload (sha256 `47f9cd65…ccb5`) |
| `~/Documents/drivers/pkgroot/` | extracted payload |
| `~/Documents/drivers/1401-Mac-1.10.0.dmg`, `1401.app/` | the app (unusable on this machine, §8) |
| `~/Documents/drivers/AMFIPass/` | AMFIPass 1.4.1 source/kext |
| `~/Documents/drivers/EFI-backup/`, `EFI-backup-live-20261010/` | full EFI snapshots (before / after) |
| `~/Documents/drivers/bootlog-20261010.txt` | full verbose boot log (~51 MB) |
| `~/Documents/drivers/UPSTREAM-ISSUE-DRAFT.md` | report to post to the GitHub repo |
| `/Users/Shared/nullmoth-stage/` | scripts (`install-nvdriver.sh`, `fix-devprops.sh`, `verify-config.sh`, …), `pkgroot/` copy, `NullMothSafe.efi`, `config-live.plist` (verified live config copy) |
| EFI `EFI/OC/` | `config.plist` + `config.plist.bak-*` backups; `Kexts/AMFIPass.kext`; `Tools/NullMothSafe.efi` |
| `/Library/NullMoth/backup-20261010-195043.USAdSW` | installer's rollback snapshot |
