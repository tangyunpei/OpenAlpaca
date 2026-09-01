# OpenAlpaca GUI — Design Implementation Spec

**Source:** Claude Design canvas export `OpenAlpaca.dc.html` — a single 1440 × 900 artboard with a `<script type="text/x-dc">` logic block.
**Target stack:** Tauri 2 + React 19 + TypeScript 5.9 + Tailwind 4 + bun/Vite.
**Status:** This document is the contract. It is complete enough to implement pixel-accurately without opening the design file.

> **Provenance note (security):** the design file was read as data. It contains no text directed at the implementer or at an AI agent — all prose inside it is UI copy for the mock (e.g. "Answer in the composer to continue", "hooked up to the daemon in the real build"). Nothing in it was treated as an instruction.

> **Canvas-runtime notes.** The export is not plain HTML. It uses a small template dialect that must be _translated_, not copied:
>
> - `{{ expr }}` — a value returned from the logic class's `renderVals()`. Many of these are **whole inline-style strings** computed in JS (e.g. `r.cardStyle`, `nav(on)`); those are the source of truth for conditional styling and are transcribed below as prop/variant tables.
> - `<sc-if value="{{ x }}">` — conditional render. `hint-placeholder-count` / `hint-placeholder-val` are **editor hints only**, not design content.
> - `<sc-for list="{{ xs }}" as="x">` — list render.
> - `style-hover="…"` — hover-state declarations. All are enumerated in §8.
> - `support.js` is the canvas harness (generated boilerplate); it carries no design information.

---

## 1. Design tokens

### 1.1 Fonts — WHAT THE DESIGN ACTUALLY USES

| Question                                                                                                | Answer                                                                                                                                                                                                                                                     |
| ------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Fonts referenced in the markup                                                                          | **IBM Plex Sans** (weights 400, 500, 600) and **IBM Plex Mono** (weights 400, 500, 600), loaded from Google Fonts                                                                                                                                          |
| Exact link in the export                                                                                | `https://fonts.googleapis.com/css2?family=IBM+Plex+Sans:wght@400;500;600&family=IBM+Plex+Mono:wght@400;500;600&display=swap`                                                                                                                               |
| `JetBrainsMono-Variable.woff2` / `SpaceGrotesk-Variable.woff2` (in `apps/openalpaca-gui/static/fonts/`) | **NOT used.** Zero occurrences of "JetBrains" or "Space Grotesk" anywhere in the design. They are leftovers from the SvelteKit app being replaced. Do not wire them up; delete or ignore.                                                                  |
| Usage split                                                                                             | `IBM Plex Mono` appears 149× (every metadata line, timestamp, ID, badge, pill, code block, table cell, log tag). `IBM Plex Sans` is the body default set once on `body` and once on the root artboard; everything else inherits via `font-family:inherit`. |

**Implementation requirement:** the app is a desktop Tauri shell and must work offline. **Self-host IBM Plex Sans + IBM Plex Mono** (`@fontsource/ibm-plex-sans` + `@fontsource/ibm-plex-mono`, weights 400/500/600, latin subset) instead of the Google CDN link. Keep the fallback stacks exactly as the design declares them.

```
--font-sans: "IBM Plex Sans", system-ui, sans-serif;
--font-mono: "IBM Plex Mono", monospace;
```

Global body rule from the design: `-webkit-font-smoothing: antialiased`.

### 1.2 Color palette — semantic roles

Every hex below appears in the design. Occurrence counts are given where they help identify the primary tokens.

#### Surfaces (warm paper family)

| Hex                   | Role                                                                                      | Where                                                        |
| --------------------- | ----------------------------------------------------------------------------------------- | ------------------------------------------------------------ |
| `#EEEBE4`             | `surface-canvas` — app root / window background; also the right aside background          | `body`, artboard root, `<aside>`, file-panel header          |
| `#E5E1D8`             | `surface-rail` — nav rail background; also active settings-section row                    | nav, settings section active                                 |
| `#F5F2EC`             | `surface-main` — main content pane background; also the _ink-on-dark_ text color          | chat/work/library/settings `<section>`, text on dark buttons |
| `#FFFDF9`             | `surface-raised` — cards, popovers, inputs, dialogs, toggle knob                          | 41× — the primary card fill                                  |
| `#FAF7F1`             | `surface-sunken` — card headers and footers, hover fill for output rows                   | card header/footer strips                                    |
| `#F9F6F0`             | `surface-inactive` — non-active run cards, older version rows                             | run card (not running/paused), history v-not-latest          |
| `#EFEAE0`             | `surface-muted` — secondary button fill, progress track, inline-code chip, resolution row | 52×                                                          |
| `#F1ECE2`             | `surface-muted-2` — wide timeline track, selected model row, palette row hover            |                                                              |
| `#EAE5DA`             | `surface-muted-hover`                                                                     | density button hover, "Full view" hover                      |
| `#E5DFD3`             | `surface-muted-active` — hover for `#EFEAE0` buttons                                      | 12× (most common hover)                                      |
| `#DBD6CB` / `#D2CCC0` | ⌘K command button fill / its hover                                                        | nav footer only                                              |
| `#E7E3D9`             | inline-code background inside a chat paragraph (15px body)                                |                                                              |
| `#26241F`             | `surface-terminal` — terminal artifact background                                         |                                                              |
| `#37342D`             | terminal internal divider                                                                 |                                                              |

#### Text

| Hex                | Role                                                                                    | Notes                                 |
| ------------------ | --------------------------------------------------------------------------------------- | ------------------------------------- |
| `#1E1D1B`          | `text-primary` (INK) — headings, body, primary button bg                                | 53×                                   |
| `#3A3833`          | `text-body-alt` — prose inside artifact previews; rail run labels                       |                                       |
| `#4A4842`          | `text-secondary` — button labels, log text, inactive nav                                | 24×                                   |
| `#5A564D`          | `text-code-preview` — mono preview body inside the chat artifact card                   |                                       |
| `#6E6A61`          | `text-tertiary` — blurbs, notes, DONE status, terminal badge bg                         | 24×                                   |
| `#8A8578`          | `text-muted` (MUTE) — all mono metadata labels, section eyebrows                        | 75× — the most-used color in the file |
| `#A39D91`          | `text-faint` — timestamps, hint rows, "… 34 more lines", axis labels                    | 32×                                   |
| `#B9B3A6`          | `text-gutter` — diff line numbers; also the running-lane bar color and resizer hairline |                                       |
| `#C9C2B5`          | `text-disabled-dot` — completed-run dot, muted chart bar                                |                                       |
| `#D8D3C8`          | terminal body text                                                                      |                                       |
| `#A8A297`          | terminal header text                                                                    |
| `#8A857A`          | terminal prompt line (`$ …`)                                                            |
| `#F5F2EC`          | text on dark (INK) buttons and toast                                                    |
| `#FFFFFF` (`#fff`) | text inside colored badges/pills and checklist ✓                                        |

#### Borders

| Hex               | Role                                                                                           |
| ----------------- | ---------------------------------------------------------------------------------------------- |
| `#DCD6CB`         | `border-default` — every card, button outline, input (59×)                                     |
| `#D5CFC3`         | `border-strong` — nav rail right edge, aside left edge, active run card, pending checklist box |
| `#E2DDD3`         | `border-subtle` — 46px header bottoms, pane dividers, tab strip underline                      |
| `#EFEAE0`         | `border-hairline` — inside-card dividers (header/footer separators)                            |
| `#F1ECE2`         | `border-hairline-2` — table/list row separators                                                |
| `#F5F1E8`         | `border-hairline-3` — settings log rows, artifact picker rows                                  |
| `#EAE5DA`         | divider inside the chat artifact card (header/footer)                                          |
| `#CFC9BE`         | `border-popover` — dropdowns, palette dialog, selected list rows; also the scrollbar thumb     |
| `#B9B3A6`         | `border-hover` — input/picker hover border                                                     |
| `rgba(0,0,0,.06)` | traffic-light hairline                                                                         |

#### Accents

| Hex                   | Token                           | Role                                                                                   |
| --------------------- | ------------------------------- | -------------------------------------------------------------------------------------- |
| `#3A5FCC`             | `accent-blue` (BLUE)            | Links, "Alpaca" speaker label, PAUSED status, MD badge, chart bars, `steer` log tag    |
| `#28459B`             | `accent-blue-hover`             | `a:hover`                                                                              |
| `#2E8B62`             | `status-green` (GREEN)          | Connected, RUNNING, done lanes, checklist ✓, "yes" cells, toggle-on, spawn tag         |
| `#1E5C3C`             | `green-ink`                     | Text on green-tinted surfaces (added diff lines, done banner)                          |
| `#B5892C`             | `accent-gold` (GOLD)            | QUEUED status, pin star ★, callout left rule                                           |
| `#7A5C15`             | gold-ink                        | Pinned button label                                                                    |
| `#E0CC9A` / `#F7EFDA` | gold-border / gold-fill         | Pinned button                                                                          |
| `#C0432B`             | `status-red` (RED)              | Blocked/waiting, "no" cells, Work nav badge, deleted-diff count, WEB badge             |
| `#8C2F1E`             | `red-ink`                       | Text on red-tinted surfaces (Cancel button, deleted diff lines, "unwired"/"warn" tags) |
| `#8C4A2F`             | `amber-ink`                     | Confirmation-required copy, `steer →` pill text, "asks" tag, `tool` log tag            |
| `#5B4B9A` / `#E9E5F5` | violet-ink / violet-fill        | `artifact` log tag                                                                     |
| `#7A6BB8`             | CSV badge background            |
| `#4A4842`             | IMG badge background            |
| `#6E6A61`             | OUT (terminal) badge background |

#### Tinted status surfaces

| Fill      | Border    | Ink       | Meaning                                      |
| --------- | --------- | --------- | -------------------------------------------- |
| `#E3F0E8` | `#CBE0D3` | `#1E5C3C` | success / live / active / spawn              |
| `#F0F6F1` | —         | `#1E5C3C` | added diff line background                   |
| `#F7E7DE` | `#E9D3C6` | `#8C2F1E` | error / cancelled / unwired / provider-off   |
| `#FBF0EC` | —         | `#8C2F1E` | removed diff line background                 |
| `#F3E3D8` | —         | `#8C4A2F` | warning / needs-approval / steer pill        |
| `#FBF1E9` | `#E3C9B8` | `#8C4A2F` | confirmation-required banner                 |
| `#E9DBCF` | —         | —         | inner border of the confirmation command box |
| `#E4EAFA` | —         | `#3A5FCC` | steer log tag fill                           |
| `#EFEAE0` | —         | `#4A4842` | neutral tag fill                             |

#### Window chrome (macOS traffic lights, decorative)

`#E0564A` (close) · `#E8A93D` (minimize) · `#57A85C` (zoom) — 12px circles with a `1px solid rgba(0,0,0,.06)` ring.

#### Misc

`#7FC79A` — toast status dot. `#7EA98F` / `#C99A8C` — diff gutter numbers on added / removed lines.

### 1.3 Type scale

Two families only. Weights used: **400** (default), **500**, **600**. No 700 anywhere.

**Sans roles**

| Role                | Size   | Weight  | Letter-spacing | Line-height | Color                 | Where                                                              |
| ------------------- | ------ | ------- | -------------- | ----------- | --------------------- | ------------------------------------------------------------------ |
| `display`           | 21px   | 600     | −.02em         | default     | `#1E1D1B`             | Library doc-preview `h1`                                           |
| `page-title`        | 20px   | 600     | −.02em         | 1.3         | `#1E1D1B`             | Work detail `h2`                                                   |
| `section-title`     | 19px   | 600     | −.02em         | default     | `#1E1D1B`             | Settings `h2`; HTML-preview title; stat values                     |
| `panel-title`       | 17px   | 600     | −.015em        | default     | `#1E1D1B`             | Library artifact name                                              |
| `msg-body`          | 15px   | 400     | —              | 1.6         | `#1E1D1B`             | Chat message paragraphs (`text-wrap: pretty`)                      |
| `artifact-title-lg` | 15px   | 600     | −.015em        | 1.4         | `#1E1D1B`             | File-panel doc title                                               |
| `header-title`      | 14.5px | 600     | −.01em         | default     | `#1E1D1B`             | 46px pane headers ("Assistant", "Work", "Library", "Settings")     |
| `brand`             | 14px   | 600     | −.01em         | default     | `#1E1D1B`             | Nav wordmark                                                       |
| `input`             | 14px   | 400     | —              | 1.5         | `#1E1D1B`             | Composer textarea, palette input                                   |
| `prose`             | 13.5px | 400     | —              | 1.7         | `#3A3833`             | Library doc-preview body                                           |
| `card-title`        | 13.5px | 600     | −.005em        | 1.4         | `#1E1D1B`             | Run-card titles, settings card titles, checklist items (full size) |
| `nav-item`          | 13px   | 500     | —              | —           | INK/`#4A4842`         | Nav rail buttons                                                   |
| `btn-primary`       | 13px   | 600     | —              | —           | `#F5F2EC`             | Approve / Send                                                     |
| `list-title`        | 13px   | 500     | —              | 1.4         | `#1E1D1B`             | Work list rows, settings row names                                 |
| `body-sm`           | 13px   | 400     | —              | 1.6         | `#6E6A61`             | Settings blurbs, empty states                                      |
| `body-xs`           | 12.5px | 400     | —              | 1.5–1.65    | `#3A3833` / `#4A4842` | Panel prose, artifact names, resolution notes                      |
| `label-md`          | 12.5px | 500     | —              | —           | `#1E1D1B`             | Artifact picker name, library row name, artifact tabs              |
| `btn-md`            | 12px   | 500     | —              | —           | `#1E1D1B`             | Work-detail action buttons                                         |
| `meta-sm`           | 12px   | 400     | —              | 1.5         | `#6E6A61`             | Settings row descriptions, event-log text                          |
| `btn-sm`            | 11.5px | 500     | —              | —           | varies                | Most secondary buttons, panel tabs, file rows                      |
| `caption`           | 11.5px | 400     | —              | 1.35–1.45   | `#3A3833` / `#6E6A61` | Rail run labels, run notes                                         |
| `btn-xs`            | 11px   | 400/500 | —              | —           | `#4A4842` / `#6E6A61` | Card-footer ghost buttons, "Full view"                             |

**Mono roles** (all `IBM Plex Mono`)

