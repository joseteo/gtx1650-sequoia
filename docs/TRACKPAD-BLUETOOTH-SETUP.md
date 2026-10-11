# Trackpad + Bluetooth on the IdeaPad Gaming 3 — Replication Guide & Setup Log

> Canonical home: https://github.com/joseteo/nvidia-macos-opencore · companion to [INSTALL-GUIDE.md](INSTALL-GUIDE.md) (GTX 1650 / NullMoth)

Date: 2026-10-10 → 2026-10-11
Machine: **Lenovo IdeaPad Gaming 3 15IMH05** (board LNVNB161216), i7-10750H, macOS 15.7.4 (24G517), OpenCore, SMBIOS MacBookPro16,4

| Device | Hardware | Status |
|---|---|---|
| Trackpad | Synaptics I2C-HID on `\_SB.PCI0.I2C1.TPD0` | ✅ **Working** — native Precision Touchpad, gestures included |
| Bluetooth | Intel AX201 BT, USB `8087:0026` on HS14 | ✅ **Working** — real address, firmware `v256 c256`, pairs devices |

No new driver code was needed — everything is configuration around proven kexts, the same wrap-don't-rewrite philosophy as itlwm and NullMoth. **Part 1 is the replication recipe. Part 2 is why each piece is needed.**

---

# Part 1 — Replicate from a basic OpenCore install

Scope: a Hackintosh that already boots macOS 15.x Sequoia via OpenCore on this laptop (base kext set — Lilu, VirtualSMC, WhateverGreen, AppleALC, itlwm, etc. — already working). This recipe adds **only** the trackpad and Bluetooth deltas.

## 1.1 Files to place in the EFI

