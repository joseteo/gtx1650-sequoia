# Hardware report — posted upstream as [nullmoth/nvidia-macos-driver#104](https://github.com/nullmoth/nvidia-macos-driver/issues/104) (2026-10-10, text as submitted, serial numbers removed)

**Title:** Hardware report (works): GTX 1650 Mobile (TU117M, 10de:1f99) Optimus laptop, macOS 15.7.4, driver 1.10.0 — 1440p@144 HDMI OK; 1401 app gaps; HDMI audio unclaimed

## Summary

First physical Turing-laptop data point: a GTX 1650 **Mobile** (TU117M, `10de:1f99`) in an Optimus/MSHybrid Lenovo laptop drives an external monitor at **2560×1440 @ 144 Hz over HDMI with Metal 3** under macOS 15.7.4 (24G517), OpenCore 1.0.6, driver 1.10.0 installed via the README "prebuilt" path. The internal panel stays on the Intel iGPU (as expected — no Optimus mux), which is fine for this use case.

## Environment

- Laptop: Lenovo (PCI subsystem `17aa:3fa2`), i7-10750H, UHD 630 (`8086:3e9b`) → internal panel
- dGPU: GTX 1650 Mobile `10de:1f99` rev a1, 4 GB, PCIe Gen3 x16; HDA audio function `10de:10fa` at `01:00.1`
- BAR1: 256 MB; ReBAR cap present but card offers no 4–8 GB size → "not placed" (expected)
- Monitor: Gigabyte GS27QA, HDMI, 2560×1440 @ 144 Hz
- macOS 15.7.4 (24G517) x86_64; OpenCore 1.0.6 (REL-106); SMBIOS MacBookPro16,4; SecureBootModel Disabled; SIP `0x0A43`
- Config per README: `nvfb=1 nvaccel=1 nvfbheads=4 -nvkmsnosmooth amfi_get_out_of_my_way=0x1 amfi=0x80` (+`nvrmsettle=10000`), `csr-active-config=<430A0000>` (+NVRAM.Delete), `Kernel.Block: IONDRVSupport Exclude`, `ResizeGpuBars=13`, `ResizeAppleGpuBars=-1`, AMFIPass.kext 1.4.1, NullMothSafe.efi picker entry
- Boot evidence: `probe 10de:1f99` → `rm_is_supported_pci_device OK` → `armed on 01:00.0` → `auto-go: go(2) in 10000 ms (BAR1 not placed)`; all four kexts loaded; WindowServer stable.

## The one non-obvious blocker (worth a check in 1401)

The OpenCore config had a dortania-style dGPU-disable leftover:

```
DeviceProperties/Add/PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0):
    class-code = <FFFFFFFF>, disable-gpu = <01000000>
```

The faked class-code silently defeats NVRM's IOPCIClassMatch: every kext installed correctly, config otherwise perfect, and **NVRM never probes** — no error anywhere. Deleting the entry was the entire fix. The README does say "no DeviceProperties are needed", but 1401 neither detects nor remediates this; laptop users coming from dortania guides will have exactly this leftover. (May be related to #38 (TU117 desktop) and #45 (TU116): "driver installed, never loads".)

## 1401.app gaps on Optimus laptops

1. The app enumerates only the Intel iGPU ("This Mac: Graphics = Intel UHD Graphics 630") and then refuses install: "No supported NVIDIA card was found. The driver needs a GeForce RTX 20 series or newer." → GTX 16 (and any Optimus) users can never install through the GUI. Suggest enumerating all display-class PCI devices, and gating on the GSP device table (which includes GTX 16), not RTX 20+.
2. No check for `disable-gpu`/faked `class-code` DeviceProperties on the dGPU path (above).
3. Because the GUI couldn't be used, the install lacks `/Library/NullMoth/state` and `com.nullmoth.recover`: the picker's "1401: Remove NVIDIA driver" still applies `-nvoff`, but the automatic config restore never runs. Manual `uninstall.sh` + config restore works.

## Note for dual-booters

`UEFI.Quirks.ResizeGpuBars=13` is applied hardware-wide by OpenCore: booting **Windows from the OC picker** blue-screens this machine; booting Windows via the firmware's own boot menu (F12) is fine. Might deserve a line in the README "Dual boot" section.

## HDMI audio: unclaimed HDAU — is RM-side enablement planned?

The GPU's HD-audio function `10de:10fa` (`HDAU@0,1`, class `0x040300`, powered, `hda-gfx`/`layout-id` injected by WhateverGreen) gets **no driver at all** on macOS 15.7:

- `AppleGFXHDA.kext` (400.1) has IOPCIMatch personalities for AMD and Intel only.
- `AppleHDAController`'s "BuiltInHDA" personality class-matches `0x04010000&0xFFFD0000` (covers `0x040300`) yet never attaches to the NVIDIA HDAU — rejected at probe time.
- A kernel class `NVHDAEnabler` is registered (0 instances) but its string exists in no kext on disk — dormant code?

Audio through the same monitor/cable works in Windows and Ubuntu. Question: does NVRM currently enable the HDA function/codec (infoframes etc.), and is a macOS audio path (NVHDAEnabler or guidance for an injector to `AppleHDAController`/`AppleGFXHDAEGController`) on the roadmap?

Happy to test builds and provide full logs (`1401` crash-report flow not used — GUI couldn't see the card). Full install log available on request.