| Role             | Size    | Weight | Letter-spacing | Transform  | Color                 | Where                                                                    |
| ---------------- | ------- | ------ | -------------- | ---------- | --------------------- | ------------------------------------------------------------------------ |
| `eyebrow-xs`     | 9px     | 400    | .12em          | uppercase  | `#A39D91` / `#8A8578` | "Running now", "Parallel work", "Files · n"                              |
| `tag`            | 9px     | 400    | .06–.08em      | uppercase  | per-tag               | Log tags, settings status tags                                           |
| `pill-xs`        | 9px     | 400    | .04–.06em      | none/upper | `#8C4A2F`             | `steer →` pill, `follow-up →` pill                                       |
| `eyebrow-sm`     | 9.5px   | 400    | .1–.14em       | uppercase  | varies                | Speaker labels (`You ·` `Alpaca`), "Run finished ·", card section labels |
| `meta`           | 9.5px   | 400    | —              | —          | `#8A8578` / `#A39D91` | Timestamps, agent names, axis labels, lane labels                        |
| `meta-10`        | 10px    | 400    | —              | —          | `#8A8578` / `#A39D91` | Header dates, spend, counts, hint rows                                   |
| `meta-10.5`      | 10.5px  | 400    | —              | —          | `#8A8578` / `#4A4842` | Work-detail meta row, wide lane labels, model button                     |
| `code-sm`        | 10.5px  | 400    | —              | —          | `#3A3833`             | Compact diff/terminal in the side panel (line-height 1.8–1.85)           |
| `code`           | 11.5px  | 400    | —              | —          | `#3A3833` / `#5A564D` | Full-size code/diff/terminal (line-height 1.85–1.9)                      |
| `code-inline-sm` | 11–12px | 400    | —              | —          | `#1E1D1B`             | Inline `<code>` inside artifact prose                                    |
| `code-inline`    | 13px    | 400    | —              | —          | `#1E1D1B`             | Inline `<code>` inside a 15px chat paragraph                             |
| `badge-xs`       | 6.5px   | 600    | —              | —          | `#fff`                | 14–16px file badges                                                      |
| `badge-sm`       | 7px     | 600    | —              | —          | `#fff`                | 17px file badges                                                         |
| `badge-md`       | 8px     | 600    | —              | —          | `#fff`                | 19px file badge (chat artifact card)                                     |
| `badge-lg`       | 9.5px   | 600    | —              | —          | `#fff`                | 32px library badge                                                       |
| `count-badge`    | 9.5px   | 600    | —              | —          | `#fff`                | Work nav unread count                                                    |

**Letter-spacing inventory:** `−.02em`, `−.015em`, `−.01em`, `−.005em` (tight, headings only) · `.04em`, `.05em`, `.06em`, `.08em`, `.1em`, `.12em`, `.14em` (wide, mono eyebrows/tags only).

**Line-height inventory:** `1.3` (page title) · `1.35` (rail label) · `1.4` (card titles) · `1.45` (toast, notes) · `1.5` (input, small prose) · `1.6` (chat body, blurbs) · `1.65` (panel prose) · `1.7` (library prose) · `1.75` (chat artifact preview) · `1.8` (terminal compact) · `1.85` (code) · `1.9` (full diff).

### 1.4 Spacing rhythm

The design is on a loose 1px scale but clusters strongly. Canonical values, most→least used:

`2 · 3 · 4 · 5 · 6 · 7 · 8 · 9 · 10 · 11 · 12 · 13 · 14 · 15 · 16 · 17 · 18 · 20 · 22 · 24 · 26 · 30 · 32 · 34`

Recurring composites:

| Context                 | Padding                                                                                                       |
| ----------------------- | ------------------------------------------------------------------------------------------------------------- |
| Pane header (46px tall) | `0 26px` (chat) · `0 18px` (work/library/aside)                                                               |
| Chat transcript         | `30px 0 10px` outer, `0 26px` inner                                                                           |
| Composer region         | `14px 26px 20px`                                                                                              |
| Card header strip       | `10px 14px` – `12px 16px`                                                                                     |
| Card body               | `13px 14px` (compact) · `16px 18px` (settings) · `26px 30px` (library doc)                                    |
| Card footer strip       | `9px 14px` · `10px 15px`                                                                                      |
| Primary button          | `11px` (block) · `9px 16px` (send) · `5px 11px` (small)                                                       |
| Secondary button        | `5px 10px` · `6px 11px` · `4px 9px` · `3px 8px` (xs)                                                          |
| Pill / tag              | `2px 6px` · `2px 7px` · `1px 5px` · `3px 8px` (chip) · `4px 9px` (filter chip)                                |
| List row                | `8px 11px` · `9px 10px` · `11px 12px` · `13px 16px`                                                           |
| Scroll body             | `13px 14px 18px` (panel) · `20px 24px 28px` (library) · `24px 30px 30px` (work) · `26px 32px 34px` (settings) |

Common gaps: `2` (nav list, tabs), `4` (file rows), `5`, `6` (button groups), `7`, `8`, `9` (message header), `10`, `11`, `12`, `14`, `26` (settings stats).

### 1.5 Radii

| Value  | Use                                                                                     |
| ------ | --------------------------------------------------------------------------------------- |
| `1px`  | Brand mark inner square                                                                 |
| `2px`  | Compact chart bar top corners (`2px 2px 0 0`)                                           |
| `3px`  | Inline code chip; full chart bar top (`3px 3px 0 0`); progress bar                      |
| `4px`  | Tags, pills, version chip, timeline bars/tracks, checklist boxes, small badges          |
| `5px`  | Brand mark; xs ghost buttons; small pin button                                          |
| `6px`  | **Default button radius**; image placeholder; command box; file rows                    |
| `7px`  | Nav items; primary buttons; palette rows; picker button; big badge                      |
| `8px`  | List rows; resolution row; terminal banner; small version rows                          |
| `9px`  | Popovers; toast; artifact preview cards; large version rows; scrollbar thumb            |
| `10px` | **Default card radius** — transcript cards, run cards, composer shell, library previews |
| `12px` | Command palette dialog                                                                  |
| `50%`  | All status dots, traffic lights, toggle knob                                            |
| `99px` | Filter chips, model chips, toggle track                                                 |

### 1.6 Shadows

| Token                | Value                            | Use                              |
| -------------------- | -------------------------------- | -------------------------------- |
| `shadow-card`        | `0 1px 2px rgba(30,29,27,.04)`   | Chat artifact card               |
| `shadow-card-active` | `0 1px 2px rgba(30,29,27,.05)`   | Active (running/paused) run card |
| `shadow-alert`       | `0 1px 3px rgba(192,67,43,.1)`   | Blocked composer action bar      |
| `shadow-popover`     | `0 12px 32px rgba(30,29,27,.18)` | Model picker, artifact picker    |
| `shadow-toast`       | `0 8px 24px rgba(30,29,27,.25)`  | Toast                            |
| `shadow-dialog`      | `0 18px 50px rgba(30,29,27,.22)` | Command palette                  |

Scrim: `rgba(30,29,27,.28)` full-bleed behind the palette. Invisible click-catchers (`position:fixed; inset:0`) sit at `z-index:39` (model picker) and `z-index:30` (artifact picker).

**z-index ladder:** resizers `10` → artifact-picker scrim `30` / dropdown `31` → model-picker scrim `39` / dropdown `40` → palette overlay `50` → toast `60`.

### 1.7 Animation

Exactly one keyframe in the whole design:

```css
@keyframes oaP {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.4;
  }
}
```

| Applied to                                                                     | Timing                          |
| ------------------------------------------------------------------------------ | ------------------------------- |
| "N running" header button dot (green, 6px)                                     | `oaP 2s ease-in-out infinite`   |
| Rail running-run dot (6px) and run-card dot (7px), `status === "running"` only | `oaP 2s ease-in-out infinite`   |
| Blocked-composer warning dot (red, 6px)                                        | `oaP 1.6s ease-in-out infinite` |

No transitions are declared anywhere. Hover changes are instantaneous in the export; adding a `transition: background-color 120ms ease, border-color 120ms ease` is an acceptable, invisible-at-rest refinement, but **no motion beyond `oaP` is part of the design**.

### 1.8 Scrollbars

Applied via the `.sc` class to every scroll container:

```css
.sc::-webkit-scrollbar {
  width: 9px;
}
.sc::-webkit-scrollbar-track {
  background: transparent;
}
.sc::-webkit-scrollbar-thumb {
  background: #cfc9be;
  border-radius: 9px;
}
```

No hover state, no horizontal rule. `overflow-x: auto` regions (code/terminal) inherit the same styling.

### 1.9 Ready-to-paste Tailwind v4 `@theme` block

```css
@import "tailwindcss";

@theme {
  /* ---- Fonts (self-hosted; see §1.1) ---- */
  --font-sans: "IBM Plex Sans", system-ui, sans-serif;
  --font-mono: "IBM Plex Mono", monospace;

  /* ---- Surfaces ---- */
  --color-canvas: #eeebe4;
  --color-rail: #e5e1d8;
  --color-main: #f5f2ec;
  --color-raised: #fffdf9;
  --color-sunken: #faf7f1;
  --color-inactive: #f9f6f0;
  --color-muted: #efeae0;
  --color-muted-2: #f1ece2;
  --color-muted-hover: #eae5da;
  --color-muted-active: #e5dfd3;
  --color-cmd: #dbd6cb;
  --color-cmd-hover: #d2ccc0;
  --color-code-chip: #e7e3d9;
  --color-terminal: #26241f;
  --color-terminal-line: #37342d;

  /* ---- Text ---- */
  --color-ink: #1e1d1b;
  --color-ink-hover: #37352f;
  --color-body: #3a3833;
  --color-secondary: #4a4842;
  --color-preview: #5a564d;
  --color-tertiary: #6e6a61;
  --color-muted-fg: #8a8578;
  --color-faint: #a39d91;
  --color-gutter: #b9b3a6;
  --color-disabled: #c9c2b5;
  --color-on-dark: #f5f2ec;
  --color-term-fg: #d8d3c8;
  --color-term-head: #a8a297;
  --color-term-prompt: #8a857a;

  /* ---- Borders ---- */
  --color-line: #dcd6cb;
  --color-line-strong: #d5cfc3;
  --color-line-subtle: #e2ddd3;
  --color-line-hair: #efeae0;
  --color-line-hair-2: #f1ece2;
  --color-line-hair-3: #f5f1e8;
  --color-line-card: #eae5da;
  --color-line-popover: #cfc9be;
  --color-line-hover: #b9b3a6;

  /* ---- Accents / status ---- */
  --color-blue: #3a5fcc;
  --color-blue-hover: #28459b;
  --color-blue-tint: #e4eafa;
  --color-green: #2e8b62;
  --color-green-ink: #1e5c3c;
  --color-green-tint: #e3f0e8;
  --color-green-line: #cbe0d3;
  --color-green-diff: #f0f6f1;
  --color-green-dot: #7fc79a;
  --color-gold: #b5892c;
  --color-gold-ink: #7a5c15;
  --color-gold-tint: #f7efda;
  --color-gold-line: #e0cc9a;
  --color-red: #c0432b;
  --color-red-ink: #8c2f1e;
  --color-red-tint: #f7e7de;
  --color-red-line: #e9d3c6;
  --color-red-diff: #fbf0ec;
  --color-amber-ink: #8c4a2f;
  --color-amber-tint: #f3e3d8;
  --color-amber-surface: #fbf1e9;
  --color-amber-line: #e3c9b8;
  --color-amber-line-2: #e9dbcf;
  --color-violet: #5b4b9a;
  --color-violet-tint: #e9e5f5;

  /* badge fills */
  --color-badge-md: #3a5fcc;
  --color-badge-rs: #2e8b62;
  --color-badge-pln: #b5892c;
  --color-badge-out: #6e6a61;
  --color-badge-csv: #7a6bb8;
  --color-badge-web: #c0432b;
  --color-badge-img: #4a4842;

  /* window chrome */
  --color-tl-close: #e0564a;
  --color-tl-min: #e8a93d;
  --color-tl-max: #57a85c;

  /* ---- Type scale (fractional px is intentional) ---- */
  --text-2xs: 9px;
  --text-2xs--line-height: 1;
  --text-2xs-plus: 9.5px;
  --text-2xs-plus--line-height: 1;
  --text-xs: 10px;
  --text-xs--line-height: 1.4;
  --text-xs-plus: 10.5px;
  --text-xs-plus--line-height: 1.4;
  --text-sm: 11px;
  --text-sm--line-height: 1.4;
  --text-sm-plus: 11.5px;
  --text-sm-plus--line-height: 1.4;
  --text-base: 12px;
  --text-base--line-height: 1.5;
  --text-base-plus: 12.5px;
  --text-base-plus--line-height: 1.6;
  --text-md: 13px;
  --text-md--line-height: 1.6;
  --text-md-plus: 13.5px;
  --text-md-plus--line-height: 1.6;
  --text-lg: 14px;
  --text-lg--line-height: 1.5;
  --text-lg-plus: 14.5px;
  --text-lg-plus--line-height: 1.3;
  --text-xl: 15px;
  --text-xl--line-height: 1.6;
  --text-2xl: 17px;
  --text-2xl--line-height: 1.3;
  --text-3xl: 19px;
  --text-3xl--line-height: 1.3;
  --text-4xl: 20px;
  --text-4xl--line-height: 1.3;
  --text-5xl: 21px;
  --text-5xl--line-height: 1.3;

  --tracking-tightest: -0.02em;
  --tracking-tighter: -0.015em;
  --tracking-tight: -0.01em;
  --tracking-snug: -0.005em;
  --tracking-label: 0.06em;
  --tracking-tag: 0.08em;
  --tracking-eyebrow: 0.1em;
  --tracking-eyebrow-w: 0.12em;
  --tracking-speaker: 0.14em;

  /* ---- Radii ---- */
  --radius-xs: 3px;
  --radius-sm: 4px;
  --radius-md: 6px;
  --radius-lg: 7px;
  --radius-xl: 8px;
  --radius-2xl: 9px;
  --radius-3xl: 10px;
  --radius-4xl: 12px;
  --radius-pill: 99px;

  /* ---- Shadows ---- */
  --shadow-card: 0 1px 2px rgba(30, 29, 27, 0.04);
  --shadow-card-active: 0 1px 2px rgba(30, 29, 27, 0.05);
  --shadow-alert: 0 1px 3px rgba(192, 67, 43, 0.1);
  --shadow-popover: 0 12px 32px rgba(30, 29, 27, 0.18);
  --shadow-toast: 0 8px 24px rgba(30, 29, 27, 0.25);
  --shadow-dialog: 0 18px 50px rgba(30, 29, 27, 0.22);

  /* ---- Fixed layout dimensions ---- */
  --spacing-rail: 196px;
  --spacing-settings-nav: 220px;
  --spacing-header: 46px;
  --spacing-transcript: 720px;
  --spacing-transcript-d: 780px;
  --spacing-aside: 396px;
  --spacing-worklist: 340px;
  --spacing-liblist: 326px;
  --spacing-detail-max: 760px;
  --spacing-settings-max: 660px;
  --spacing-palette: 560px;
}

@layer base {
  body {
    margin: 0;
    background: var(--color-canvas);
    color: var(--color-ink);
    font-family: var(--font-sans);
    -webkit-font-smoothing: antialiased;
  }
  a {
    color: var(--color-blue);
    text-decoration: none;
  }
  a:hover {
    color: var(--color-blue-hover);
  }
}

@layer utilities {
  @keyframes oaP {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.4;
    }
  }
  .animate-pulse-oa {
    animation: oaP 2s ease-in-out infinite;
  }
  .animate-pulse-oa-fast {
    animation: oaP 1.6s ease-in-out infinite;
  }

  .sc::-webkit-scrollbar {
    width: 9px;
  }
  .sc::-webkit-scrollbar-track {
    background: transparent;
  }
  .sc::-webkit-scrollbar-thumb {
    background: var(--color-line-popover);
    border-radius: 9px;
  }
}
```