| File | Version used | Where it comes from |
|---|---|---|
| `VoodooI2C.kext` | 2.9.1 | [VoodooI2C/VoodooI2C](https://github.com/VoodooI2C/VoodooI2C) release |
| `VoodooI2CHID.kext` | 2.9.1 (same package) | same |
| `SSDT-GPI0.aml` | board-specific | [isg32 EFI](https://github.com/isg32/ideapad-gaming-3-macos-15IMH05) `EFI/OC/ACPI/` |
| `SSDT-TPD0.aml` | board-specific | same |
| `BlueToolFixup.kext` | 2.7.2 | [acidanthera/BrcmPatchRAM](https://github.com/acidanthera/BrcmPatchRAM) release zip |
| `IntelBluetoothFirmware.kext` | **2.5.0** (master build — see warning) | [Hackintosh-Kext-Factory](https://github.com/ClayMoreBoy/Hackintosh-Kext-Factory) bundle: `release/IntelBluetoothFirmware/{Release,Debug}/`, or build OpenIntelWireless master |
| `IntelBTPatcher.kext` | 2.4.0 | OpenIntelWireless release (already in most EFIs) |

> ⚠️ **Do not use IntelBluetoothFirmware 2.4.0 on Sequoia** — it is the newest *tagged* release but predates macOS 15; on this machine it attaches to the chip and then silently never uploads firmware. Use a 2.5.0 master build. The Debug build logs to `sudo dmesg | grep -i IntelFirmware`; Release is drop-in identical otherwise.
>
> ⚠️ **The two SSDTs are specific to this board** (LNVNB161216: GPIO pin, I2C address `0x2c` on `I2C1`). For any other laptop model, generate them from that machine's own DSDT (see VoodooI2C's GPIO-pinning docs) — do not copy these.

Placement:

```
EFI/OC/Kexts/BlueToolFixup.kext           ← add
EFI/OC/Kexts/IntelBluetoothFirmware.kext  ← replace with 2.5.0
EFI/OC/Kexts/VoodooI2C.kext               ← add (with its PlugIns intact)
EFI/OC/Kexts/VoodooI2CHID.kext            ← add
EFI/OC/ACPI/SSDT-GPI0.aml                 ← add
EFI/OC/ACPI/SSDT-TPD0.aml                 ← add
```

## 1.2 config.plist — Kernel → Add

Fastest path: open `config.plist` in **ProperTree**, `Cmd/Ctrl+R` (OC Clean Snapshot) pointing at `OC/` — it auto-adds kexts *including their PlugIns*, which is the step hand-built EFIs usually miss. Then verify the final state matches this table exactly (order and Enabled flags matter):

```
Lilu.kext                                                  true   (must be first overall)
…(your existing base kexts)…
VoodooPS2Controller.kext                                   true
VoodooPS2Controller.kext/…/VoodooInput.kext                true
VoodooPS2Controller.kext/…/VoodooPS2Keyboard.kext          true
VoodooPS2Controller.kext/…/VoodooPS2Mouse.kext             false  ← I2C trackpad, not PS/2
VoodooPS2Controller.kext/…/VoodooPS2Trackpad.kext          false  ← I2C trackpad, not PS/2
VoodooI2C.kext                                             true
VoodooI2C.kext/Contents/PlugIns/VoodooGPIO.kext            true   ← easy to miss
VoodooI2C.kext/Contents/PlugIns/VoodooI2CServices.kext     true   ← easy to miss, fatal
VoodooI2CHID.kext                                          true
IntelBluetoothFirmware.kext                                true
BlueToolFixup.kext                                         true
IntelBTPatcher.kext                                        true
```

If editing by hand instead, the two plugin entries verbatim (paste-ready dicts):

```xml
<dict>
    <key>Arch</key><string>x86_64</string>
    <key>BundlePath</key><string>VoodooI2C.kext/Contents/PlugIns/VoodooGPIO.kext</string>
    <key>Comment</key><string>VoodooI2C GPIO interrupt support</string>
    <key>Enabled</key><true/>
    <key>ExecutablePath</key><string>Contents/MacOS/VoodooGPIO</string>
    <key>MaxKernel</key><string></string><key>MinKernel</key><string></string>
    <key>PlistPath</key><string>Contents/Info.plist</string>
</dict>
<dict>
    <key>Arch</key><string>x86_64</string>
    <key>BundlePath</key><string>VoodooI2C.kext/Contents/PlugIns/VoodooI2CServices.kext</string>
    <key>Comment</key><string>VoodooI2C core services library (required)</string>
    <key>Enabled</key><true/>
    <key>ExecutablePath</key><string>Contents/MacOS/VoodooI2CServices</string>
    <key>MaxKernel</key><string></string><key>MinKernel</key><string></string>
    <key>PlistPath</key><string>Contents/Info.plist</string>
</dict>
```

## 1.3 config.plist — ACPI → Add

```xml
<dict>
    <key>Comment</key><string>Enable GPIO controller for I2C trackpad interrupts</string>
    <key>Enabled</key><true/>
    <key>Path</key><string>SSDT-GPI0.aml</string>
</dict>
<dict>
    <key>Comment</key><string>Trackpad (I2C1/TPD0) GPIO pin config for VoodooI2C</string>
    <key>Enabled</key><true/>
    <key>Path</key><string>SSDT-TPD0.aml</string>
</dict>
```

## 1.4 config.plist — ACPI → Patch (the one that makes SSDT-TPD0 load at all)

```xml
<dict>
    <key>Base</key><string></string>
    <key>BaseSkip</key><integer>0</integer>
    <key>Comment</key><string>Rename _DSM to XDSM in DSDT (required by SSDT-TPD0)</string>
    <key>Count</key><integer>0</integer>
    <key>Enabled</key><true/>
    <key>Find</key><data>X0RTTQ==</data>          <!-- "_DSM" -->
    <key>Limit</key><integer>0</integer>
    <key>Mask</key><data></data>
    <key>OemTableId</key><data></data>
    <key>Replace</key><data>WERTTQ==</data>       <!-- "XDSM" -->
    <key>ReplaceMask</key><data></data>
    <key>Skip</key><integer>0</integer>
    <key>TableLength</key><integer>0</integer>
    <key>TableSignature</key><data>RFNEVA==</data> <!-- "DSDT" — scoped to DSDT only -->
</dict>
```

## 1.5 config.plist — NVRAM → Add → `7C436110-AB2A-4BBB-A880-FE41995C9F82`

```xml
<key>bluetoothExternalDongleFailed</key><data>AA==</data>                 <!-- 1 zero byte -->
<key>bluetoothInternalControllerInfo</key><data>AAAAAAAAAAAAAAAAAAA=</data> <!-- 14 zero bytes -->
```

And in the same GUID's `boot-args`, append (keep your existing args untouched):

```
-btlfxallowanyaddr
```

> **⚠ Do not regenerate `boot-args` from a template — append only.** On the GTX 1650 display setup the string must keep `nvrmsettle=10000` (dropping it = external monitor never arms, no error). Likewise never re-add `class-code=<FFFFFFFF>` / `disable-gpu` DeviceProperties on the dGPU path (`PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0)`) — that silently disables the NVIDIA driver entirely. Both clobbers happened on 2026-10-11 and cost a boot cycle each. See `INSTALL-GUIDE.md` §2 and §5.

## 1.6 Apply, reboot, verify

1. `sudo diskutil mount <esp>` — Sequoia refuses EFI mounts for non-root users (`storagekitd: skipping EFI partition owned by uid 0`); the ESP also unmounts at every reboot.
2. Back up: copy `EFI/OC/config.plist` to `config.plist.bak` **on the EFI itself**, plus a full EFI copy elsewhere.
3. Apply the edits; `plutil -lint config.plist` must say `OK`.
4. **Cold boot**: Shut Down → wait ~10 s → power on. (A warm restart can leave the Intel BT chip in a stale firmware state.)
5. Verify:

```bash
# Trackpad — expect VoodooI2CHID loaded and this registry chain:
kmutil showloaded | grep -i voodoo
ioreg -p VoodooI2C -w0 | head -20
#   I2C1 → VoodooI2CPCILakeController → …→ TPDX → VoodooI2CHIDDevice
#   → VoodooI2CPrecisionTouchpadHIDEventDriver → VoodooI2CNativeEngine

# Bluetooth — expect State On, a real address, non-zero firmware:
system_profiler SPBluetoothDataType | head -12
#   Address: xx:xx:xx:xx:xx:xx  State: On  Chipset: THIRD_PARTY_DONGLE  Firmware Version: v256 c256
```

`THIRD_PARTY_DONGLE` is the expected chipset label for Intel BT on this kext stack. Note: Intel BT + itlwm Wi-Fi covers mice/keyboards/audio; AirDrop/Handoff/Continuity require Broadcom hardware.

## 1.7 If it fails

| Symptom | Read this |
|---|---|
| Trackpad dead, keyboard fine | `log show --last boot \| grep -i voodoo` — a `VoodooI2CServices not found` line means §1.2's plugin entries are missing; `ACPI Error … AE_ALREADY_EXISTS` means §1.4's rename is missing |
| BT: `Address NULL`, `v0 c0`, toggle flips off | Check `ioreg -p IOUSB -l \| grep -i FirmwareLoaded` — absent means the firmware kext never finished (wrong kext version — use 2.5.0, §1.1 warning) |
| Any change went wrong | Boot another OS, restore `config.plist.bak` → `config.plist` |

---

# Part 2 — Why each piece is needed (the debugging record)

## 2.1 Trackpad — three stacked defects, all required

**Defect 1 — VoodooI2C never loaded.** The config enabled `VoodooI2C.kext` + `VoodooI2CHID.kext`, but OpenCore injects only what is explicitly listed; the bundled plugin kexts were missing, so dependency resolution failed at boot:

```
Kext com.alexandred.VoodooI2C - library kext com.alexandred.VoodooI2CServices not found.
Can't load kext com.alexandred.VoodooI2C - failed to resolve library dependencies.
```

**Defect 2 — the trackpad SSDTs were absent.** `SSDT-GPI0` enables the GPIO controller under macOS; `SSDT-TPD0` defines `TPDX`, the macOS-visible trackpad device carrying the I2C address and GPIO interrupt pin. Both self-disable under non-macOS (`_OSI("Darwin")` guards; `TPDX` presents `_HID "XXXX0000"` + `_STA 0` elsewhere) — safe for dual boot.

**Defect 3 — SSDT-TPD0 needs the `_DSM`→`XDSM` rename.** The SSDT wraps the DSDT's existing `TPD0._DSM` and calls it as `XDSM`. Without the rename, the injected `_DSM` collides with the DSDT's (`ACPI Error: Namespace lookup failure, AE_ALREADY_EXISTS`), and the table **aborts mid-load**: its early `TPTY = 0` store still executes and *disables the original `TPD0`*, while `TPDX` is never created — the SSDT kills the trackpad device and fails before installing its replacement. The reference EFI does this rename globally; here it is scoped to the DSDT table only.

Verified post-fix: `TPDX` in IOACPIPlane, and the full chain `I2C1 → VoodooI2CPCILakeController → VoodooI2CControllerNub → VoodooI2CControllerDriver → TPDX (VoodooI2CDeviceNub) → VoodooI2CHIDDevice → IOHIDInterface → VoodooI2CPrecisionTouchpadHIDEventDriver → VoodooI2CMultitouchInterface → VoodooI2CNativeEngine`. The Precision Touchpad event driver is the proof macOS treats it natively — gestures, no mouse emulation.

## 2.2 Bluetooth — IntelBluetoothFirmware 2.4.0 predates Sequoia

Symptoms were `Address NULL / State Off / Chipset BCM_4350C2 / Firmware v0 c0` with a toggle that flips back off — the signature of **no firmware ever loaded** (a successful upload makes the chip USB-reset and re-enumerate with a new `bcdDevice`, which never happened).

The first gap was real: `BlueToolFixup.kext` (required on macOS 12+) was missing entirely. But even with the full 2.4.0-era trio loaded, the upload never completed.

Dead ends worth recording:

- **The installed kext was not stripped.** It's byte-identical to the official v2.4.0 release — firmware blobs are embedded in the 11.5 MB binary, not shipped as loose `.sfi` files.
- **The failure is silent by design.** The kext logs through zxystd's `XYLog` macro, compiled to nothing in release builds — `sudo dmesg | grep IntelFirmware` returns nothing even though the driver runs. The observable tell is the registry: the driver publishes `FirmwareLoaded`/`fw_name` on the USB device node when `setup()` completes, and it was absent (`ioreg -p IOUSB`).

The fix (batched in one boot cycle; all three are community-validated for Sequoia + AX201): **kext 2.4.0 → 2.5.0 master build** (believed decisive — 2.4.0 predates macOS 15), **the two NVRAM variables** (make the BT stack treat the controller as internal and re-read its address), and **`-btlfxallowanyaddr`** (reported ineffective without the NVRAM pair). Individual necessity was not isolated — if minimizing, drop the boot-arg first, then the NVRAM pair, and keep 2.5.0.

Confirmed *not* the cause: Windows Fast Startup chip-wedging (cold boot changed nothing on 2.4.0), USB port absence (chip enumerates every boot), BlueToolFixup absence alone (necessary, not sufficient).

## 2.3 The Windows BSOD during testing — unrelated

A blue screen appeared once when Windows was chain-booted mid-testing. The SSDTs cannot affect Windows (Darwin-only guards), and the signature fits a Windows Fast Startup hibernate-resume failure (`hiberfil.sys` + hard reboot). One clean full boot clears it; disabling Fast Startup in Windows removes the class.

## 2.4 Safety rails

- Live EFI backed up (`~/Documents/drivers/EFI-backup-live-20261010/`) before any write; pre-change config kept on the EFI as `config.plist.bak-bttouch`, pre-NVRAM config as `config.plist.bak-round2`.
- `Misc → Debug → Target = 67` on this EFI: OpenCore writes `opencore-*.txt` to the ESP root each boot — read it when a table fails to load.

## 2.5 Sources

- [isg32/ideapad-gaming-3-macos-15IMH05](https://github.com/isg32/ideapad-gaming-3-macos-15IMH05) — working EFI for this exact board (SSDTs, version pins; its `_DSM→XDSM` rename is global, DSDT-scoped here)
- [luchina-gabriel/EFI-NOTEBOOK-LENOVO-IDEAPAD-GAMING-3i](https://github.com/luchina-gabriel/EFI-NOTEBOOK-LENOVO-IDEAPAD-GAMING-3i-10300H-10750H-i5-i7-iGPU) — original SSDT source
- [OpenIntelWireless/IntelBluetoothFirmware](https://github.com/OpenIntelWireless/IntelBluetoothFirmware) + [issue #441](https://github.com/OpenIntelWireless/IntelBluetoothFirmware/issues/441) (healthy `dmesg` trail), [ClayMoreBoy/Hackintosh-Kext-Factory](https://github.com/ClayMoreBoy/Hackintosh-Kext-Factory) (2.5.0 builds)
- [acidanthera/BrcmPatchRAM](https://github.com/acidanthera/BrcmPatchRAM) — BlueToolFixup
- [VoodooI2C/VoodooI2C](https://github.com/VoodooI2C/VoodooI2C) — I2C HID stack
- [amd-osx.com Sequoia Intel-BT thread](https://forum.amd-osx.com/threads/finally-intel-bluetooth-working-under-sequoia-15-0-24a335.5430/) — the 2.5.0 + NVRAM + boot-arg recipe
