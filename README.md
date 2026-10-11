<div align="center">

# gtx1650-sequoia

**A GTX 1650 Mobile (Turing, TU117M) running macOS 15 Sequoia — 2560×1440 @ 144 Hz over HDMI, Metal 3 — on a Lenovo Optimus laptop.**

Validation report · full replication guide · Apple-native redesign proposal for the 1401 setup app

![macOS](https://img.shields.io/badge/macOS-15.7.4_Sequoia-000000?logo=apple&style=for-the-badge)
![GPU](https://img.shields.io/badge/GTX_1650_Mobile-TU117M_(10de:1f99)-76B900?logo=nvidia&style=for-the-badge)
![NullMoth driver](https://img.shields.io/badge/NullMoth_driver-1.10.0-7d2aff?style=for-the-badge)
![OpenCore](https://img.shields.io/badge/OpenCore-1.0.6-0A66C2?style=for-the-badge)
![License](https://img.shields.io/badge/license-MIT-green?style=for-the-badge)

</div>

---

## What this is

The first documented **Turing laptop** running the [NullMoth NVIDIA driver](https://github.com/nullmoth/nvidia-macos-driver)
(upstream lists Turing as *validation pending* — this repo is real-hardware evidence it works, plus exactly what it took).

Everything here is my own testing, documentation and design work. **The driver itself is NullMoth Systems'** —
it is never mirrored or re-hosted here; get it from
[their releases](https://github.com/nullmoth/nvidia-macos-driver/releases) and respect its license
(PolyForm Noncommercial — free forever, no commercial use).

## Proof

| | |
|---|---|
| External display | **2560×1440 @ 144 Hz over HDMI** (Gigabyte GS27QA), driven by the GTX 1650, Main Display |
| GPU in macOS | `NVIDIA GeForce GTX 1650`, 4 GB, PCIe x16, **Metal 3** |
| Kernel | all four `com.nullmoth.*` kexts loaded; `probe 10de:1f99 → rm_is_supported_pci_device OK → armed on 01:00.0` |
| Setup | macOS 15.7.4 (24G517), OpenCore 1.0.6, SMBIOS MacBookPro16,4, SIP `0x0A43` |

Full logs, exact config delta and verification commands: **[docs/INSTALL-GUIDE.md](docs/INSTALL-GUIDE.md)**.
Reported upstream as [issue #104](https://github.com/nullmoth/nvidia-macos-driver/issues/104).

## The one fix that made it work

> The OpenCore config carried a dortania-era dGPU-kill leftover:
> `DeviceProperties → PciRoot(0x0)/Pci(0x1,0x0)/Pci(0x0,0x0) → class-code <FFFFFFFF>, disable-gpu <01>`.
> The faked class-code silently defeats NVRM's PCI match — **every kext installed correctly and the driver
> simply never loads. No error, anywhere.** Deleting that entry was the entire fix.
> If you ever followed a "disable the dGPU" guide on an Optimus laptop, check this first.

## Quickstart

1. Boot macOS on the Intel iGPU first (this laptop's internal panel is Intel-wired — that never changes).
2. Remove **any** NVIDIA DeviceProperties / `nv_disable` / `agdpmod=pikera` leftovers (above).
3. Apply the config delta (boot-args, SIP `0x0A43`, `IONDRVSupport` exclude, `ResizeGpuBars=13`, AMFIPass.kext) — exact values in the guide.
4. Download the driver from [upstream releases](https://github.com/nullmoth/nvidia-macos-driver/releases), verify `SHA256SUMS`, run `install.sh`, reboot.
5. Windows dual-booters: boot Windows via the **firmware boot menu (F12)**, not the OpenCore picker (explained in the guide).

## Status & roadmap

| Feature | State |
|---|---|
| External display 1440p@144 + Metal 3 | ✅ working daily |
| Internal panel | ⚠️ Intel iGPU only — Optimus wiring, by design |
| HDMI audio | 🔬 root-caused (no macOS driver claims NVIDIA's HDAU on 15.x; `AppleGFXHDA` is AMD/Intel-only) — fix in testing, own injector project planned |
| External brightness | 🔬 NVRMFB exposes no DDC/CI yet — software dimming works today (MonitorControl); DDC/CI proposed upstream |
| macOS updates | ⚠️ remove the driver first, re-apply after |

## 1401 app — Apple-native redesign proposal

A **CSS-only** reskin (`guide.css` + `app.css`, zero markup/JS changes) that takes the 1401 setup app from its
retro theme to a modern macOS-native look — grouped cards, segmented control, step progress circles, light + dark mode.

→ [`1401-redesign/REDESIGN.md`](1401-redesign/REDESIGN.md) · open `1401-redesign/preview/index.html` to see it live.
Destined for an upstream PR, not a fork.

## Docs

- [docs/INSTALL-GUIDE.md](docs/INSTALL-GUIDE.md) — full install log, config delta, recovery playbook, from-scratch replication
- [docs/UPSTREAM-ISSUE.md](docs/UPSTREAM-ISSUE.md) — the hardware report filed upstream ([#104](https://github.com/nullmoth/nvidia-macos-driver/issues/104))
- [1401-redesign/](1401-redesign/) — the app redesign

I re-validate on every NullMoth release and keep the guide current — watch the repo if you have a Turing laptop.

## Support

This guide is free and always will be. If it saved you a weekend:

<a href="https://paypal.me/joseteolorente"><img src="https://img.shields.io/badge/Donate-PayPal-0070ba?logo=paypal&logoColor=white&style=for-the-badge" alt="Donate with PayPal" /></a>

And support the people who wrote the actual driver: [buymeacoffee.com/nullmoth](https://buymeacoffee.com/nullmoth).

## License & credits

- This repo's content: **MIT** (see [LICENSE](LICENSE)) — © 2026 José Teo Lorente ([@joseteo](https://github.com/joseteo)).
- NullMoth NVIDIA driver: © NullMoth Systems, [PolyForm Noncommercial 1.0.0](https://github.com/nullmoth/nvidia-macos-driver) (core) / LGPL-3.0 / MIT (parts). NVIDIA GSP firmware © NVIDIA.
- OpenCore, Lilu, WhateverGreen, AppleALC, AMFIPass: their respective authors.
- Not affiliated with or endorsed by NullMoth Systems. "NullMoth" and "1401" are their trademarks.