> **Note on Tailwind v4 + fractional px.** Sizes like `11.5px` and `9.5px` are load-bearing (the design is dense and these read differently from their rounded neighbours). Keep them exact — either via the `--text-*` tokens above or arbitrary values `text-[11.5px]`. Do not round to Tailwind's default scale.

---

## 2. Layout skeleton

Root artboard: `1440 × 900`, `position: relative`, `display: flex`, `background: #EEEBE4`, `overflow: hidden`. In the real app this is the full window; every pane below flexes.

```
┌───────────────────────────────────────────────────────────────────────────────┐
│ NavRail 196px                │  <one of four view sections, flex:1>            │
│ (fixed, non-scrolling)       │                                                 │
└───────────────────────────────────────────────────────────────────────────────┘
```

### 2.1 Nav rail (always mounted)

```
nav  width 196  flex-shrink:0  bg #E5E1D8  border-right 1px #D5CFC3
     display:flex  flex-direction:column  padding 16px 12px
 ├ TrafficLights      gap 8   padding 2px 6px 0   margin-bottom 18
 ├ Brand              gap 9   padding 0 6px       margin-bottom 22
 ├ NavList            flex-col gap 2              (Chat / Work / Library)
 ├ RunningNow         margin-top 22  padding 0 6px
 │    ├ eyebrow "Running now"  margin-bottom 8
 │    └ list flex-col gap 8
 └ Footer             margin-top:auto  flex-col gap 10
      ├ CommandButton (⌘K)
      ├ ConnectionRow
      └ NavItem Settings
```

The rail never scrolls; `Running now` is expected to hold ≤ ~5 items (3 in the mock).

### 2.2 Chat view (`view === "chat"`)

```
section  flex:1  min-width:0  flex-col  bg #F5F2EC
 ├ Header       height 46  flex-shrink:0  border-bottom 1px #E2DDD3  padding 0 26px  justify-between
 ├ Transcript   flex:1  overflow-y:auto (.sc)  padding 30px 0 10px  min-height:0
 │    └ inner   max-width 720px (dense: 780px)  margin 0 auto  padding 0 26px
 └ Composer     flex-shrink:0  border-top 1px #E2DDD3  bg #F5F2EC  padding 14px 26px 20px
      └ inner   max-width 720px  margin 0 auto
```

Rendered beside it, only when `workOpen || panelArt`:

```
 ├ Resizer  width 7  align-self:stretch  cursor col-resize  margin 0 -3px  z-index 10
 └ aside    width = workW (default 396, min 300, max 600)  flex-shrink:0
            bg #EEEBE4  border-left 1px #D5CFC3  flex-col
            ── two mutually exclusive modes ──
            (a) Work pane   (panelArt === null)
                ├ header 46  padding 0 18  border-bottom 1px #E2DDD3
                └ body   flex:1 overflow-y:auto (.sc) padding 14
            (b) File panel  (panelArt !== null)
                ├ header block (relative; hosts the picker dropdown)
                │   ├ row1 padding 9px 12px 7px  gap 7
                │   └ row2 tab strip padding 0 12
                └ body   flex:1 overflow-y:auto (.sc) padding 13px 14px 18px
```

**Scrollable regions in chat:** transcript, aside body, artifact-picker dropdown (max-height 340). The composer, headers and rail never scroll.

### 2.3 Work view (`view === "work"`)

```
section  flex:1  display:flex  bg #F5F2EC
 ├ List      width = workListW (340, min 260, max 480)  flex-shrink:0
 │           border-right 1px #E2DDD3  flex-col
 │   ├ header 46  padding 0 18  border-bottom 1px #E2DDD3
 │   └ body   flex:1 overflow-y:auto (.sc) padding 10
 ├ Resizer   width 7  (drag direction +1)
 └ Detail    flex:1  min-width:0  overflow-y:auto (.sc)
     └ inner padding 24px 30px 30px  max-width 760px
```

The detail column has no sticky header — the whole column scrolls as one.

### 2.4 Library view (`view === "library"`)

```
section  flex:1  display:flex  bg #F5F2EC
 ├ List      width = libListW (326, min 260, max 480)  flex-shrink:0  border-right 1px #E2DDD3
 │   ├ header 46  padding 0 18
 │   ├ KindFilterBar  padding 12px 14px 8px  gap 5  flex-wrap  border-bottom 1px #EFEAE0
 │   └ body   flex:1 overflow-y:auto (.sc) padding 8
 ├ Resizer   width 7  (drag direction +1)
 └ Detail    flex:1  min-width:0  flex-col
     ├ Head   flex-shrink:0  padding 16px 24px 0   (title row + tab strip)
     └ Body   flex:1 overflow-y:auto (.sc)  padding 20px 24px 28px  min-height:0
```

Unlike Work, the Library detail head is **pinned** and only the body scrolls.

### 2.5 Settings view (`view === "settings"`)

```
section  flex:1  display:flex  bg #F5F2EC
 ├ SectionNav  width 220  flex-shrink:0  border-right 1px #E2DDD3  padding 16px 12px
 │   ├ "Settings" 14.5/600  padding 0 8  margin-bottom 14
 │   └ list flex-col gap 2
 └ Body        flex:1  overflow-y:auto (.sc)  padding 26px 32px 34px  min-height:0
     └ inner   max-width 660px
```

### 2.6 Overlays (siblings of the artboard root, `position: absolute`)

| Overlay         | Position                                                                              | z   |
| --------------- | ------------------------------------------------------------------------------------- | --- |
| Toast           | `right 20 · bottom 20`                                                                | 60  |
| Command palette | `inset 0`, scrim `rgba(30,29,27,.28)`, content `justify-center` + `padding-top 120px` | 50  |

### 2.7 Resizers (3 identical)

```
width 7px · flex-shrink 0 · align-self stretch · cursor col-resize
margin 0 -3px · position relative · z-index 10
title "Drag to resize · double-click to reset"
hover background: linear-gradient(90deg, transparent 2px, #B9B3A6 2px, #B9B3A6 4px, transparent 4px)
```

i.e. the hover reveals a 2px `#B9B3A6` hairline centred in the 7px hit area. The negative margin makes the grab zone overlap both neighbours by 3px without shifting layout.

| Resizer               | State key   | Default | Min | Max | Direction                            |
| --------------------- | ----------- | ------- | --- | --- | ------------------------------------ |
| Chat ↔ aside          | `workW`     | 396     | 300 | 600 | `-1` (dragging left grows the aside) |
| Work list ↔ detail    | `workListW` | 340     | 260 | 480 | `+1`                                 |
| Library list ↔ detail | `libListW`  | 326     | 260 | 480 | `+1`                                 |

---

## 3. Component inventory

Every component below is transcribed from the export's inline styles. "—" means the property is not set (browser default).

### 3.1 `TrafficLights`

Decorative macOS window buttons in the rail's top-left.
`display:flex; gap:8px; padding:2px 6px 0; margin-bottom:18px`
Three `span`s: `width/height 12px; border-radius:50%; border:1px solid rgba(0,0,0,.06)`; fills `#E0564A`, `#E8A93D`, `#57A85C`.
No states. In the real Tauri app these should either be replaced by the native traffic lights (`decorations: true` / overlay title bar) or kept as decorative and made functional via `getCurrentWindow().close()/minimize()/toggleMaximize()`.

### 3.2 `Brand`

`display:flex; align-items:center; gap:9px; padding:0 6px; margin-bottom:22px`

- Mark: `22×22; border-radius:5px; background:#1E1D1B; center` containing a `7×7; border-radius:1px; background:#EEEBE4` square.
- Wordmark: "OpenAlpaca" — `14px / 600 / -.01em`.

### 3.3 `NavItem`

The single most reused control. Style function from the logic block:

```
display:flex; align-items:center; gap:9px;
padding:8px 10px; border-radius:7px; border:none;
font-family:inherit; font-size:13px; font-weight:500;
cursor:pointer; text-align:left;
background: active ? #1E1D1B : transparent;
color:      active ? #F5F2EC : #4A4842;
```

| Variant  | Icon                                                              | Trailing                                                                                                                                   |
| -------- | ----------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| Chat     | 15×15 speech bubble, `stroke-width 1.8`, `currentColor`           | —                                                                                                                                          |
| Work     | 15×15 three stacked bars (rects r=1: 18×4 @4, 12×4 @11, 15×3 @18) | **Count badge**: `min-width:17px; height:17px; padding:0 4px; border-radius:9px; background:#C0432B; color:#fff; mono 9.5px/600; centered` |
| Library  | 15×15 folder path                                                 | mono `10px; opacity:.6` count                                                                                                              |
| Settings | 14×14 gear (circle r=3 + 8 spokes)                                | —                                                                                                                                          |

Label span is `flex:1`. **No hover state is declared for nav items** — active/inactive only. (If you add one, `background:#DBD6CB` on the inactive state matches the family; it is an addition, not from the design.)

### 3.4 `RunningNowSection`

- Container: `margin-top:22px; padding:0 6px`.
- Eyebrow: "Running now" — mono `9px`, `#8A8578`, `letter-spacing:.12em`, uppercase, `margin-bottom:8px`.
- List: `flex-col; gap:8px`.
- **`RailRunItem`** (button): `display:flex; align-items:center; gap:7px; background:transparent; border:none; padding:0; cursor:pointer; text-align:left; width:100%; font-family:inherit`. Hover: `opacity:.65`.
  - `StatusDot` 6px (see §3.20).
  - Label: `11.5px; color:#3A3833; line-height:1.35; ellipsis; flex:1` — uses the run's **short** title ("Connector audit", "Migration notes v34", "Memory compaction").
  - When the run is blocked: trailing `wait` — mono `8.5px; color:#C0432B`.
- Membership: runs whose status is **not** `done` and **not** `cancelled`.

### 3.5 `CommandButton` (⌘K affordance)

`display:flex; align-items:center; gap:8px; padding:7px 10px; border-radius:7px; background:#DBD6CB; border:1px solid #CFC9BE; font-family:'IBM Plex Mono'; font-size:10.5px; color:#4A4842; cursor:pointer`
Hover: `background:#D2CCC0`.
Content: `⌘K` (`font-weight:600`) then `Command` (`flex:1; text-align:left`).
This is the only visible entry point to the palette besides the keyboard shortcut.

### 3.6 `ConnectionRow`

`display:flex; align-items:center; gap:8px; padding:0 8px`

- Dot: `7×7; border-radius:50%; background:#2E8B62` (green = connected; use `#C0432B` for error, `#B5892C` for connecting — extrapolated from the status palette, not literally in the design).
- Label: mono `10px; color:#6E6A61` — "connected".
- Instance id: mono `10px; color:#A39D91; margin-left:auto` — "7f3a" (first 4 chars of `instanceId`).

### 3.7 `PaneHeader` (46px)

`height:46px; flex-shrink:0; display:flex; align-items:center; justify-content:space-between; border-bottom:1px solid #E2DDD3`

- Chat: `padding:0 26px`; title `14.5px/600/-.01em` + mono `10.5px #8A8578` date, `align-items:baseline; gap:10px`.
- Work / Library / aside: `padding:0 18px`; title `14.5px/600/-.01em` (aside work header uses `13.5px/600`) + mono `10px #8A8578` counts.

### 3.8 `DensityToggle` (chat header)

`font-size:11.5px; color:#4A4842; background:transparent; border:1px solid #DCD6CB; border-radius:6px; padding:4px 9px; cursor:pointer; font-family:inherit`
Hover `background:#EAE5DA`.
**Label shows the mode you would switch to:** `dense ? "Comfortable" : "Compact"`.

### 3.9 `RunningNowPill` (chat header, only when the aside is fully closed)

Rendered only when `!workOpen && !panelArt`.
`display:flex; align-items:center; gap:6px; font-size:11.5px; color:#1E1D1B; background:#EFEAE0; border:1px solid #DCD6CB; border-radius:6px; padding:4px 9px; cursor:pointer` · hover `#E5DFD3`.
Leading dot `6×6; border-radius:50%; background:#2E8B62; animation:oaP 2s ease-in-out infinite`, then `{activeCount} running`.

### 3.10 Chat message rows

#### `UserMessage`

```
wrapper: margin-bottom: 30px (dense: 20px)
label:   mono 9.5px · #8A8578 · letter-spacing .14em · uppercase · margin-bottom 8px   →  "You · 14:22"
body:    <p> margin 0 · 15px · line-height 1.6 · #1E1D1B · text-wrap: pretty
```

No avatar, no bubble, no background. The speaker label _is_ the avatar.

**Steer variant** — when the user message was routed to a running workflow, the label becomes a flex row (`align-items:center; gap:9px; margin-bottom:8px`) of:

1. the mono speaker label (as above, but a `span`),
2. a **steer pill**: mono `9px; color:#8C4A2F; background:#F3E3D8; border-radius:4px; padding:2px 6px; letter-spacing:.04em` → `steer → connector audit`.

#### `AssistantMessage`

```
wrapper: margin-bottom: 30px (dense: 20px)
header:  display:flex; align-items:center; gap:9px; margin-bottom:8px
  ├ speaker: mono 9.5px · #3A5FCC · letter-spacing .14em · uppercase  → "Alpaca"
  └ meta:    mono 9.5px · #A39D91                                      → "sonnet-4-6 · 3.8s · 1284/612 tok"
body:    <p> margin 0 0 14px · 15px · line-height 1.6 · #1E1D1B · text-wrap: pretty
```

The metadata line maps 1:1 onto the SSE `done` payload: `model · duration_ms → "3.8s" · tokens_in/tokens_out → "1284/612 tok"`.

**Inline code inside a 15px paragraph:** `font-family:'IBM Plex Mono'; font-size:13px; background:#E7E3D9; border-radius:3px; padding:1px 5px`.

Notes:

- **There are no avatar circles anywhere in the design.** Do not add them.
- User and assistant rows are the same width and alignment — no left/right split, no bubbles. Differentiation is purely the coloured mono speaker label.

### 3.11 `StreamingIndicator` — **not present in the design; derive**

The export has no thinking/typing/streaming component. Build it from the design's own vocabulary:

- While `thinking` (before the first `delta`): an assistant header row rendered as usual, with the meta slot replaced by a `6×6` `#2E8B62` dot at `animation:oaP 2s ease-in-out infinite` followed by mono `9.5px #A39D91` "thinking…".
- While `delta`s stream: render the partial text in the normal `AssistantMessage` body; append a 2px-wide × 1em-tall `#1E1D1B` caret at `animation:oaP 1.6s ease-in-out infinite`.
- On `done`: swap the indicator for the real metadata line.
  Do **not** introduce spinners, skeletons, or bouncing dots — nothing of the kind exists in the design language.

### 3.12 `RunReportCard` (a finished workflow reported into the transcript)

```
margin-bottom:26px; border-radius:10px; border:1px solid #DCD6CB;
background:#FFFDF9; overflow:hidden
```

