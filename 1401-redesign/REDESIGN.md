# 1401 — Apple-native UI redesign (proposal for upstream)

A drop-in reskin of the [NullMoth 1401 app](https://github.com/nullmoth/nvidia-macos-driver) that takes it
from the retro BBS theme to a modern macOS-native look: SF typography, System-Settings-style grouped cards,
rounded-square app icon treatment, segmented-control tabs, step check-circles, light + dark mode.

**100% CSS-only.** `index.html` and `app.js` are untouched — the entire redesign is `guide.css` + `app.css`.
That makes the upstream diff two files and the review trivial.

![1401 with the Apple-native reskin — dark mode](screenshot.png)

## Files

| File | What |
|---|---|
| `guide.css` | base theme (colors via CSS custom properties, header, segmented tabs, buttons, "This Mac" grouped list, footer) |
| `app.css` | app surfaces (cards, install steps with state circles, activity log, USB port map, status colors) |
| `preview/` | view it without installing anything: open `preview/index.html` in a browser |
| `mockup-before-after.html` | the approved design mockup (current theme vs this one, side by side) |

## Preview

```bash
open preview/index.html
```

`preview/index.html` is the real markup with two differences, clearly marked in the file:
the CSP meta tag is removed and `app.js` is swapped for `preview.js`, which simulates live state
(populated "This Mac" list, steps done/current, log lines, working tabs). `preview.js` is **not** part
of the proposal — the shipped change is the two CSS files only.

## Contract preserved (verified against app.js)

- step states: `.todo li.done` / `.todo li.now` (restyled as ✓ / accent circles)
- tabs: `.tab.on`
- log coloring: `#log .c / .k / .x / .s`
- status text: `.good / .bad / .warn`
- USB map: `.ports td.on / .off`
- `[hidden]` behavior, `disabled` buttons, all element ids — unchanged

## Design decisions worth a reviewer's attention

- The scrolling `***` ticker text is still in the markup; CSS hides it and shows a one-line privacy strip
  (`Everything stays on this Mac · Every change is backed up first · …`). If upstream prefers, the same
  sentence can replace the ticker text in `index.html` instead.
- Primary actions (`Install the driver`, `Update driver`, `Write the map`, `Restart now`) get the accent
  color by id; all other buttons stay neutral macOS push-buttons.
- The moth icon is unchanged, just rendered in a rounded-square (22.5% radius) with a subtle inset stroke.
- Dark mode via `prefers-color-scheme` tokens; `prefers-reduced-motion` kills all animation; print stylesheet
  kept (plain black on white).
- The footer "visitor 000001" joke survives, quieter.
- One optional markup addition, shown in the preview only: a footer credit line for the redesign author
  ("Apple-native UI refresh contributed by @joseteo"). The shipped CSS patch needs no markup changes.

Made by [@joseteo](https://github.com/joseteo) as a contribution proposal — MIT, take it.
