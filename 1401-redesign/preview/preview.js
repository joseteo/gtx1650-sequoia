// PREVIEW ONLY — not shipped. Simulates app.js state so the redesigned theme
// can be viewed in a plain browser (the real app.js talks to the native shell).
const rows = [
  ["macOS", "15.7.4"],
  ["Processor", "x86_64"],
  ["Graphics", "Intel UHD Graphics 630"],
  ["OpenCore", "REL-106-2025-11-03"],
  ["NullMoth driver", "1.10.0 — installed"],
];
document.getElementById("mac").innerHTML =
  rows.map((r) => `<tr><th>${r[0]}</th><td>${r[1]}</td></tr>`).join("");

document.getElementById("verdict").innerHTML =
  '<p><span class="good">●</span> <b>Driver installed and running.</b> GTX 1650 driving GS27QA at 2560×1440 · 144 Hz · Metal 3.</p>';

document.getElementById("steps").hidden = false;
document.getElementById("s1").classList.add("done");
document.getElementById("s1t").textContent =
  "Downloaded and verified against the published SHA-256.";
document.getElementById("s2").classList.add("done");
document.getElementById("s3").classList.add("now");
document.getElementById("dry").disabled = false;
document.getElementById("go").disabled = false;

document.getElementById("logwrap").hidden = false;
document.getElementById("log").innerHTML =
  '<span class="c">NVRM-xnu:</span> probe 10de:1f99 class 03.00 subsystem 17aa:3fa2\n' +
  '<span class="c">NVRM-xnu:</span> rm_is_supported_pci_device OK; RM names 10de:1f99 "NVIDIA GeForce GTX 1650"\n' +
  '<span class="c">NVRM-xnu:</span> armed on 01:00.0 10de:1f99\n' +
  '<span class="k">✓ driver armed — display at 2560×1440 @ 144 Hz</span>';

document.getElementById("ver").textContent = "1.10.0";

document.querySelectorAll(".tab").forEach((b) =>
  b.addEventListener("click", () => {
    document.querySelectorAll(".tab").forEach((x) => x.classList.toggle("on", x === b));
    ["drv", "usb", "crash"].forEach(
      (t) => (document.getElementById("t-" + t).hidden = b.dataset.t !== t)
    );
  })
);