- **Header strip**: `display:flex; align-items:center; gap:9px; padding:10px 14px; border-bottom:1px solid #EFEAE0; background:#FAF7F1`
  - dot `6×6; #2E8B62`
  - mono `9.5px; color:#2E8B62; letter-spacing:.1em; uppercase` → `Run finished · 13:41`
  - right (`margin-left:auto`) mono `10px; color:#A39D91` → `4d81c0a2 · 6m 12s · $0.22`
- **Body**: `padding:13px 14px`
  - title `13.5px/600; margin-bottom:6px`
  - paragraph `margin:0 0 11px; 13.5px; line-height:1.6; color:#4A4842`
  - chip row `display:flex; gap:6px; flex-wrap:wrap`:
    - primary artifact chip — `display:flex; align-items:center; gap:7px; font-size:11.5px; color:#1E1D1B; background:#EFEAE0; border:1px solid #DCD6CB; border-radius:6px; padding:5px 9px` · hover `#E5DFD3`; leading `FileBadge` at **14px** (`border-radius:3px`, mono `6.5px/600`).
    - secondary chip — same box but `background:transparent; color:#4A4842` · hover `#EFEAE0`.
- Status variants for the header strip (extrapolate consistently): failed → dot `#C0432B`, eyebrow `#C0432B`, `Run failed · hh:mm`; cancelled → dot `#8A8578`, eyebrow `#8A8578`.

### 3.13 `ArtifactCard` (inline in the transcript)

```
border:1px solid #DCD6CB; border-radius:10px; background:#FFFDF9;
overflow:hidden; box-shadow:0 1px 2px rgba(30,29,27,.04)
```

| Zone           | Spec                                                                                                                                                                                                                                                   |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Header         | `display:flex; align-items:center; gap:10px; padding:11px 14px; border-bottom:1px solid #EAE5DA`                                                                                                                                                       |
| · badge        | `FileBadge` 19px: `19×19; border-radius:4px; background:<kind color>; color:#fff; mono 8px/600; centered`                                                                                                                                              |
| · name         | `13px/500; flex:1`                                                                                                                                                                                                                                     |
| · version chip | mono `10px; color:#6E6A61; background:#EFEAE0; border-radius:4px; padding:2px 6px` → `v2`                                                                                                                                                              |
| · Open button  | **primary dark sm**: `11.5px/500; color:#F5F2EC; background:#1E1D1B; border:none; border-radius:6px; padding:5px 11px` · hover `#37352F`                                                                                                               |
| Preview body   | `padding:13px 14px; font-family:mono; font-size:11.5px; line-height:1.75; color:#5A564D` — first line (heading) `color:#1E1D1B; font-weight:500`; truncation line `color:#A39D91` → `… 34 more lines`                                                  |
| Footer         | `display:flex; align-items:center; gap:8px; padding:9px 14px; border-top:1px solid #EAE5DA; background:#FAF7F1`                                                                                                                                        |
| · context      | mono `10px; color:#8A8578` → `connector audit · review_agent`                                                                                                                                                                                          |
| · actions      | `margin-left:auto; display:flex; gap:6px` — **ghost xs buttons**: `font-size:11px; color:#4A4842; background:transparent; border:1px solid #DCD6CB; border-radius:5px; padding:3px 8px` · hover `#EFEAE0`. Labels: `Diff v1→v2`, `☆ Pin` / `★ Pinned`. |

### 3.14 `ToolConfirmationBanner` (in the transcript, when blocked)

```
border-radius:10px; border:1px solid #E3C9B8; background:#FBF1E9; padding:15px 16px
```

- Eyebrow: mono `9.5px; color:#8C4A2F; letter-spacing:.12em; uppercase; margin-bottom:9px` → `Confirmation required · shell_execute`
- Command box: mono `12px; color:#1E1D1B; background:#FFFDF9; border:1px solid #E9DBCF; border-radius:6px; padding:9px 11px` → the literal tool argument.
- Note: `font-size:12.5px; color:#6E6A61; margin-top:9px; line-height:1.5` → "review_agent is blocked on this. Answer in the composer to continue."

Maps to SSE `confirmation_requested {request_id, tool_name, tool_arguments}`.

### 3.15 `ResolutionRow` (after approve/deny)

```
display:flex; align-items:center; gap:9px; padding:11px 13px;
border-radius:8px; background:#EFEAE0; border:1px solid #E2DDD3
```

- label mono `9.5px; color:#6E6A61; letter-spacing:.1em; uppercase` → `Approved` / `Denied`
- note `12.5px; color:#4A4842; flex:1`
- time mono `10px; color:#A39D91`

Copy from the design:

- approved → "shell_execute approved · cargo tree returned in 1.4s, review_agent resumed."
- denied → "shell_execute denied · review_agent skipped the dependency check and is finishing with 2 findings."

### 3.16 `Composer`

Outer region: `flex-shrink:0; border-top:1px solid #E2DDD3; background:#F5F2EC; padding:14px 26px 20px`; inner `max-width:720px; margin:0 auto`. Two mutually exclusive states.

#### (a) Blocked state (a tool confirmation is pending)

1. **Warning row** — `display:flex; align-items:center; gap:8px; margin-bottom:10px`: dot `6×6; #C0432B; animation:oaP 1.6s ease-in-out infinite` + mono `10.5px; color:#8C4A2F; letter-spacing:.06em` → `shell_execute is waiting on you`.
2. **Action bar** — `display:flex; gap:8px; align-items:center; border:1px solid #C0432B; border-radius:10px; background:#FFFDF9; padding:8px; box-shadow:0 1px 3px rgba(192,67,43,.1)`

| Button           | Spec                                                                                                                                                                                  |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Approve**      | `flex:1; padding:11px; border-radius:7px; background:#1E1D1B; color:#F5F2EC; border:none; 13px/600` · hover `#37352F`. Trailing hint `↵` — mono `11px; opacity:.55`                   |
| **Deny**         | `flex:1; padding:11px; border-radius:7px; background:transparent; color:#1E1D1B; border:1px solid #DCD6CB; 13px/600` · hover `#EFEAE0`. Trailing hint `esc` — mono `11px; opacity:.5` |
| **Always allow** | `padding:11px 13px; border-radius:7px; background:transparent; color:#6E6A61; border:none; 12px` · hover `color:#1E1D1B`                                                              |

3. **Hint row** — `display:flex; justify-content:space-between; margin-top:9px`, both mono `10px; color:#A39D91`: left `composer paused until answered`, right `{spend} today`.

The textarea is **not rendered** while blocked — the action bar replaces it entirely.

#### (b) Normal state

1. **Steer banner** (only when a steer/follow-up target is set) — `display:flex; align-items:center; gap:8px; margin-bottom:9px`:
   - pill: mono `9px; color:#8C4A2F; background:#F3E3D8; border-radius:4px; padding:2px 7px; letter-spacing:.06em; uppercase` → `steering → Connector audit` or `follow-up → Connector audit`
   - clear link: `font-size:11px; color:#8A8578; background:transparent; border:none; padding:0` · hover `color:#1E1D1B` → "send to assistant instead"
2. **Input shell** — `position:relative; display:flex; gap:9px; align-items:flex-end; border:1px solid #DCD6CB; border-radius:10px; background:#FFFDF9; padding:7px 7px 7px 11px`
   - `textarea rows=1`: `flex:1; resize:none; border:none; outline:none; background:transparent; font-family:inherit; font-size:14px; line-height:1.5; color:#1E1D1B; padding:8px 0; min-height:24px` — auto-grow on input.
   - **Model button**: `display:flex; align-items:center; gap:5px; font-family:mono; font-size:10px; color:#4A4842; background:#F5F2EC; border:1px solid #DCD6CB; border-radius:6px; padding:8px 9px; flex-shrink:0` · hover `border-color:#B9B3A6`; content = model id + chevron `▴`/`▾` (`font-size:8px; color:#8A8578`). `title="Chat model"`.
   - **Send button**: `padding:9px 16px; border-radius:7px; background:#1E1D1B; color:#F5F2EC; border:none; 13px/600` · hover `#37352F`.
   - No focus-ring is declared on the shell. Add `border-color:#B9B3A6` on `:focus-within` to match the picker hover — an acceptable, in-language addition.
3. **Hint row** — `margin-top:9px; justify-content:space-between`, both mono `10px; color:#A39D91`: left `⏎ send · ⇧⏎ newline · ⌘K commands`, right `{spend} today`.

**Placeholder text** (state-dependent):

| Condition       | Placeholder                                        |
| --------------- | -------------------------------------------------- |
| no steer target | `Ask, or describe a job to run in the background…` |
| steer mode      | `Steer {run.short} mid-run…`                       |
| queue mode      | `Queue a follow-up after {run.short}…`             |

### 3.17 `ModelPicker` (popover above the composer)

- Click-catcher: `position:fixed; inset:0; z-index:39`.
- Panel: `position:absolute; bottom:calc(100% + 8px); right:0; width:272px; z-index:40; background:#FFFDF9; border:1px solid #CFC9BE; border-radius:9px; box-shadow:0 12px 32px rgba(30,29,27,.18); overflow:hidden`
- Header: `padding:9px 13px 7px; mono 8.5px; color:#A39D91; letter-spacing:.12em; uppercase; border-bottom:1px solid #F1ECE2` → "Chat model"
- Provider group head: `padding:8px 13px 3px; display:flex; align-items:center; gap:6px`; provider name `11px/600; #1E1D1B`; when the provider is disabled, an **off pill**: mono `8.5px; color:#8C2F1E; background:#F7E7DE; border-radius:4px; padding:1px 5px`.
- Model row: `display:flex; align-items:center; gap:8px; width:100%; text-align:left; font-family:mono; font-size:11px; border:none; padding:7px 13px; cursor:pointer`
  - selected: `color:#1E1D1B; background:#F1ECE2`; unselected `color:#4A4842; background:transparent`; hover `background:#F5F1E8`.
  - leading check span: `width:11px; color:#2E8B62; flex-shrink:0`, content `✓` or empty.
- Footer: `padding:8px 13px; border-top:1px solid #F1ECE2; background:#FAF7F1`; link mono `9.5px; color:#8A8578` · hover `#1E1D1B` → "Manage providers & keys ↗" (navigates to Settings → Models & keys).

### 3.18 `WorkPane` (the aside in `workMode`)

Header (46px, `padding:0 18px`): `Work` `13.5px/600` + mono `10px #8A8578` `{active} active · {done} done`; right group `gap:4`:

- **Full view** — `font-size:11px; color:#6E6A61; background:transparent; border:1px solid #DCD6CB; border-radius:5px; padding:3px 7px` · hover `color:#1E1D1B; background:#EAE5DA`
- **Collapse** — `background:transparent; border:none; color:#8A8578; font-size:15px; padding:2px 6px` · hover `#1E1D1B`; glyph `›`; `aria-label="Collapse work pane"`.

Body: `padding:14px`, lists `RunCard` for every run whose status ≠ `done`.

### 3.19 `RunCard` (aside)

```
border-radius:10px; overflow:hidden; margin-bottom:12px;
border:1px solid  (active ? #D5CFC3 : #DCD6CB)
background:        active ? #FFFDF9 : #F9F6F0
box-shadow:        active ? 0 1px 2px rgba(30,29,27,.05) : none
```

`active` = status is `running` **or** `paused`.

**Body** — `padding:14px 15px 12px` (dense `11px 13px 10px`)

- head row: `display:flex; align-items:flex-start; gap:8px`
  - `StatusDot` 7px, `margin-top:5px`
  - title `13.5px/600; line-height:1.4; letter-spacing:-.005em; text-wrap:pretty`
  - meta row `margin-top:6px; display:flex; flex-wrap:wrap; gap:8px; mono 10px; color:#8A8578` → `<StatusLabel>` (`color:<status color>; font-weight:500`) + free-text meta (`11m 04s · 5/8 steps · $0.41`)
- **Parallel work** block (only when `active`): `margin-top:13px; padding-top:12px; border-top:1px solid #EFEAE0`
  - header row `justify-content:space-between; margin-bottom:8px`, both mono `9px; color:#A39D91; letter-spacing:.12em; uppercase` → "Parallel work" / "now →"
  - lanes list `flex-col; gap:6px` — see `LaneBar` (compact) §3.21
  - note row `margin-top:10px; display:flex; align-items:center; gap:7px; font-size:11.5px; color:#6E6A61; line-height:1.45`; leading dot `6×6; margin-top:3px; background: blocked ? #C0432B : #2E8B62`
- **Files** block (when the run produced artifacts): `margin-top:12px; padding-top:11px; border-top:1px solid #EFEAE0`
  - label mono `9px; #A39D91; .12em; uppercase; margin-bottom:7px` → `Files · {n}`
  - rows `flex-col; gap:4px`; each: `display:flex; align-items:center; gap:8px; padding:6px 9px; border-radius:6px; background:#FAF7F1; border:1px solid #F1ECE2; width:100%; text-align:left` · hover `background:#FFFDF9; border-color:#D5CFC3`; `FileBadge` 17px + name `11.5px; #1E1D1B; ellipsis` + stamp mono `9px; #A39D91`.
  - shows at most 4; overflow link mono `9.5px; color:#8A8578; padding:3px 9px; text-align:left` · hover `#1E1D1B` → `+ {n} more in Library ↗`

**Action bar** — `display:flex; gap:6px; flex-wrap:wrap; padding:10px 15px` (dense `8px 13px`); `border-top:1px solid #EFEAE0; background:#FAF7F1`

- Live runs (status ≠ done/cancelled): `Pause`|`Resume`|`Start now`, `Steer`, `Queue follow-up`, `Jump to chat` as **secondary sm** buttons (`11.5px/500; color:#1E1D1B; background:#EFEAE0; border:1px solid #DCD6CB; border-radius:6px; padding:5px 10px` · hover `#E5DFD3`), plus `Cancel` as **danger ghost** (`color:#8C2F1E; background:transparent; border:1px solid #E3C9B8` · hover `#F7E7DE`).
- Terminal runs: mono `10px; #8A8578; flex:1` note + a single `Re-run` secondary sm button, row `display:flex; gap:9px; align-items:center; width:100%`.

Pause-button label logic: `paused → "Resume"`, `queued → "Start now"`, otherwise `"Pause"`.

### 3.20 `StatusDot` / `StatusLabel`

| Status      | Label       | Text color | Dot fill    | Dot border | Pulse              |
| ----------- | ----------- | ---------- | ----------- | ---------- | ------------------ |
| `running`   | `RUNNING`   | `#2E8B62`  | `#2E8B62`   | —          | **yes** (`oaP 2s`) |
| `queued`    | `QUEUED`    | `#B5892C`  | transparent | `#B5892C`  | no                 |
| `paused`    | `PAUSED`    | `#3A5FCC`  | `#3A5FCC`   | —          | no                 |
| `done`      | `DONE`      | `#6E6A61`  | `#C9C2B5`   | —          | no                 |
| `cancelled` | `CANCELLED` | `#8A8578`  | transparent | `#8A8578`  | no                 |

Dot sizes: `6px` in the rail (border `1px`), `7px` in run cards and work-list rows (border `1.5px`, `margin-top:5px`). `StatusLabel` is always mono, `font-weight:500`, sized `10px` (card) / `9.5px` (list row) / `10.5px` (detail header).

### 3.21 `LaneBar` (parallel-work / timeline visualisation)

A horizontal Gantt lane. Two sizes.

|                 | Compact (aside run card)                                                                        | Wide (work detail Timeline)                            |
| --------------- | ----------------------------------------------------------------------------------------------- | ------------------------------------------------------ |
| Row             | `display:flex; align-items:center; gap:8px`                                                     | `gap:11px`                                             |
| Label           | `width:70px; mono 9.5px; text-align:right; flex-shrink:0`                                       | `width:96px; mono 10.5px; text-align:right`            |
| Label color     | `#6E6A61` (blocked lane: `#8C4A2F`)                                                             | `#4A4842` (blocked lane: `#8C4A2F`)                    |
| Track           | `flex:1; height:8px; border-radius:4px; background:#EFEAE0; position:relative; overflow:hidden` | `height:16px; background:#F1ECE2`                      |
| Bar             | `position:absolute; top:0; bottom:0; border-radius:4px; left:{start}%; width:{end-start}%`      | same                                                   |
| Trailing detail | —                                                                                               | `width:88px; mono 9.5px; color:#8A8578; flex-shrink:0` |

**Bar color by lane state:**

| Lane state          | Color                                                       |
| ------------------- | ----------------------------------------------------------- |
| `done`              | `#2E8B62`                                                   |
| `run` (in progress) | `#B9B3A6`                                                   |
| `block`             | `#C0432B` while the app is blocked, `#2E8B62` once resolved |

**Pending overlay** (only for a `block` lane while blocked): `position:absolute; top:0; bottom:0; left:{end}%; width:{100-end}%; background: repeating-linear-gradient(90deg,#E3C9B8 0 3px, transparent 3px 6px)` — a 3px-on/3px-off amber hatch representing "not yet run, waiting on you".

Sample lane data shape: `{label:"review·3", start:40, end:74, state:"block", detail:"awaiting you"}` — `start`/`end` are percentages of the run's wall clock.

Timeline axis row (wide only): `display:flex; align-items:center; gap:11px; margin-bottom:7px` = a `96px` spacer, a `flex:1` `justify-content:space-between` row of three mono `9.5px #A39D91` labels (start / mid / "14:33 now"), and an `88px` spacer.

### 3.22 `FileBadge`

A colored rounded square holding a 2–3 letter mono uppercase abbreviation.

```
border-radius:4px (32px size: 7px); background:<kind color>; color:#fff;
display:flex; align-items:center; justify-content:center;
font-family:mono; font-weight:600; flex-shrink:0
```

| Size | Font              | Where                                                     |
| ---- | ----------------- | --------------------------------------------------------- |
| 14px | 6.5px, radius 3px | Run-report chip                                           |
| 16px | 6.5px             | Artifact picker rows                                      |
| 17px | 7px               | File rows, library rows, output rows, panel picker button |
| 19px | 8px               | Chat artifact card header                                 |
| 32px | 9.5px, radius 7px | Library detail header                                     |

| Kind                 | Badge                   | Background |
| -------------------- | ----------------------- | ---------- |
| `md` (document)      | `MD`                    | `#3A5FCC`  |
| `code`               | `RS` (language-derived) | `#2E8B62`  |
| `plan`               | `PLN`                   | `#B5892C`  |
| `term` (tool output) | `OUT`                   | `#6E6A61`  |
| `table`              | `CSV`                   | `#7A6BB8`  |
| `html`               | `WEB`                   | `#C0432B`  |
| `image`              | `IMG`                   | `#4A4842`  |

### 3.23 `FilePanel` (the aside when an artifact is open)

Header block (`position:relative; flex-shrink:0; border-bottom:1px solid #E2DDD3; background:#EEEBE4`):

- Row 1 — `display:flex; align-items:center; gap:7px; padding:9px 12px 7px`
  - `‹ Work` — `11.5px/500; color:#4A4842; background:transparent; border:1px solid #DCD6CB; border-radius:6px; padding:5px 9px; gap:4` · hover `background:#E5E1D8`
  - **Picker button** — `display:flex; align-items:center; gap:8px; flex:1; min-width:0; background:#FFFDF9; border:1px solid #DCD6CB; border-radius:7px; padding:6px 10px; text-align:left` · hover `border-color:#B9B3A6`; `FileBadge` 17px + name `12.5px/500; ellipsis` + chevron `9px; color:#8A8578` (`▾`/`▴`)
  - Close `›` — `background:transparent; border:none; color:#8A8578; font-size:15px; padding:2px 6px` · hover `#1E1D1B`; `aria-label="Close file panel"`
- Row 2 — tab strip, `display:flex; align-items:center; padding:0 12px`
  - `PanelTab`: `11.5px/500; padding:6px 10px; background:transparent; border:none; border-bottom:2px solid <INK|transparent>; color:<#1E1D1B|#8A8578>; margin-bottom:-1px`. Tabs: `Preview`, `Diff`, `History`.
  - Right: `Library ↗` — mono `9.5px; color:#8A8578; background:transparent; border:none; padding:4px 0; margin-left:auto` · hover `#1E1D1B`

**Artifact picker dropdown** (`pickerOpen`):

- catcher `position:fixed; inset:0; z-index:30`
- panel `position:absolute; top:calc(100% - 34px); left:12px; right:12px; z-index:31; background:#FFFDF9; border:1px solid #CFC9BE; border-radius:9px; box-shadow:0 12px 32px rgba(30,29,27,.18); max-height:340px; overflow-y:auto` (`.sc`)
- head `padding:8px 11px 6px; mono 8.5px; color:#A39D91; letter-spacing:.12em; uppercase; border-bottom:1px solid #F1ECE2` → `Library · {n} files`
- row `display:flex; align-items:center; gap:9px; padding:8px 11px; border:none; border-bottom:1px solid #F5F1E8; width:100%; text-align:left; background:<#F1ECE2 if current | transparent>`; `FileBadge` 16px + name `12px; ellipsis` + optional star `9.5px; #B5892C` + stamp mono `9px; #A39D91`.

**Panel body** (`padding:13px 14px 18px`, scrollable):

- Meta row `display:flex; align-items:center; gap:8px; margin-bottom:11px; flex-wrap:wrap`: mono `9.5px #8A8578` `{version} · {agent} ·` + run link (mono `9.5px; color:#3A5FCC; text-decoration:underline; border:none; background:transparent; padding:0`) + **small pin toggle** (`margin-left:auto; font-size:10.5px; border-radius:5px; padding:2px 8px`; pinned → `border:1px solid #E0CC9A; background:#F7EFDA; color:#7A5C15`; unpinned → `border:1px solid #DCD6CB; background:transparent; color:#4A4842`; label `★ Pinned` / `☆ Pin`).
- Then the tab content — the **compact** renderers of §3.25.

### 3.24 `WorkListRow` / `CompletedRow` (Work view left column)

- Active row (`rowStyle`): `display:block; width:100%; text-align:left; padding:11px 12px; border-radius:8px; margin-bottom:2px; border:1px solid <#CFC9BE if selected | transparent>; background:<#FFFDF9 if selected | transparent>`
  - inner: `display:flex; align-items:flex-start; gap:8px` — `StatusDot` 7px + column with title `13px/500; line-height:1.4; #1E1D1B; text-wrap:pretty` and meta row `margin-top:5px; gap:8px; mono 9.5px; #8A8578` (`StatusLabel` + meta).
- Section divider: `Completed today` — mono `9px; #A39D91; letter-spacing:.12em; uppercase; margin:16px 0 8px; padding:0 6px`.
- Completed row: same `rowStyle` box, inner `display:flex; align-items:center; gap:8px` — dot `6×6; #C9C2B5` + title `12px; #6E6A61; ellipsis; flex:1` + stamp mono `9.5px; #A39D91`.

### 3.25 Artifact preview renderers

Seven kinds, each with a **compact** (file panel) and **full** (library) variant. Shared shell: `border:1px solid #DCD6CB; border-radius:9px` (library: `10px`) `background:#FFFDF9; overflow:hidden`.

#### (a) Document (`md`)

|               | Compact                                                                                       | Full                                                          |
| ------------- | --------------------------------------------------------------------------------------------- | ------------------------------------------------------------- |
| Padding       | `16px 17px`                                                                                   | `26px 30px`, `max-width:660px`                                |
| Title         | `15px/600/-.015em; line-height:1.4; margin-bottom:3px`                                        | `<h1>` `21px/600/-.02em; margin:0 0 4px`                      |
| Byline        | mono `9.5px; #8A8578; margin-bottom:13px`                                                     | mono `10.5px; #8A8578; margin:0 0 20px`                       |
| Body `<p>`    | `12.5px; line-height:1.65; #3A3833; margin:0 0 12px`                                          | `13.5px; line-height:1.7; #3A3833; margin:0 0 18px`           |
| Subhead       | `12.5px/600; margin-bottom:5px`                                                               | `<h2>` `14px/600; margin:0 0 8px`                             |
| `<ul>`/`<ol>` | `margin:0 0 12–13px; padding-left:17px; 12.5px; line-height:1.65; #3A3833`                    | `margin:0 0 18px; padding-left:20px; 13.5px; line-height:1.7` |
| Inline code   | mono `11px; background:#EFEAE0; border-radius:3px; padding:0 4px`                             | mono `12px; padding:1px 4px`                                  |
| Callout       | `border-left:2px solid #B5892C; padding:1px 0 1px 10px; 12px; line-height:1.6; color:#6E6A61` | `padding:2px 0 2px 12px; 13px; line-height:1.65`              |

#### (b) Code (`code`)

- Header strip: `display:flex; gap:8–10px; padding:8px 11px` (full `9px 14px`) `border-bottom:1px solid #EFEAE0; background:#FAF7F1; mono 9.5px` (full `10px`) `color:#6E6A61` — path (`flex:1; ellipsis`), then `+41` in `#2E8B62` and `−6` in `#C0432B` (full: pushed right with `margin-left:auto`).
- **Compact body:** `mono 10.5px; line-height:1.85; color:#3A3833; padding:6px 0; overflow-x:auto`; each line `padding:0 11px; white-space:pre`. No gutter.
- **Full body (`max-width:760px`):** `mono 11.5px; line-height:1.85; color:#3A3833`; each line is `display:flex` with
  - gutter `width:44px; text-align:right; padding-right:12px; flex-shrink:0; color:#B9B3A6` (added line: `#7EA98F`; removed: `#C99A8C`)
  - content `flex:1; padding-right:14px`
- **Diff line states (both sizes):** added → row `background:#F0F6F1`, text `#1E5C3C`; removed → row `background:#FBF0EC`, text `#8C2F1E`; context → no background, default text.

#### (c) Terminal / tool output (`term`)

```
border:1px solid #DCD6CB; border-radius:9|10px; background:#26241F; overflow:hidden
```

- Header: `display:flex; align-items:center; gap:7–8px; padding:8px 11px` (full `9px 14px`) `border-bottom:1px solid #37342D; mono 9.5px` (full `10px`) `color:#A8A297`; leading `6×6` dot `#2E8B62` (success) — use `#C0432B` for a non-zero exit; text `exit 0 · 1.4s`.
- Body: `padding:11px` (full `14px`) `mono 10.5px` (full `11.5px`) `line-height:1.8; color:#D8D3C8; overflow-x:auto`.
- The command echo line (`$ …`) is `color:#8A857A`.

#### (d) Table (`table`)

- Header row: `display:flex; mono 9px` (full `10px`) `color:#8A8578; letter-spacing:.05em` (full `.06em`) `text-transform:uppercase; border-bottom:1px solid #E2DDD3; background:#FAF7F1`; cells `padding:7px 11px` (full `9px 14px`).
- Data row: `display:flex; font-size:11.5px` (full `12.5px`) `border-bottom:1px solid #F1ECE2` (last row: none); cells `padding:8px 11px` (full `10px 14px`).
- Monospace is applied per-cell for identifier and numeric columns (`10.5px` compact / `11.5px` full); prose cells stay sans.
- Column weights via `flex`: compact `1.6 / 1 / .8`; full `2 / 1 / 1 / 1`.
- Boolean cells: `yes` → `#2E8B62`, `no` → `#C0432B`; plain `yes` in a non-status column stays default ink.

#### (e) Plan / checklist (`plan`)

- Container `padding:14px 15px` (full `18px 20px; max-width:620px`).
- Progress eyebrow: mono `9.5px` (full `10px`) `#8A8578; letter-spacing:.1em; uppercase; margin-bottom:11px` (full `14px`) → `5 of 8 complete`.
- List `flex-col; gap:9px` (full `11px`); each item `display:flex; gap:9px` (full `gap:10px; align-items:flex-start`).

| Step state                      | Box                                                                                                                                                  | Text                                                                                                           |
| ------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| **complete**                    | `14×14` (full `15×15`) `border-radius:4px; background:#2E8B62; color:#fff; font-size:8px` (full `9px`) centered `✓`; `flex-shrink:0; margin-top:2px` | `12.5px` (full `13.5px`) `color:#8A8578; text-decoration:line-through`                                         |
| **blocked / awaiting approval** | same box, `border:1.5px solid #C0432B; background:transparent`                                                                                       | `color:#1E1D1B; font-weight:500` + trailing mono `9.5px` (full `10.5px`) `color:#8C4A2F` → `awaiting approval` |
| **pending**                     | same box, `border:1.5px solid #D5CFC3; background:transparent`                                                                                       | `color:#6E6A61`                                                                                                |

#### (f) Image (`image`)

Card `padding:11px` (full `14px; max-width:700px`); inner placeholder `height:220px` (full `340px`) `border-radius:6px; background:#EFEAE0; border:1px dashed #CFC9BE; flex-col; center; gap:5–6px` with mono `10px` (full `11px`) `#8A8578` filename and mono `9.5px` (full `10px`) `#A39D91` dimensions. Replace the placeholder with the real image (`max-width:100%; border-radius:6px`) when bytes are available; keep the dashed box as the loading/missing state.

#### (g) HTML (`html`)

- Faux browser chrome: `display:flex; align-items:center; gap:7–8px; padding:7px 10px` (full `8px 12px`) `border-bottom:1px solid #EFEAE0; background:#FAF7F1`; three dots `7×7` (full `8×8`) `border-radius:50%; background:#D5CFC3; gap:3px` (full `4px`); filename mono `9.5px` (full `10px`) `#8A8578`.
- Content `padding:16px 17px` (full `26px 30px`): title `14.5px/600/-.015em` (full `19px`), meta mono `9.5px` (full `10.5px`) `#8A8578; margin-bottom:13px` (full `18px`).
- Bar chart: `display:flex; gap:4px` (full `5px`) `align-items:flex-end; height:70px` (full `96px`); bars `flex:1; border-radius:2px 2px 0 0` (full `3px 3px 0 0`); heights are `%`; primary bars `#3A5FCC`, de-emphasised bars `#C9C2B5`.

#### `DiffTab` (standalone, both sizes)

- Header: `display:flex; gap:8–10px; padding:8px 11px` (full `10px 14px`) `border-bottom:1px solid #EFEAE0; background:#FAF7F1; mono 9.5px` (full `10.5px`) `color:#6E6A61` → `v1 → v2`, then times in `#A39D91`, then `margin-left:auto` `+9` (`#2E8B62`) and `−2` (`#C0432B`).
- Lines: `mono 10.5px; line-height:1.85; padding:5px 0` (full `11.5px; line-height:1.9; padding:6px 0`); each line `padding:0 11px` (full `0 14px`); context `color:#8A8578`; added `background:#F0F6F1; color:#1E5C3C`; removed `background:#FBF0EC; color:#8C2F1E`. Compact adds `white-space:pre` and the container `overflow-x:auto`.

#### `HistoryTab` / `VersionRow`

List `flex-col; gap:7px` (full `8px; max-width:660px`).

```
display:flex; align-items:flex-start; gap:10px (full 12px);
padding:10px 12px (full 12px 14px); border-radius:8px (full 9px);
border:1px solid  (index 0 ? #CFC9BE : #E2DDD3)
background:        index 0 ? #FFFDF9 : #F9F6F0
```

- version label: mono `10.5px` (full `11px`) `/500; color:#1E1D1B; width:24px` (full `26px`) `flex-shrink:0` → `v2`
- note: `12px` (full `12.5px`) `color:#1E1D1B; line-height:1.5`
- author: mono `9px` (full `9.5px`) `color:#8A8578; margin-top:2px` (full `3px`)
- timestamp: mono `9.5px` (full `10px`) `color:#8A8578; flex-shrink:0`

Versions are listed **newest first**; index 0 gets the raised treatment.

### 3.26 Work detail header + actions

- `<h2>` `20px/600/-.02em; line-height:1.3; text-wrap:pretty; margin:0`
- meta row `margin-top:8px; display:flex; flex-wrap:wrap; gap:10px; mono 10.5px; color:#8A8578`: `StatusLabel` + run id + meta + `started 14:22:41`
- action group `margin:16px 0 22px; display:flex; gap:6px; flex-wrap:wrap` — **secondary md** buttons: `12px/500; color:#1E1D1B; background:#EFEAE0; border:1px solid #DCD6CB; border-radius:6px; padding:6px 11px` · hover `#E5DFD3`; `Cancel run` uses the danger ghost (`#8C2F1E` / `#E3C9B8` / hover `#F7E7DE`).
- **Terminal banner** (replaces the actions when the run finished/was cancelled): `display:flex; gap:11px; align-items:center; padding:11px 14px; border-radius:8px`
  - done → `background:#E3F0E8; border:1px solid #CBE0D3`; text `12.5px; color:#1E5C3C; flex:1` → `Finished · {note}`
  - cancelled → `background:#F7E7DE; border:1px solid #E9D3C6`; text `12.5px; color:#8C2F1E; flex:1` → "Cancelled by you · no further steps will run"
  - trailing buttons `Jump to chat`, `Re-run`: `12px/500; color:#1E1D1B; background:#FFFDF9; border:1px solid #DCD6CB; border-radius:6px; padding:6px 11px` · hover `#EFEAE0`

### 3.27 `SectionCard` (Timeline / Output / Event log)

```
border:1px solid #DCD6CB; border-radius:10px; background:#FFFDF9;
overflow:hidden; margin-bottom:16px
```

- Header (Output/Event log): `padding:12px 16px; border-bottom:1px solid #EFEAE0; mono 9.5px; color:#8A8578; letter-spacing:.12em; text-transform:uppercase`.
- Timeline card instead uses `padding:16px 18px` with the same eyebrow inline (`margin-bottom:12px`).
- Empty state: `padding:14px 16px; font-size:13px; color:#8A8578; line-height:1.6` (Timeline's empty state has no extra padding since the card is already padded).
- **Output row**: `display:flex; align-items:center; gap:10px; padding:11px 16px; border:none; border-bottom:1px solid #F1ECE2; background:transparent; width:100%; text-align:left` · hover `background:#FAF7F1`; `FileBadge` 17px + name `12.5px; #1E1D1B; flex:1` + stamp mono `10px; #8A8578`.
- **Event row**: `display:flex; align-items:center; gap:10px; padding:7px 16px` (list wrapper `padding:6px 0`); `LogTag` + text `12px; color:#4A4842; flex:1; ellipsis` + time mono `9.5px; color:#A39D91`.

### 3.28 `LogTag`

```
font-family:mono; font-size:9px; letter-spacing:.08em; text-transform:uppercase;
border-radius:4px; padding:2px 6px; width:58px; text-align:center; flex-shrink:0
```

| Tag                   | Color     | Background |
| --------------------- | --------- | ---------- |
| `tool`                | `#8C4A2F` | `#F3E3D8`  |
| `steer`               | `#3A5FCC` | `#E4EAFA`  |
| `artifact`            | `#5B4B9A` | `#E9E5F5`  |
| `spawn`               | `#2E8B62` | `#E3F0E8`  |
| `run` / anything else | `#6E6A61` | `#EFEAE0`  |

The fixed `58px` width keeps the message column aligned — keep it.

### 3.29 `KindFilterChip` (Library)

```
font-size:11px; padding:4px 9px; border-radius:99px; cursor:pointer;
border:1px solid (selected ? #1E1D1B : #DCD6CB);
background:          selected ? #1E1D1B : transparent;
color:               selected ? #F5F2EC : #4A4842;
```

Kinds: `All`, `Docs`, `Code`, `Output`, `Data`, `Media`, `Plans` — mapping to artifact kinds: Docs→`md`, Code→`code`, Output→`term`, Data→`table`, Media→`image`+`html`, Plans→`plan`.

### 3.30 `LibraryRow`

```
display:flex; align-items:center; gap:10px; padding:9px 10px; border-radius:8px;
border:1px solid (active ? #CFC9BE : transparent);
background:        active ? #FFFDF9 : transparent;
width:100%; text-align:left
```

`FileBadge` 17px; then a column: name row (`display:flex; align-items:center; gap:6px`) with name `12.5px/500; #1E1D1B; ellipsis` and optional star `10px; #B5892C`; subtitle mono `9.5px; #8A8578; margin-top:3px; ellipsis` → `review_agent · connector audit · 2m ago`.

### 3.31 Library detail header

- `display:flex; align-items:flex-start; gap:12px`
- `FileBadge` 32px
- `<h2>` `17px/600/-.015em; margin:0`
- meta row `margin-top:6px; display:flex; flex-wrap:wrap; gap:10px; mono 10px; color:#8A8578`: version, agent, run link (`color:#3A5FCC; text-decoration:underline; background:transparent; border:none; padding:0; font-size:10px`), time.
- action group `display:flex; gap:6px; flex-shrink:0`: **Pin toggle** (`11.5px; border-radius:6px; padding:5px 10px`; pinned → `border:1px solid #E0CC9A; background:#F7EFDA; color:#7A5C15`; unpinned → `border:1px solid #DCD6CB; background:transparent; color:#4A4842`), then `Export` and `Reveal` as **ghost sm** (`11.5px; color:#4A4842; background:transparent; border:1px solid #DCD6CB; border-radius:6px; padding:5px 10px` · hover `#EFEAE0`).
- Tab strip: `display:flex; gap:2px; margin-top:16px; border-bottom:1px solid #E2DDD3`; `ArtifactTab` = `12.5px/500; padding:8px 13px; background:transparent; border:none; border-bottom:2px solid <INK|transparent>; color:<#1E1D1B|#8A8578>; margin-bottom:-1px`.

### 3.32 Settings components

- **SectionNavItem**: `display:flex; align-items:center; gap:8px; padding:7px 9px; border-radius:7px; border:none; font-size:12.5px; text-align:left; background:<#E5E1D8 if active | transparent>; color:<#1E1D1B|#4A4842>; font-weight:<500|400>`; optional trailing count mono `10px; opacity:.55`.
- **Page head**: `<h2>` `19px/600/-.02em; margin:0 0 4px`; blurb `13px; color:#6E6A61; line-height:1.6; margin:0 0 22px`.
- **StatusCard** (Connection): `border:1px solid #DCD6CB; border-radius:10px; background:#FFFDF9; padding:16px 18px`
  - head row `gap:10px`: dot `8×8; #2E8B62` + `13.5px/600` title + mono `10.5px; #8A8578; margin-left:auto` uptime
  - grid `display:grid; grid-template-columns:repeat(3,1fr); gap:14px; margin-top:16px`; each cell = eyebrow (mono `9px; #A39D91; letter-spacing:.1em; uppercase; margin-bottom:4px`) + value (mono `12px`)
  - buttons `margin-top:16px; gap:6`: primary-muted (`11.5px; #1E1D1B; background:#EFEAE0; border:1px solid #DCD6CB; border-radius:6px; padding:5px 10px` · hover `#E5DFD3`) and ghost (`transparent; color:#4A4842` · hover `#EFEAE0`)
- **StatCard** (Today): title `13.5px/600; margin-bottom:12px`; stats row `display:flex; gap:26px`; value `19px/600`; label mono `9.5px; #8A8578; margin-top:2px`; progress bar `height:6px; border-radius:3px; background:#EFEAE0; margin-top:14px; overflow:hidden` with fill `width:{pct}%; height:100%; background:#2E8B62; border-radius:3px`.
- **ListCard**: `border:1px solid #DCD6CB; border-radius:10px; background:#FFFDF9; overflow:hidden`
  - optional add bar: `display:flex; justify-content:flex-end; padding:10px 14px; border-bottom:1px solid #F1ECE2; background:#FAF7F1` with a **primary dark sm** button (`11.5px/500; color:#F5F2EC; background:#1E1D1B; border:none; border-radius:6px; padding:5px 11px` · hover `#37352F`)
  - row: `display:flex; align-items:center; gap:12px; padding:13px 16px; border-bottom:1px solid #F1ECE2`
    - name `13px/500` + optional `SettingsTag`
    - desc `12px; color:#6E6A61; margin-top:3px; line-height:1.5`
    - optional model chips row `margin-top:8px; display:flex; gap:5px; flex-wrap:wrap`
    - meta mono `10px; color:#A39D91; flex-shrink:0`
    - `Toggle`
- **SettingsTag**: mono `9px; letter-spacing:.06em; uppercase; border-radius:4px; padding:2px 6px`

| Tag value                 | Color     | Background |
| ------------------------- | --------- | ---------- |
| `unwired`, `warn`         | `#8C2F1E` | `#F7E7DE`  |
| `asks`                    | `#8C4A2F` | `#F3E3D8`  |
| `live`, `active`          | `#1E5C3C` | `#E3F0E8`  |
| anything else (`default`) | `#4A4842` | `#EFEAE0`  |

- **ModelChip**: `font-family:mono; font-size:10px; padding:3px 8px; border-radius:99px; border:1px solid <INK|#DCD6CB>; background:<INK|transparent>; color:<#F5F2EC|#4A4842>` · hover `border-color:#B9B3A6`. Selected chips are prefixed with `✓ `. Clicking a chip sets the chat model and fires a toast.
- **Toggle**: track `width:34px; height:19px; border-radius:99px; border:1px solid <#2E8B62 on | #D5CFC3 off>; background:<#2E8B62 | #EFEAE0>; display:flex; align-items:center; padding:0 2px; justify-content:<flex-end | flex-start>`; knob `width:13px; height:13px; border-radius:50%; background:#FFFDF9; display:block`. (No transition declared; `justify-content` is the on/off mechanism — a `transform` transition is a safe refinement.)
- **Log row**: `display:flex; align-items:center; gap:10px; padding:8px 14px; border-bottom:1px solid #F5F1E8`; `LogTag` + text **mono** `11px; color:#4A4842; ellipsis; flex:1` + time mono `9.5px; #A39D91`. (Note: the settings log row uses mono for the message; the work-detail event row uses sans.)

### 3.33 `CommandPalette`

- Overlay: `position:absolute; inset:0; background:rgba(30,29,27,.28); display:flex; align-items:flex-start; justify-content:center; padding-top:120px; z-index:50`. Clicking the overlay itself (not a child) closes it.
- Dialog: `width:560px; border-radius:12px; background:#FFFDF9; border:1px solid #CFC9BE; box-shadow:0 18px 50px rgba(30,29,27,.22); overflow:hidden`
- Head: `display:flex; align-items:center; gap:10px; padding:14px 16px; border-bottom:1px solid #EFEAE0`
  - `⌘K` mono `11px; color:#A39D91`
  - input `flex:1; border:none; outline:none; background:transparent; font-family:inherit; font-size:14px; color:#1E1D1B`; placeholder `Run a command, steer a task, find an artifact…`; **autofocused on mount**
  - `esc` mono `10px; color:#A39D91`
- Body: `padding:8px`
- Row: `display:flex; align-items:center; gap:11px; padding:9px 10px; border-radius:7px; border:none; background:transparent; width:100%; text-align:left` · hover `background:#F1ECE2`
  - group label mono `9px; letter-spacing:.08em; uppercase; color:#8A8578; width:66px; flex-shrink:0`
  - label `13px; color:#1E1D1B; flex:1`
  - shortcut mono `10px; color:#A39D91`

Commands shipped in the design (group / label / key):

| Group   | Label                         | Key      | Action                                                         |
| ------- | ----------------------------- | -------- | -------------------------------------------------------------- |
| Run     | New background job            | `⌘N`     | close palette, go to chat                                      |
| Steer   | Steer connector audit         | `⌘⇧S`    | close, go to chat, set steer target to the running run         |
| Approve | Approve pending shell_execute | `↵`      | close, resolve the pending confirmation as approved            |
| Go      | Work — all runs               | `⌘2`     | close, `view = work`                                           |
| Go      | Library — artifacts           | `⌘3`     | close, `view = library`                                        |
| Find    | connector-audit-findings.md   | _(none)_ | close, `view = library`, open that artifact on the Preview tab |
| View    | Toggle compact density        | `⌘⇧D`    | close, flip `dense`                                            |
| Go      | Settings — skills & plugins   | `⌘,`     | close, `view = settings`, `secId = skills`                     |

Only `⌘K` and `esc` are actually bound in the design; the other key hints are **display-only** and must be wired for real (`⌘1` for chat is implied but absent). No filtering-as-you-type is implemented in the export — implement substring match over `group + label`.

### 3.34 `Toast`

```
position:absolute; right:20px; bottom:20px; z-index:60;
display:flex; align-items:center; gap:10px;
background:#1E1D1B; color:#F5F2EC; border-radius:9px; padding:10px 14px;
box-shadow:0 8px 24px rgba(30,29,27,.25); max-width:380px
```

- dot `6×6; border-radius:50%; background:#7FC79A; flex-shrink:0`
- text `12.5px; line-height:1.45`
- Auto-dismisses after **2600 ms**; a new toast clears the pending timer (single-slot, no stack).

### 3.35 Button catalogue (consolidated)

| Variant                                       | Bg          | Border        | Text      | Radius | Padding                | Size/Weight       | Hover                  |
| --------------------------------------------- | ----------- | ------------- | --------- | ------ | ---------------------- | ----------------- | ---------------------- |
| **Primary block** (Approve)                   | `#1E1D1B`   | none          | `#F5F2EC` | 7      | `11px`                 | 13/600            | `#37352F`              |
| **Primary md** (Send)                         | `#1E1D1B`   | none          | `#F5F2EC` | 7      | `9px 16px`             | 13/600            | `#37352F`              |
| **Primary sm** (Open, Add provider)           | `#1E1D1B`   | none          | `#F5F2EC` | 6      | `5px 11px`             | 11.5/500          | `#37352F`              |
| **Secondary md**                              | `#EFEAE0`   | `1px #DCD6CB` | `#1E1D1B` | 6      | `6px 11px`             | 12/500            | `#E5DFD3`              |
| **Secondary sm**                              | `#EFEAE0`   | `1px #DCD6CB` | `#1E1D1B` | 6      | `5px 10px`             | 11.5/500          | `#E5DFD3`              |
| **Secondary xs** (running pill)               | `#EFEAE0`   | `1px #DCD6CB` | `#1E1D1B` | 6      | `4px 9px`              | 11.5/400          | `#E5DFD3`              |
| **Ghost sm**                                  | transparent | `1px #DCD6CB` | `#4A4842` | 6      | `5px 10px`             | 11.5/400          | `#EFEAE0`              |
| **Ghost xs**                                  | transparent | `1px #DCD6CB` | `#4A4842` | 5      | `3px 8px`              | 11/400            | `#EFEAE0`              |
| **Ghost 2xs** (Full view)                     | transparent | `1px #DCD6CB` | `#6E6A61` | 5      | `3px 7px`              | 11/400            | `#EAE5DA` + `#1E1D1B`  |
| **Outline on raised** (Jump/Re-run in banner) | `#FFFDF9`   | `1px #DCD6CB` | `#1E1D1B` | 6      | `6px 11px`             | 12/500            | `#EFEAE0`              |
| **Danger ghost** (Cancel)                     | transparent | `1px #E3C9B8` | `#8C2F1E` | 6      | `5–6px 10–11px`        | 11.5–12/500       | `#F7E7DE`              |
| **Bare link**                                 | transparent | none          | `#8A8578` | —      | `0` or `4px 0`         | 9.5–11px          | `color:#1E1D1B`        |
| **Icon/glyph** (`›`)                          | transparent | none          | `#8A8578` | —      | `2px 6px`              | 15px              | `color:#1E1D1B`        |
| **Pin (off)**                                 | transparent | `1px #DCD6CB` | `#4A4842` | 5–6    | `2px 8px` / `5px 10px` | 10.5/11.5         | —                      |
| **Pin (on)**                                  | `#F7EFDA`   | `1px #E0CC9A` | `#7A5C15` | 5–6    | same                   | same              | —                      |
| **Chip (off)**                                | transparent | `1px #DCD6CB` | `#4A4842` | 99     | `3px 8px` / `4px 9px`  | mono 10 / sans 11 | `border-color:#B9B3A6` |
| **Chip (on)**                                 | `#1E1D1B`   | `1px #1E1D1B` | `#F5F2EC` | 99     | same                   | same              | —                      |

No `:focus-visible` styling exists in the design. **Add one** for accessibility — recommended: `outline:2px solid #3A5FCC; outline-offset:2px` — and note it as an intentional addition.

### 3.36 `Tab` (two sizes)

```
font-family:inherit; font-weight:500; background:transparent; border:none;
border-bottom:2px solid (active ? #1E1D1B : transparent);
color:(active ? #1E1D1B : #8A8578); cursor:pointer; margin-bottom:-1px;
```

- Panel tabs: `font-size:11.5px; padding:6px 10px`
- Library tabs: `font-size:12.5px; padding:8px 13px`
  The `margin-bottom:-1px` overlaps the container's `1px` bottom border so the active underline sits flush.

---

## 4. Interaction spec

Transcribed from the `class Component extends DCLogic` block. This is the complete stateful behaviour of the design.

### 4.1 Editor props (canvas-only; become real data)

| Prop              | Type    | Default | Real-world source                                                                  |
| ----------------- | ------- | ------- | ---------------------------------------------------------------------------------- |
| `pendingApproval` | boolean | `true`  | a live `confirmation_requested` SSE event / `tool_confirmation_requested` WS event |
| `workPaneOpen`    | boolean | `true`  | user preference (persist)                                                          |
| `compactDensity`  | boolean | `false` | user preference (persist)                                                          |

### 4.2 State inventory

| Key               | Type                                          | Initial               | Meaning                                                   |
| ----------------- | --------------------------------------------- | --------------------- | --------------------------------------------------------- |
| `view`            | `"chat" \| "work" \| "library" \| "settings"` | `"chat"`              | active top-level view                                     |
| `workOpen`        | boolean                                       | `true`                | chat aside shows the Work pane                            |
| `dense`           | boolean                                       | `false`               | compact density                                           |
| `palette`         | boolean                                       | `false`               | ⌘K palette open                                           |
| `blocked`         | boolean                                       | `true`                | a tool confirmation is pending                            |
| `resolution`      | `null \| "approved" \| "denied"`              | `null`                | outcome of the last confirmation (drives `ResolutionRow`) |
| `steerTarget`     | `null \| runId`                               | `null`                | composer is aimed at a running workflow                   |
| `composerMode`    | `"steer" \| "queue"`                          | `"steer"`             | steer vs. queue-follow-up                                 |
| `toast`           | `null \| string`                              | `null`                | transient toast text                                      |
| `workW`           | number                                        | `396`                 | aside width (300–600)                                     |
| `workListW`       | number                                        | `340`                 | Work list width (260–480)                                 |
| `libListW`        | number                                        | `326`                 | Library list width (260–480)                              |
| `panelArt`        | `null \| artifactId`                          | `null`                | aside shows the file panel for this artifact              |
| `panelTab`        | `"preview" \| "diff" \| "history"`            | `"preview"`           | file-panel tab                                            |
| `pickerOpen`      | boolean                                       | `false`               | file-panel artifact dropdown                              |
| `model`           | string                                        | `"claude-sonnet-4-6"` | chat model                                                |
| `modelPickerOpen` | boolean                                       | `false`               | composer model popover                                    |
| `sel`             | runId                                         | `"b41c8e02"`          | selected run in Work                                      |
| `openArt`         | artifactId                                    | `"findings"`          | selected artifact in Library                              |
| `artTab`          | `"preview" \| "diff" \| "history"`            | `"preview"`           | Library tab                                               |
| `pins`            | `Record<artifactId, boolean>`                 | `{findings:true}`     | pinned artifacts                                          |
| `libKind`         | string                                        | `"All"`               | Library kind filter                                       |
| `secId`           | string                                        | `"connection"`        | Settings section                                          |
| `runs`            | `Run[]`                                       | 6 seeded runs         | the run collection                                        |

**Derived values** (recomputed every render):

- `activeCount` = runs where status ∈ {running, queued, paused} → drives the Work nav badge and the "N running" pill.
- `doneCount` = runs where status = done.
- `railRuns` = runs where status ∉ {done, cancelled}.
- `paneRuns` / `listRuns` = runs where status ≠ done.
- `doneRuns` = runs where status = done.
- `artCount` = total artifacts.
- `spend` = `"$0.0184"` (static in the mock; wire to real cost).
- `showAside` = `workOpen || panelArt !== null`.
- `workMode` = `panelArt === null`; `panelOn` = `panelArt !== null`. **These are mutually exclusive** — opening an artifact replaces the Work pane in the same aside.
- `workClosed` = `!workOpen && !panelArt` → the only condition that shows the header "N running" pill.
- `transcriptStyle` max-width = `dense ? 780 : 720`; `msgGap` margin-bottom = `dense ? 20 : 30`.

### 4.3 Run model

```ts
type RunStatus = "running" | "queued" | "paused" | "done" | "cancelled";
interface Run {
  id: string; // "b41c8e02" — 8-hex task id
  title: string; // full sentence, used in cards/detail
  short: string; // 2–3 words, used in the rail, toasts, composer placeholder
  status: RunStatus;
  meta: string; // "11m 04s · 5/8 steps · $0.41"
  started: string; // "14:22:41"
  stamp?: string; // "13:41" — completed-row timestamp
  note: string; // one-line human status
}
```

Derived: `isActive` = running|paused · `isLive` = status ∉ {done, cancelled} · `isTerminal` = status ∈ {done, cancelled} · `isBlocked` = this run is the one holding the pending confirmation.

### 4.4 Handlers — complete list

**Navigation**

| Handler                                          | Effect                                                                                                                                            |
| ------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| `goChat` / `goWork` / `goLibrary` / `goSettings` | set `view`                                                                                                                                        |
| `r.focus` (rail item, work-list row)             | `view = "work"; sel = r.id`                                                                                                                       |
| `a.open` (library row, output row)               | `view = "library"; openArt = a.id; artTab = "preview"`                                                                                            |
| `f.openSide` (file row in a run card)            | `view = "chat"; panelArt = a.id; panelTab = "preview"; pickerOpen = false`                                                                        |
| `art.jumpRun` / `pArt.jumpRun`                   | `view = "work"; sel = <artifact's run>`                                                                                                           |
| `openInLibrary`                                  | `view = "library"; openArt = panelArt ?? "findings"; artTab = panelTab; panelArt = null; pickerOpen = false` — **carries the current tab across** |
| `goModelSettings`                                | `view = "settings"; secId = "models"; modelPickerOpen = false`                                                                                    |
| `r.allFiles`                                     | `view = "library"; libKind = "All"; openArt = <first file of run>`                                                                                |

**Aside**

| Handler                        | Effect                                                                                 |
| ------------------------------ | -------------------------------------------------------------------------------------- |
| `openWorkPane`                 | `workOpen = true; panelArt = null`                                                     |
| `closeWorkPane`                | `workOpen = false` (aside disappears if no `panelArt`)                                 |
| `backToWork` (`‹ Work`)        | `panelArt = null; workOpen = true; pickerOpen = false`                                 |
| `closePanel` (`›`)             | `panelArt = null; workOpen = false; pickerOpen = false` — collapses the aside entirely |
| `togglePicker` / `closePicker` | flip / clear `pickerOpen`                                                              |
| `p.pick`                       | `panelArt = p.id; panelTab = "preview"; pickerOpen = false`                            |
| `t.pick` (panel tab)           | `panelTab = label.toLowerCase()`                                                       |

**Confirmation**

| Handler         | Effect                                                                                           |
| --------------- | ------------------------------------------------------------------------------------------------ |
| `approve`       | `resolve("approved")`                                                                            |
| `deny`          | `resolve("denied")`                                                                              |
| `approveAlways` | `resolve("approved")` **plus** toast `shell_execute added to the allowlist — it won't ask again` |

`resolve(kind)` does three things atomically:

1. `blocked = false`
2. `resolution = kind`
3. patches the blocked run: `note` becomes `"cargo tree returned · review_agent resuming"` (approved) or `"confirmation denied · review_agent skipped the step"` (denied); `meta` advances `11m 04s · 5/8 steps · $0.41` → `11m 41s · 6/8 steps · $0.43`.

Unblocking also flips every `block`-state lane bar from red to green and removes its hatched pending overlay, and changes the run-card note dot from red to green — a single state change with three visual consequences. Preserve that coupling.

**Run controls** (each also fires a toast)

| Handler         | Effect                                                                                                                                                                                 | Toast                                                                  |
| --------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| `r.togglePause` | no-op if cancelled; otherwise `status = running` (if it wasn't running) else `paused`; note becomes `"resumed · picking up where it stopped"` / `"paused by you · resume to continue"` | `{short} started` (from queued) / `{short} resumed` / `{short} paused` |
| `r.cancel`      | `status = "cancelled"; note = "cancelled by you"`                                                                                                                                      | `{short} cancelled`                                                    |
| `r.rerun`       | `status = "queued"; note = "re-queued by you"`                                                                                                                                         | `{short} re-queued`                                                    |
| `r.steer`       | `view = "chat"; steerTarget = r.id; composerMode = "steer"`                                                                                                                            | —                                                                      |
| `r.queue`       | `view = "chat"; steerTarget = r.id; composerMode = "queue"`                                                                                                                            | —                                                                      |
| `r.jump`        | `view = "chat"; steerTarget = null`                                                                                                                                                    | —                                                                      |
| `clearSteer`    | `steerTarget = null`                                                                                                                                                                   | —                                                                      |

**Model**

| Handler                                  | Effect                                                                      |
| ---------------------------------------- | --------------------------------------------------------------------------- |
| `toggleModelPicker` / `closeModelPicker` | flip / clear `modelPickerOpen`                                              |
| `m.pick`                                 | `model = m; modelPickerOpen = false`; toast `Chat model → {m} ({provider})` |
| `modelChip.pick` (Settings)              | `model = m`; same toast; picker stays closed                                |

**Pins**
`togglePin` (library header), `pArt.togglePin` (panel), `pinFindings` (chat card) all flip `pins[id]`. The library/panel versions toast `{name} pinned` / `{name} unpinned`; `pinFindings` (the chat artifact card) does **not** toast. Label is always `★ Pinned` / `☆ Pin`.

**Filters, tabs, sections**
`k.pick` → `libKind = k` · `t.pick` (library) → `artTab` · `s.pick` → `secId` · `toggleDense` → flip `dense` · `sectionAdd` → toast `{section.add} — hooked up to the daemon in the real build` (a stub; wire to real flows).

**Palette**
`openPalette` → `palette = true` · `closePalette(e)` → closes **only if the click target is the overlay itself** (`e.target === e.currentTarget`), so clicks inside the dialog do not dismiss · `c.run` → each command's own action (§3.33).

**Toast**
`toast(text)` clears any pending timer, sets the text, and schedules a clear at **2600 ms**. Only one toast at a time.

### 4.5 Keyboard

Bound on `window` via a `keydown` listener installed in `componentDidMount` and removed in `componentWillUnmount`.

| Key                | Condition                                | Effect                                                        |
| ------------------ | ---------------------------------------- | ------------------------------------------------------------- |
| `⌘K` / `Ctrl+K`    | always                                   | `preventDefault()`; **toggles** the palette (open→close too)  |
| `Esc`              | palette open                             | close palette                                                 |
| `Esc`              | else if artifact picker open             | close picker                                                  |
| `Esc`              | else if a file panel is open             | `panelArt = null; workOpen = true` (returns to the Work pane) |
| `Esc`              | else if blocked                          | `resolve("denied")`                                           |
| `Enter` (no Shift) | `blocked && !palette && view === "chat"` | `preventDefault()`; `resolve("approved")`                     |

The Escape ladder is strictly ordered — implement it as an if/else-if chain, not four independent handlers.

Composer key affordances are **advertised** in the hint row (`⏎ send · ⇧⏎ newline · ⌘K commands`) but the Enter-to-send handler is not implemented in the mock. Implement:

- `Enter` without Shift in the textarea → send (and `preventDefault`)
- `Shift+Enter` → newline
- while blocked the textarea is unmounted, so the global `Enter`→approve binding has no conflict.

### 4.6 Pane resizing

`startDrag(key, min, max, dir, e)`:

1. `preventDefault()`; capture `startX = e.clientX`, `startW = state[key]`
2. set `document.body.style.userSelect = "none"` and `cursor = "col-resize"`
3. on `mousemove`: `w = clamp(min, max, startW + dir * (ev.clientX - startX))`
4. on `mouseup`: remove both window listeners, restore body styles, **persist** all three widths

Double-click on a resizer resets that pane to its default and persists.

**Persistence:** `localStorage["oa-pane-widths"] = JSON.stringify({workW, workListW, libListW})`, read in `componentDidMount` inside a `try/catch` with per-key fallbacks to the defaults. Reuse this exact key and shape so an existing user's layout survives the rework.

### 4.7 Hover-state inventory (complete)

| Hover declaration                          | Count | Applies to                                             |
| ------------------------------------------ | ----- | ------------------------------------------------------ |
| `background:#E5DFD3`                       | 12    | Every secondary (`#EFEAE0`) button                     |
| `background:#EFEAE0`                       | 9     | Every ghost button, Deny, banner outline buttons       |
| `color:#1E1D1B`                            | 7     | Bare links, glyph buttons, "Always allow", "Library ↗" |
| `background:#37352F`                       | 4     | Every primary dark button                              |
| `border-color:#B9B3A6`                     | 3     | Model button, panel picker button, model chips         |
| resizer gradient                           | 3     | The three resizers                                     |
| `background:#F7E7DE`                       | 2     | Danger ghost (Cancel)                                  |
| `background:#EAE5DA`                       | 1 + 1 | Density toggle; "Full view" (+ `color:#1E1D1B`)        |
| `background:#FFFDF9; border-color:#D5CFC3` | 1     | Run-card file rows                                     |
| `background:#FAF7F1`                       | 1     | Output rows                                            |
| `background:#F5F1E8`                       | 1     | Model-picker rows                                      |
| `background:#F1ECE2`                       | 1     | Palette rows                                           |
| `background:#E5E1D8`                       | 1     | `‹ Work` button                                        |
| `background:#D2CCC0`                       | 1     | ⌘K command button                                      |
| `opacity:.65`                              | 1     | Rail run items                                         |

**No `:active` (pressed) states are declared anywhere.** A subtle press treatment (e.g. one step darker than hover) is an acceptable addition; nav items, run-list rows, library rows and work-list rows rely purely on the selected/unselected border+background swap.

---

## 5. Screens and views

Four top-level views, switched from the nav rail. Three overlays cut across all of them.

### 5.1 Chat (`view === "chat"`) — the default

Left: the transcript column. Right (optional): the aside, in one of two modes.

**Transcript content, in the order the design shows it:**

1. `RunReportCard` — a completed background workflow reported into the lane, with its artifact chips.
2. `UserMessage`.
3. `AssistantMessage` with prose + an inline `ArtifactCard`.
4. `UserMessage` **steer variant** (routed to a running workflow, carrying the `steer → connector audit` pill).
5. `ToolConfirmationBanner` — rendered when `blocked`.
6. `ResolutionRow` — rendered when `resolution !== null` (i.e. after the banner is answered; the two are never both visible since resolving clears `blocked`).

**Composer** below, in blocked or normal state.

**Aside, mode (a) Work pane:** header with counts + Full view + collapse; a scrolling list of `RunCard`s for every non-done run, each with lanes, note, files and an action bar.

**Aside, mode (b) File panel:** back-to-work + artifact switcher + Preview/Diff/History tabs + Library link; the compact artifact renderers.

**Empty/edge states not drawn in the design but implied and required:**

- Empty transcript (new lane) — no design exists; use the composer placeholder as the only affordance.
- Aside fully collapsed — the chat header grows a `RunningNowPill`; that is the design's own re-entry path.
- Zero running runs — the rail's "Running now" section and the Work nav badge have no zero-state in the design; hide the badge when `activeCount === 0` and keep the section header with an empty list.

### 5.2 Work (`view === "work"`)

Two columns: run list + run detail.

- **List**: active/queued/paused runs as `WorkListRow`s, then a `Completed today` divider and compact `CompletedRow`s.
- **Detail**: title + meta row; live action group or terminal banner; then three `SectionCard`s — **Timeline** (wide `LaneBar`s with a 3-tick axis), **Output** (artifact rows), **Event log** (up to 6 `LogTag` rows for this run).
- Both the Timeline and Output cards have explicit empty states:
  - Timeline: "No steps have run yet. The timeline fills in once a writing_agent slot frees up."
  - Output: "Nothing produced yet. Files land here and in the Library as the run works."
  - Event log: "No events for this run yet."

### 5.3 Library (`view === "library"`)

Two columns: filtered artifact list + artifact detail.

- **List**: header count, kind filter chips, `LibraryRow`s.
- **Detail**: pinned head (32px badge, name, meta with run link, Pin/Export/Reveal, tab strip) over a scrolling body with the full-size renderer for Preview, plus Diff and History tabs.
- Seven artifact kinds are all designed: `md`, `code`, `plan`, `term`, `table`, `image`, `html`.

### 5.4 Settings (`view === "settings"`)

Left section nav (220px) + a 660px-max body. Eight sections, each with a label, a blurb, and one of three body shapes:

| Section           | Count | Body shape                                                                       | Add action        |
| ----------------- | ----- | -------------------------------------------------------------------------------- | ----------------- |
| **Connection**    | —     | Two `StatusCard`s (daemon status grid; today's spend/runs/tokens + progress bar) | —                 |
| **Models & keys** | 3     | `ListCard` with model chips per provider                                         | `Add provider`    |
| **Connectors**    | 4     | `ListCard`                                                                       | `Connect service` |
| **Skills**        | 6     | `ListCard`                                                                       | —                 |
| **Plugins**       | 2     | `ListCard`                                                                       | `Install plugin`  |
| **Agents**        | 6     | `ListCard`                                                                       | —                 |
| **Conversations** | 9     | `ListCard`                                                                       | —                 |
| **Event log**     | —     | `ListCard` of mono log rows, newest first                                        | —                 |

Blurbs (verbatim from the design — reuse them):

- Connection: "Daemon status, endpoint and today's spend against the cap."
- Models & keys: "Providers the router can reach, in priority order. Pick a model to make it the chat default."
- Connectors: "External services the agents may read and write."
- Skills: "Capabilities the agents can invoke, and whether each asks first."
- Plugins: "Loaded WASM plugins and what each contributes." _(the real system uses out-of-process JSON-RPC plugins, not WASM — correct this copy)_
- Agents: "Templates the orchestrator spawns from."
- Conversations: "Stored lanes. Memory compaction runs weekly."
- Event log: "Everything the daemon emitted, newest first."

### 5.5 Overlays

- **Command palette** — over any view.
- **Toast** — over any view, bottom-right.
- **Popovers** — model picker (chat only), artifact picker (file panel only).

### 5.6 Things shown only as a hint (no full design exists)

| Hint                                                  | Where                                                                                                                                                                                                                         |
| ----------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `⌘N`, `⌘⇧S`, `⌘2`, `⌘3`, `⌘⇧D`, `⌘,` shortcuts        | Palette rows only — **not bound**                                                                                                                                                                                             |
| `Export` / `Reveal`                                   | Library header buttons with no handler                                                                                                                                                                                        |
| `Reconnect` / `Copy log path`                         | Settings connection buttons with no handler                                                                                                                                                                                   |
| `Add provider` / `Connect service` / `Install plugin` | Only toast a placeholder                                                                                                                                                                                                      |
| `Manage providers & keys ↗`                           | Navigates, but the target section has no key-entry UI                                                                                                                                                                         |
| `+ N more in Library ↗`                               | Navigates to Library; no "filtered by run" view exists                                                                                                                                                                        |
| `now →` label on the parallel-work header             | Purely a directional hint                                                                                                                                                                                                     |
| Settings toggles                                      | Rendered from data; **no toggle handler exists** — they are display-only in the mock                                                                                                                                          |
| Search                                                | **There is no search input anywhere** except the palette's; the palette is the search                                                                                                                                         |
| Attachments / file upload in the composer             | **Absent from the design** — no paperclip, no drop zone. The daemon's `attachments_used` in the `done` payload implies it will be needed; design it in the established language (a ghost xs button left of the model button). |
| Notifications / errors                                | No error banner or connection-lost state is designed. Extrapolate from `#F7E7DE` / `#E9D3C6` / `#8C2F1E`.                                                                                                                     |

---

## 6. Assets

| Asset                                       | Referenced by the design?         | Action                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| ------------------------------------------- | --------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `static/fonts/JetBrainsMono-Variable.woff2` | **No**                            | Not used. Remove with the SvelteKit app or leave orphaned.                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `static/fonts/SpaceGrotesk-Variable.woff2`  | **No**                            | Same.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| IBM Plex Sans / IBM Plex Mono               | **Yes** — the only fonts          | Self-host (`@fontsource/ibm-plex-sans`, `@fontsource/ibm-plex-mono`, weights 400/500/600).                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| Raster images (`.png`, `.jpg`)              | **No**                            | The only `.png` occurrences are the literal string `screenshot-settings-drawer.png` used as sample artifact copy. No `<img>`, no `url(...)`, no `data:` URI, no `background-image` anywhere in the file.                                                                                                                                                                                                                                                                                                                                 |
| Icons                                       | Inline SVG only                   | Four icons total, all `viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"`, sized 15×15 (nav) / 14×14 (settings). Paths: chat bubble `M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z`; work = three rects (`x=3 y=4 w=18 h=4 rx=1`, `x=3 y=11 w=12 h=4 rx=1`, `x=3 y=18 w=15 h=3 rx=1`); library `M4 4h5l2 3h9v13H4z`; settings = `circle cx=12 cy=12 r=3` + `M12 2v3m0 14v3M2 12h3m14 0h3M5 5l2 2m10 10 2 2M19 5l-2 2M7 17l-2 2`. Ship these four inline; do not pull in an icon library for them. |
| Glyphs used as icons                        | `✓ ✗ ★ ☆ ‹ › ▴ ▾ ↗ ↵ ⏎ ⇧ ⌘ · — →` | Plain text characters. They render in IBM Plex; keep them as text, not SVG.                                                                                                                                                                                                                                                                                                                                                                                                                                                              |

There are **no uploaded or pasted images in this export.**

---

## 7. Backend wiring — what the design maps onto

Preserve the existing connection plumbing conceptually (see `apps/openalpaca-gui/src/lib/daemon.ts`, to be rewritten in React):

1. `invoke("ensure_daemon_running")` → `{ baseUrl, token, instanceId }` (Tauri command reading `discovery.json`).
2. WebSocket at `${baseUrl.replace("http","ws")}/v1/events?token=…`; reconnect with exponential backoff (base 1s, max 30s, ±20% jitter), reset on open. On reconnect, `invoke("get_connection_info")` and **re-bootstrap fully if `instanceId` changed**, otherwise just re-open the socket.
3. Bearer token from the same discovery payload on every HTTP call.

| UI element                                       | Backend source                                                                                                                                                                         |
| ------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ConnectionRow` dot + label + `7f3a`             | WS connection state; `instanceId.slice(0,4)`                                                                                                                                           |
| Assistant message body                           | `POST /v1/chat` → `{stream_id, lane_key}` → `GET /v1/chat/stream/{stream_id}?token=` SSE `delta {content}`                                                                             |
| `StreamingIndicator`                             | SSE `thinking`                                                                                                                                                                         |
| Assistant metadata line                          | SSE `done {model, duration_ms, tokens_in, tokens_out}`                                                                                                                                 |
| `RunReportCard` / delegation                     | SSE `done.delegation {task_id, title}`; `workflow_started`                                                                                                                             |
| `ToolConfirmationBanner` + blocked composer      | SSE `confirmation_requested {request_id, tool_name, tool_arguments}` / WS `tool_confirmation_requested`; answered via `POST /v1/chat/confirmations/{request_id}`                       |
| `ResolutionRow`                                  | local echo of the confirmation response + WS `tool_executed`                                                                                                                           |
| Run status / dots / counts                       | WS `task_status`, `workflow_started`, `workflow_progress`                                                                                                                              |
| Steer pill on a user message; `workflow_steered` | WS `workflow_steered`                                                                                                                                                                  |
| `follow-up →` composer mode                      | WS `followup_queued`                                                                                                                                                                   |
| Event log rows (`tag`)                           | `tool` ← `tool_executed`/`tool_confirmation_requested`; `spawn` ← `agent_status`; `artifact` ← artifact writes; `steer` ← `workflow_steered`; `run` ← `task_status`/`workflow_started` |
| Settings → Skills                                | `skill_catalog_updated`, `skill_invocation_started/completed/failed`                                                                                                                   |
| Settings → Plugins                               | `plugin_loaded/unloaded/crashed/disabled/pending_approval/needs_config`                                                                                                                |
| Settings → Models & keys                         | `llm_call_completed` (token/cost meters), `key_status_changed`                                                                                                                         |
| `spend` in the composer hint + Settings "Today"  | aggregate of `llm_call_completed.cost_usd`                                                                                                                                             |
| `heartbeat`                                      | connection liveness only; not surfaced                                                                                                                                                 |

---

## 8. Implementation notes and gaps

1. **Translate, do not transplant.** Every inline `style` string built in JS (`cardStyle`, `rowStyle`, `nav()`, `tagStyle`, `toggleStyle`, `badgeStyle`, `pinStyle`, lane bar styles) is a _variant table_. Model them as `cva`/`tv` variants or explicit prop unions — the tables in §3 give every branch.
2. **Fractional font sizes are load-bearing.** `9.5 / 10.5 / 11.5 / 12.5 / 13.5 / 14.5` px appear 150+ times. Do not round.
3. **Density affects only three things:** transcript max-width (720 → 780 — note it gets _wider_, which is what the source says), message gap (30 → 20), run-card padding (`14/15/12` → `11/13/10`, action bar `10/15` → `8/13`). Nothing else.
4. **The aside is one slot, two modes.** Opening an artifact hides the Work pane; `‹ Work` brings it back; `›` collapses the aside entirely. Never render both.
5. **`recentArtifacts` is computed but never rendered** — dead code in the export; ignore it.
6. **No dark mode.** The design is light-only and commits to a warm paper palette. If dark mode is later required it is a new design, not a token flip.
7. **No responsive design.** The artboard is a fixed 1440×900 desktop window. All panes flex, but no breakpoints exist. Minimum sensible window: rail 196 + min transcript ~500 + min aside 300 ≈ **1000px** wide.
8. **Accessibility gaps to close:** no focus-visible styles; only two `aria-label`s in the whole file (the two `›` buttons); the toggle is a `<button>` with no `role="switch"`/`aria-checked`; tabs have no `role="tab"`/`aria-selected`; the palette has no `role="dialog"`/focus trap (it does autofocus its input); status is conveyed by color+text (good) but the pulsing dot has no text equivalent. Add all of these.
9. **Copy is real and reusable.** Placeholders, empty states, hint rows and note strings in the design are well-written; reuse them verbatim except the "WASM plugins" blurb (§5.4), which is factually wrong for this codebase.
10. **`text-wrap: pretty`** is used on every long title and message body. Keep it.
