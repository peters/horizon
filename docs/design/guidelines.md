# Horizon Design Guidelines

This document describes how Horizon's interface looks and behaves today, in
both the dark and the light theme. It is a description of the current system,
not a redesign: every value below was read from the source (mainly
[`crates/horizon-ui/src/theme.rs`](../../crates/horizon-ui/src/theme.rs)) and
the widgets that consume it. When the code and this document disagree, the code
wins; fix the document in the same PR that changes the code.

Horizon is a terminal board built on egui. Its look is a calm, low-chrome
surface stack: a canvas, terminal panels and overlays that differ mostly by
lightness, one blue accent, and per-workspace accent colors used as tints.

- [Principles](#principles)
- [Themes](#themes)
- [Color tokens](#color-tokens)
- [Derived colors (blend recipes)](#derived-colors-blend-recipes)
- [Contrast](#contrast)
- [Typography](#typography)
- [Shape, spacing and elevation](#shape-spacing-and-elevation)
- [Components](#components)
- [Terminal](#terminal)
- [States, focus and keyboard](#states-focus-and-keyboard)
- [Motion and repaint rules](#motion-and-repaint-rules)
- [Do and don't](#do-and-dont)
- [Reproducing the numbers](#reproducing-the-numbers)

## Principles

1. **Tokens, not literals.** Widgets take colors from `theme::*()` accessors
   and derive tints with `theme::blend` / `theme::alpha`. The accessors read
   the active theme, so a widget written against tokens is correct in dark and
   light without a branch.
2. **Depth by lightness, emphasis by accent.** Surfaces are separated by small
   steps in lightness and a 1 px border. Selection, focus and primary actions
   use `ACCENT` (or a workspace accent), never a new hue.
3. **Both themes are first-class.** Light mode is warm paper, not an inverted
   dark mode. Its state colors and terminal palette are deliberately deeper so
   they keep their contrast on light surfaces.
4. **The terminal is the content.** Chrome stays quiet, small and dim so that
   terminal output carries the visual weight.
5. **Idle is free.** Passive polish must not cause repaints. See
   [Motion and repaint rules](#motion-and-repaint-rules).

## Themes

The `appearance.theme` setting is one of `auto` (default), `dark` or `light`.
`auto` follows the operating system theme and falls back to dark when the
system reports none. `theme::apply` installs a complete egui style for both
themes on every apply, resolves the preference, and stores the result in a
process-wide atomic that the token accessors read. Code that must differ per
theme (rare) asks `theme::current_theme()` and matches on `ResolvedTheme`.

Character of each theme:

| | Dark | Light |
|---|---|---|
| Feel | Deep navy ink, cool | Warm off-white paper |
| Canvas vs panel | Canvas (`BG`) is the darkest layer; panels and raised fills are progressively lighter | Panels are the lightest (near white); the canvas is a warmer, slightly darker paper tone |
| Accent | Bright periwinkle `#6A90FF` | Deeper royal blue `#3A56D4` |
| State colors | Light pastels | Deep, saturated |
| Window shadow | Alpha 128, offset (0, 10), blur 28, spread 2 | Alpha 36, same geometry |
| Popup shadow | Alpha 118, offset (0, 6), blur 22 | Alpha 24, same geometry |

Surface lightness order, darkest to lightest:

- Dark: `BG`, `TITLEBAR_BG`, toolbar fill, `BG_ELEVATED`, `PANEL_BG`,
  `PANEL_BG_ALT`.
- Light: `PANEL_BG_ALT`, `BG`, `TITLEBAR_BG` / toolbar fill, `BG_ELEVATED`,
  `PANEL_BG`.

So `PANEL_BG_ALT` is the lightest surface in dark mode and the darkest in light
mode: it always reads as a raised control sitting on a panel, which is why it is
the base for button, tab and chip fills. Treat the token names as roles, not as
a lightness ranking.

## Color tokens

Values are the exact `Color32::from_rgb` constants in `theme.rs`.

### Surfaces

| Token | Dark | Light | Role |
|---|---|---|---|
| `BG` | `#070A10` | `#F3F0EA` | Canvas background, text-input well (`extreme_bg_color`), text color on solid accent fills |
| `BG_ELEVATED` | `#0C1018` | `#FAF8F3` | Modal and side-panel fill, overlay input well, code background, noninteractive widget fill |
| `PANEL_BG` | `#10141E` | `#FCFBF7` | Terminal panel body, window fill, overlay cards (command palette, pickers), settings section cards |
| `PANEL_BG_ALT` | `#161B27` | `#F0ECE4` | Raised control fill (buttons, tabs, chips), titlebar base, faint/striped background, base for accent blends |
| `TITLEBAR_BG` | `#080B11` | `#F6F3EC` | Root toolbar strip |
| toolbar fill (no accessor) | `#0B0F16` | `#F6F3EC` | egui `panel_fill` for built-in panels |

### Text

| Token | Dark | Light | Role |
|---|---|---|---|
| `FG` | `#E4E9F4` | `#1A1E26` | Primary text, titles, focus outlines, selected text |
| `FG_SOFT` | `#AEB9CD` | `#505964` | Secondary text, unselected titles, control labels |
| `FG_DIM` | `#707D96` | `#7C8693` | Tertiary text: captions, hints, placeholders, disabled-looking chrome |
| `ACCENT` (as text) | `#6A90FF` | `#3A56D4` | Links (`hyperlink_color`), emphasized inline actions |

### Accent, borders and canvas decoration

| Token | Dark | Light | Role |
|---|---|---|---|
| `ACCENT` | `#6A90FF` | `#3A56D4` | Primary action fill, selection, focused-panel fallback, spinner, links |
| `CURSOR` | `#B2C8FF` | `#405CD2` | Terminal cursor (drawn at 0.8-0.9 opacity) |
| `BORDER_SUBTLE` | `#273042` | `#DBD4C7` | Default 1 px outlines: cards, inputs, dividers |
| `BORDER_STRONG` | `#445470` | `#B2A893` | Modal outline, strong control outline, base for accent-tinted borders |
| `GRID_DOT` | `#1D2330` | `#D6CFC1` | Canvas dot grid |
| `CANVAS_COOL_GLOW` | rgba(92, 122, 235, 22) | rgba(94, 106, 210, 10) | Canvas ambient glow, cool |
| `CANVAS_WARM_GLOW` | rgba(255, 152, 86, 24) | rgba(233, 168, 106, 12) | Canvas ambient glow, warm |

### State colors

| Token | Dark | Light | Role |
|---|---|---|---|
| `PALETTE_GREEN` | `#A6E3A1` | `#128654` | Success, in stock, done, healthy frame rate |
| `PALETTE_YELLOW` | `#E9BE6D` | `#A35C00` | Warning, low stock, caution banners |
| `PALETTE_RED` | `#F38BA8` | `#C71636` | Error, out of stock, needs input, destructive text |
| `PALETTE_CYAN` | `#66D4D6` | `#007492` | Informational highlight |
| `BTN_CLOSE` | `#EB6058` | `#CF3742` | Close control hover, destructive confirm fill |

State colors are used three ways: as text or a dot (always beside words or a
shape, see [Do and don't](#do-and-dont)), as a translucent fill plus outline
(`alpha(color, 24-34)` fill, `alpha(color, 90)` 1 px outline), and as a blend
tint on a neutral fill.

### Workspace accents

Each workspace has one of eight accent colors. Dark mode uses the pastel set
defined in `horizon-core` (`WORKSPACE_COLORS`); light mode uses a deeper set in
`theme.rs` (`WORKSPACE_ACCENTS_LIGHT`) so blends into paper-white fills keep
contrast. Use `theme::workspace_accent(idx)` and never pick from either table
directly.

| Index / name | Dark | Light |
|---|---|---|
| 0 blue | `#89B4FA` | `#316CEB` |
| 1 green | `#A6E3A1` | `#188F58` |
| 2 yellow | `#F9E2AF` | `#BC8000` |
| 3 red | `#F38BA8` | `#D62454` |
| 4 pink | `#F5C2E7` | `#C957A0` |
| 5 teal | `#94E2D5` | `#0092A8` |
| 6 mauve | `#CBA6F7` | `#8B58E2` |
| 7 peach | `#FAB387` | `#DB6F28` |

The dark accents and the dark `PALETTE_GREEN` and `PALETTE_RED` are Catppuccin
Mocha pastels. The surface, text, border and accent tokens are Horizon's own
navy and paper palettes, not Catppuccin.

### Terminal palette (ANSI 0-15)

| Index | Name | Dark | Light |
|---|---|---|---|
| 0 | black | `#2D313E` | `#232630` |
| 1 | red | `#E36B75` | `#D20F39` |
| 2 | green | `#8FD582` | `#15803D` |
| 3 | yellow | `#E9BE6D` | `#925E00` |
| 4 | blue | `#74A2F7` | `#1B60E8` |
| 5 | magenta | `#CA97EA` | `#8334E3` |
| 6 | cyan | `#66D4D6` | `#007A9E` |
| 7 | white | `#C4CCDB` | `#373C4E` |
| 8 | bright black | `#4A5061` | `#686E82` |
| 9 | bright red | `#F28287` | `#E71A44` |
| 10 | bright green | `#AAE09E` | `#0D6F35` |
| 11 | bright yellow | `#F4CF85` | `#B26500` |
| 12 | bright blue | `#93BBFF` | `#144AD1` |
| 13 | bright magenta | `#E0B2F7` | `#982AE8` |
| 14 | bright cyan | `#8DE1E3` | `#006885` |
| 15 | bright white | `#E7ECF5` | `#11141C` |

In light mode "white" is a dark gray and "bright white" is near black: the
palette names describe intent (normal versus emphasized), not literal color, so
that text the program marks as emphasized reads as stronger on paper.

Indices 16-255 use the standard xterm 6x6x6 cube and grayscale ramp (or the
color the application sets through OSC). Default terminal foreground is `FG`
and default background is `PANEL_BG`.

## Derived colors (blend recipes)

Most interactive colors are not tokens but a `theme::blend(base, tint, amount)`
of a neutral and an accent, sometimes wrapped in `theme::alpha(color, a)`.
`blend` mixes linearly per sRGB channel with the amount clamped to 0-1. The
resulting hex values below are computed from the token values above.

| Use | Recipe | Dark | Light |
|---|---|---|---|
| Widget hover fill (egui `hovered.bg_fill`) | `blend(PANEL_BG_ALT, ACCENT, 0.16)` | `#232E4A` | `#D3D4E1` |
| Widget pressed fill (egui `active.bg_fill`) | `blend(PANEL_BG_ALT, ACCENT, 0.22)` | `#283557` | `#C8CBE0` |
| Text selection fill (egui) | `alpha(ACCENT, 54)` with 1 px `ACCENT` stroke | | |
| Selected tab fill | `blend(PANEL_BG_ALT, ACCENT, 0.20)` | `#273252` | `#CCCEE1` |
| Selected tab outline | `blend(BORDER_SUBTLE, ACCENT, 0.5)` | `#4960A1` | `#8B95CE` |
| Selected worker card fill | `blend(PANEL_BG_ALT, ACCENT, 0.14)` | `#222B45` | `#D7D7E2` |
| Selected chip fill | `blend(PANEL_BG_ALT, ACCENT, 0.22)` | `#283557` | `#C8CBE0` |
| Chip hover fill | `blend(PANEL_BG_ALT, FG, 0.06)` | `#222733` | `#E3E0D9` |
| Primary button fill (toolbar) | `blend(PANEL_BG_ALT, ACCENT, 0.28)` | `#2E3C63` | `#BDC2E0` |
| Primary button outline | `blend(BORDER_STRONG, ACCENT, 0.72)` | `#5F7FD7` | `#5C6DC2` |
| Selected list row (palette, pickers, search results) | `alpha(blend(PANEL_BG_ALT, ACCENT, 0.28), 200)` | | |
| Active search toggle (`Aa`, `.*`) | `blend(PANEL_BG_ALT, ACCENT, 0.35)` | `#334473` | `#B0B8DE` |
| Hovered list row | `alpha(PANEL_BG_ALT, 160)` | | |
| Danger button fill | `blend(PANEL_BG_ALT, PALETTE_RED, 0.22)` | `#473443` | `#E7BDBE` |
| Danger button outline | `blend(BORDER_STRONG, PALETTE_RED, 0.68)` | `#BB7996` | `#C04554` |
| Focused panel border | `blend(BORDER_STRONG, panel accent, 0.78)` (accent shown for `ACCENT`) | `#6283E0` | `#5468C6` |
| Unfocused panel border | `alpha(blend(BORDER_SUBTLE, panel accent, 0.32), 196)` | | |
| Terminal selection | `alpha(ACCENT, 76)` over `PANEL_BG`, text `FG` | | |
| Checked checkbox fill (dialog) | solid `ACCENT`, `BG` check mark | `#6A90FF` | `#3A56D4` |

Accent-tint amounts follow a ladder: about 0.05-0.10 for a resting tint
(workspace label, panel titlebar unfocused at 0.10), 0.12-0.22 for a hover or
active tint, 0.28-0.35 for a selected row or primary button. New tints should
sit on this ladder.

## Contrast

Ratios are WCAG 2.x relative-luminance contrast, computed from the hex values
in this document (see [Reproducing the numbers](#reproducing-the-numbers)).
Thresholds: 4.5:1 for normal text, 3:1 for large text (about 24 px, or 18.7 px
bold) and for non-text UI components such as control outlines and focus
indicators. A ratio below 4.5 is marked with `*`.

### Text tokens on surfaces

**Dark**

| Foreground | `BG` | `BG_ELEVATED` | `PANEL_BG` | `PANEL_BG_ALT` | `TITLEBAR_BG` |
|---|---|---|---|---|---|
| `FG` | 16.28 | 15.65 | 15.13 | 14.15 | 16.19 |
| `FG_SOFT` | 10.02 | 9.63 | 9.31 | 8.70 | 9.96 |
| `FG_DIM` | 4.78 | 4.59 | 4.44 `*` | 4.15 `*` | 4.75 |
| `ACCENT` | 6.65 | 6.39 | 6.18 | 5.77 | 6.61 |
| `PALETTE_GREEN` | 13.33 | 12.81 | 12.38 | 11.58 | 13.25 |
| `PALETTE_YELLOW` | 11.37 | 10.93 | 10.56 | 9.88 | 11.30 |
| `PALETTE_RED` | 8.56 | 8.22 | 7.95 | 7.43 | 8.51 |
| `PALETTE_CYAN` | 11.28 | 10.84 | 10.48 | 9.80 | 11.21 |
| `BTN_CLOSE` | 5.99 | 5.75 | 5.56 | 5.20 | 5.95 |
| `CURSOR` | 11.89 | 11.42 | 11.04 | 10.33 | 11.82 |

**Light**

| Foreground | `BG` | `BG_ELEVATED` | `PANEL_BG` | `PANEL_BG_ALT` | `TITLEBAR_BG` |
|---|---|---|---|---|---|
| `FG` | 14.68 | 15.73 | 16.13 | 14.17 | 15.07 |
| `FG_SOFT` | 6.25 | 6.70 | 6.87 | 6.03 | 6.41 |
| `FG_DIM` | 3.25 `*` | 3.48 `*` | 3.57 `*` | 3.13 `*` | 3.33 `*` |
| `ACCENT` | 5.33 | 5.72 | 5.86 | 5.15 | 5.47 |
| `PALETTE_GREEN` | 4.05 `*` | 4.34 `*` | 4.44 `*` | 3.91 `*` | 4.15 `*` |
| `PALETTE_YELLOW` | 4.52 | 4.84 | 4.96 | 4.36 `*` | 4.64 |
| `PALETTE_RED` | 5.13 | 5.50 | 5.64 | 4.96 | 5.27 |
| `PALETTE_CYAN` | 4.72 | 5.06 | 5.18 | 4.55 | 4.84 |
| `BTN_CLOSE` | 4.32 `*` | 4.62 | 4.74 | 4.17 `*` | 4.43 `*` |
| `CURSOR` | 5.02 | 5.38 | 5.51 | 4.84 | 5.15 |

What this means:

- `FG` and `FG_SOFT` pass 4.5:1 on every surface in both themes. Use `FG_SOFT`,
  not `FG_DIM`, for any secondary text a person needs to read to finish a task.
- `FG_DIM` is a de-emphasis tier, not a readable-text tier. It reaches 4.5:1
  only on the darkest dark surfaces and never in light mode (3.1 to 3.6). Keep
  it to captions, hints, placeholders and decorative labels whose loss would not
  block a task, and pair it with a clearer control (an icon, an outline) when
  the item is interactive. Do not use it for a value, an error or a required
  instruction.
- In light mode the state colors sit between 3.9 and 5.6 on common surfaces.
  Green is the weakest (3.91 to 4.44): always write the state in words
  ("In stock", "Done") and treat the color as reinforcement, never the sole
  carrier. Yellow, red and cyan meet 4.5 on every light surface except the
  yellow on `PANEL_BG_ALT` marked above; `BTN_CLOSE` is close to the line.
- Dark-mode state colors are all well above 4.5 and can be used as text
  freely; the text-versus-fill pairs in the next table cover tinted fills.

### Component pairs

| Pair | Dark | Light |
|---|---|---|
| `FG` on primary button fill (`blend .28`) | 8.88 | 9.51 |
| `BG` on solid `ACCENT` (dialog "Start" button label, checkbox mark) | 6.65 | 5.33 |
| `FG` on solid `ACCENT` (do not use) | 2.45 `*` | 2.75 `*` |
| `FG` on danger button fill | 9.38 | 9.88 |
| `FG` on selected tab fill (`blend .20`) | 10.37 | 10.72 |
| `FG_DIM` unselected tab label on `BG_ELEVATED` | 4.59 | 3.48 `*` |
| `FG` on selected list row (`blend .28`) | 8.88 | 9.51 |
| `FG_SOFT` on selected list row (`blend .28`) | 5.47 | 4.05 `*` |
| `FG` on active search toggle (`blend .35`) | 7.79 | 8.55 |
| `FG` / `FG_SOFT` / `FG_DIM` on selected worker card | 11.51 / 7.08 / 3.38 `*` | 11.69 / 4.98 / 2.59 `*` |
| `FG_SOFT` on hovered widget fill (`blend .16`) | 6.80 | 4.83 |
| `ACCENT` text on `+ Add` button fill (`blend .08`) | 5.17 | 4.61 |
| `FG` on terminal selection (`alpha(ACCENT, 76)` over `PANEL_BG`) | 9.28 | 10.31 |

Rules that follow:

- On a solid `ACCENT` fill, the label is `BG` (dark ink in dark mode, paper in
  light mode). `FG` fails there in both themes.
- On accent-tinted selected states, use `FG` for the selected label. In light
  mode `FG_SOFT` drops to 4.05 on a selected row and `FG_DIM` to 2.59 on a
  selected card, so promote text to a stronger token when a row becomes
  selected (the command palette already does this).
- An unselected tab label in light mode (`FG_DIM`, 3.48) is below 4.5: the tab
  bar relies on size, position and the selected tab's fill and outline. Do not
  copy that pattern to labels that carry unique information.

### Non-text contrast

| Element | Dark on `PANEL_BG` | Light on `PANEL_BG` |
|---|---|---|
| `BORDER_SUBTLE` | 1.39 | 1.42 |
| `BORDER_STRONG` | 2.41 | 2.27 |
| `GRID_DOT` on `BG` | 1.26 | 1.36 |
| `ACCENT` outline / focus | 6.18 | 5.86 |
| `FG` focus ring | 15.13 | 16.13 |
| Primary button outline on `BG_ELEVATED` | 4.99 | 4.47 |

`BORDER_SUBTLE` and `BORDER_STRONG` are separators and grouping cues, below the
3:1 needed to identify a control on their own. Controls are therefore
identified by fill, label and icon as well, and the states that must not be
missed (selected, focused, invalid) use `ACCENT`, `FG` or a state color, all
above 3:1 on every surface. Do not make a state visible through a border
between two neutrals alone.

### Terminal palette

Each palette color against `PANEL_BG` (default terminal background):

| Index | Dark | Light |
|---|---|---|
| 0 black | 1.42 | 14.58 |
| 1 red | 5.81 | 5.24 |
| 2 green | 10.53 | 4.84 |
| 3 yellow | 10.56 | 5.30 |
| 4 blue | 7.23 | 5.22 |
| 5 magenta | 8.00 | 5.69 |
| 6 cyan | 10.48 | 4.74 |
| 7 white | 11.40 | 10.57 |
| 8 bright black | 2.29 | 4.90 |
| 9 bright red | 7.29 | 4.38 |
| 10 bright green | 12.15 | 6.07 |
| 11 bright yellow | 12.37 | 4.27 |
| 12 bright blue | 9.46 | 6.92 |
| 13 bright magenta | 10.41 | 5.29 |
| 14 bright cyan | 12.28 | 6.11 |
| 15 bright white | 15.53 | 17.78 |
| default `FG` | 15.13 | 16.13 |

Two mechanisms keep terminal text legible:

- The light palette is held to at least 4.2:1 against the light `PANEL_BG` by a
  unit test in `theme.rs`.
- At render time `ensure_terminal_text_contrast` lifts any foreground that has
  less than 3.6:1 against the actual cell background toward near-white (on a
  dark cell) or near-black (on a light cell). This is why the dark palette's
  black (1.42) and bright black (2.29) are acceptable: they are intended as
  background and dim-text colors, and the floor rescues them when a program
  prints them as foreground.

Workspace accents against `PANEL_BG`: dark 7.95 to 14.49 (all readable as
text); light 3.21 to 4.78 (blue 4.53, red 4.78 and mauve 4.42 come close to
text contrast, while green 3.97, yellow 3.26, pink 3.79, teal 3.57 and peach
3.21 are graphic-object colors). In light mode use workspace accents for dots,
bars, outlines and tints, and put label text in `FG` or `FG_SOFT`.

## Typography

### Families

| Family | Font | Fallbacks |
|---|---|---|
| Proportional (UI) | Inter (variable, bundled) | Noto Sans CJK SC, Noto Sans Symbols 2 |
| Monospace (terminal, code, keys, numbers) | JetBrains Mono Regular (bundled) | Noto Sans CJK SC, Noto Sans Symbols 2 |

JetBrains Mono is the metrics source for the terminal grid; the CJK and symbol
fonts only fill glyphs it lacks. Do not add another UI family.

### Scale

Horizon does not override egui's named text styles (defaults for egui 0.36:
Body and Button 13, Heading 18, Small 9, Monospace 13). Widgets that need a
specific size set it explicitly with `RichText::size` or `FontId`, so the
sizes actually used form this scale. Size is in logical points; weight is
egui's normal or `.strong()` (a bolder rendering of the same family).

| Size | Weight | Where it is used | Color |
|---|---|---|---|
| 28 | strong | Startup session screen product name | `FG` |
| 26 | strong | Dialog and page titles ("New cloud", cloud name), loading-screen product name | `FG` |
| 20-23 | strong | Summary and detail titles inside a dialog or device details | `FG` |
| 18 | strong | Window and panel titles ("Settings", "Sessions"), large stat values | `FG` |
| 16 | strong or normal | Section titles inside a dialog, sub-section labels | `FG` / `FG_SOFT` |
| 15 | strong or normal | Card titles, input text, dialog sub-headings, overlay input | `FG` |
| 14 | strong or normal | Field labels (strong), dialog buttons, body copy in dialogs, brand in toolbar | `FG` / `FG_SOFT` |
| 13 - 13.5 | normal or strong | Settings section headings (13 strong, `FG_SOFT`), palette rows, help lines, checkbox labels | `FG` / `FG_SOFT` |
| 12 - 12.5 | normal | Dense body: settings rows, secondary lines, chips, detail lines | `FG_SOFT` |
| 11 - 11.5 | normal | Chrome buttons, hints under fields, badges | `FG_SOFT` / `FG_DIM` |
| 10 - 10.5 | strong for captions | Upper-case section captions ("STORAGE"), key hints, palette section headers | `FG_DIM` |
| 8.5 - 9 | normal or strong | Micro labels: fps unit | state color / `FG_DIM` |

Terminal and code fonts:

| Surface | Family and size |
|---|---|
| Terminal grid | JetBrains Mono 13, line height 1.3 x font size |
| Text editor panel | JetBrains Mono 14 |
| Git changes / diff | JetBrains Mono 11 for file rows, summary and diff lines; 10 and 9.5 for meta |
| Panel titlebar title | Inter 13; session badge JetBrains Mono 10.5 |
| Key hints in overlays | JetBrains Mono 10 in a `BG_ELEVATED` chip |
| Toolbar frame-rate readout | JetBrains Mono 11.5 |

Rules of thumb:

- Titles are `FG`, strong. Secondary lines under a title are `FG_SOFT`.
- Field labels sit above their field at 14 strong, `FG`; hints under a field at
  11.5-12.5 in `FG_DIM` or `FG_SOFT`.
- Upper-case captions are 10.5 strong `FG_DIM` and only for short section names.
- Use monospace for anything the person may copy or compare: paths, commands,
  hashes, numeric readouts. Use proportional for everything else.
- Long single-line text truncates with an ellipsis (`Label::truncate` or the
  `single_line_label_job` helper) rather than wrapping inside chrome.

## Shape, spacing and elevation

### Corner radii

| Radius | Used for |
|---|---|
| 22 / 20 | Overlay cards (command palette, directory picker; 22 is the outer glow ring), workspace frames on the canvas, session manager |
| 18 | Focus ring around a focused panel |
| 16 | Terminal panel body and titlebar, large form dialogs' modal frame, egui windows, file-drop highlight on a panel |
| 14 | Search dropdown (15 for its outer ring), upload dialog, session manager card |
| 12 | Default widget radius in the egui style, overlay input wells, summary cards |
| 10 | Buttons (primary, chrome, danger, 40 px dialog buttons), section cards, selector cards, sidebar rows, workspace labels, pills, toolbar search input shell |
| 8 | Tabs, chips, toggle buttons, status frames, palette rows, preset cards |
| 6 | Search result rows |
| 4 | Key-hint chips, checkbox box |
| 1-2 | Thin indicator bars, cursor |

Nested shapes step down: an outer 16 modal holds 10 cards, which hold 8 chips.

### Spacing

| Value | Used for |
|---|---|
| 8 x 8 | Global egui `item_spacing` |
| 12 x 6 | Global egui `button_padding` |
| 10 x 8 | `item_spacing` inside dialogs |
| 24 | Large form dialogs' `inner_margin`; dialog column gutter; settings panel horizontal margin |
| 16 | Vertical gap after a dialog heading or settings heading; settings section-card margin; settings panel vertical margin |
| 12 | Gap between a scroll body and its action bar; gap after a settings card |
| 8 | Gap between form rows in a dialog |
| 6 | Sidebar row horizontal inset; gap between preset cards |
| 4 | Gap between tabs in the tab bar |

Fixed metrics:

| Metric | Value |
|---|---|
| Root toolbar height | 46 |
| Toolbar buttons | 30 high, 8 gap, 14 horizontal padding |
| Sidebar width | 210 default, 168 minimum |
| Panel titlebar height | 34 |
| Panel padding | 8 |
| Panel resize handle | 32 screen points, independent of canvas zoom (bounded by panel extent). The painted mark is six dots in the corner. Those dots keep the same screen size when the corner can hold them. The square is the hit target and has no fill. |
| Canvas dot grid | 22 spacing and 2.3 dot diameter at 100% zoom. Both scale with zoom; the spacing doubles until it is at least 14 on screen (so zooming out shows a coarser grid, not none) and the dot diameter is clamped to 1-5 |
| Text field in a dialog | 38 high, text margin (12, 10) |
| Dialog buttons | creation dialog: at least 120 x 40; accounts dialog: the primary ("Save settings", or "Save and start" while continuing a first cloud) 148 x 40, `Cancel` at least 80 x 40 |
| Overlay text input | 44 high |
| Command palette | 500 wide, 36 row, 28 section header, at most 12 visible rows |
| Search dropdown | 600 wide, 36 high toolbar input, 32 row, 24 section header, at most 12 visible rows |
| Directory picker | 520 wide, 34 row, at most 460 tall |
| Dialog width | viewport width minus 64, clamped to 240-1180; two columns from 800 (summary column 330, gutter 24) |

### Strokes

| Width | Used for |
|---|---|
| 0.5 | Row separators in overlays (`BORDER_SUBTLE` alpha 180) |
| 1.0 | Default: card, input and button outlines, dividers, toolbar bottom edge |
| 1.2 / 1.8 | Panel border, unfocused / focused |
| 1.5 | Overlay card outline (`ACCENT` alpha 80), checkbox edge, focus ring on a checkbox |
| 2.0 | Selected card outline (`ACCENT`), keyboard-focused card (`FG`), focus glow ring on an overlay (alpha 25), drop indicator |
| 2.5 | File-drop highlight outline |
| 3.0 | Focused-panel glow (accent alpha 56, drawn outside the panel) |

Strokes are drawn inside the shape for controls (`StrokeKind::Inside`) and
outside for large frames and glows.

### Elevation

There are four levels, from back to front:

1. **Canvas**: `BG`, two low-alpha radial glows and the dot grid.
2. **Workspace frame and panels**: workspace frame at radius 20 with a faint
   accent tint (`alpha(blend(PANEL_BG, accent, 0.12), 24)` active or 14 idle)
   and a 1 px accent outline (alpha 110 active, 55 idle); panels at radius 16
   on `PANEL_BG`.
3. **Chrome**: toolbar and sidebar, drawn over the canvas.
4. **Overlays and modals**: dimmed backdrop (`black` alpha 140 for pickers),
   then the card. Overlay cards use `PANEL_BG` with a 1.5 px `ACCENT` alpha-80
   outline and a soft 2 px alpha-25 outer ring; large form dialogs use
   `BG_ELEVATED` with a 1 px `BORDER_STRONG` outline. Shadows come from the egui style
   ([Themes](#themes)).

## Components

### Modal dialog

Two form dialogs share one frame: the cloud creation dialog (up to 1180 wide)
and the cloud accounts dialog (up to 1060 wide). Small confirmations (such as
closing a cloud) and the browser file chooser keep egui's default modal frame
from the theme instead: window fill `PANEL_BG`, 1 px `BORDER_SUBTLE` outline and
the popup shadow, without the 24 px margin below.

Shared by both form dialogs:

- Frame: fill `BG_ELEVATED`, stroke 1 px `BORDER_STRONG`, corner radius 16,
  inner margin 24. Raise it above the toolbar with `Order::Tooltip`.
- Heading at 26 strong, then a one-line description in `FG_SOFT`, then 16 of
  space.
- Body scrolls; the action bar stays pinned below it.
- Escape and a click on the backdrop dismiss.
- Footer: right-aligned primary and `Cancel`, 40 high, primary radius 10.

Cloud creation dialog specifics:

- `item_spacing` (10, 8); the heading has a `PANEL_BG_ALT` info chip on the
  right (radius 8, margin 12 x 6, 13 `FG_SOFT`); the description is 14.
- 12 of space between the scroll body and the action bar. Keep the dialog height
  steady as choices change so it does not jump.
- Footer buttons are at least 120 x 40 with a 14 label; the primary is a solid
  `ACCENT` fill. A short reason or hint (13 `FG` or 11.5 `FG_DIM`) sits to the
  left when the primary is disabled.
- Enter in the first text field submits when the form is valid.
- While the repository picker is open above it, the dialog is disabled and the
  picker owns Escape and outside clicks.

Cloud accounts dialog specifics:

- One page, no tabs. Heading 24 strong, a 13 `FG_SOFT` description, then a pinned
  readiness banner above the scrolling cards. The banner is an `alpha(state, 24)`
  fill with an `alpha(state, 90)` outline, radius 12: yellow while something is
  left to do and the first thing is named, green ("Settings complete") when keys are
  saved, the SSH identity is ready and every image repository has a saved pull-access
  validation. Provider accounts are still checked when a cloud starts.
- Cards (provider, agents, container registry, workspace) are `PANEL_BG`, 1 px
  `BORDER_SUBTLE`, radius 12, margin 16, 16 apart, in two columns from 760 wide and
  stacked below. A card has a 15 strong title, a 12 `FG_DIM` note and a status at the
  right: a dot plus a word (`PALETTE_GREEN` saved or verified, `PALETTE_YELLOW` needs
  something, `FG_DIM` off or optional). Color never stands alone.
- A saved key is a masked well (`BG`, radius 8) with Replace; a key is never shown
  back. Fields are 36 high with a 12 `FG_SOFT` label above.
- A separator sits above the footer. The primary ("Save settings", or
  "Save and start" while continuing a first cloud) is 148 x 40 with a
  `blend(PANEL_BG_ALT, ACCENT, 0.35)` fill and the default label; `Cancel` is
  at least 80 x 40.
- Dismissal is ignored while a save is running.

### Overlay card (command palette and pickers)

- Card: `PANEL_BG`, radius 20, 1.5 px `ACCENT` alpha 80 outline, plus a 2 px
  alpha 25 ring at radius 22. Placed a quarter of the way down the window,
  centered horizontally.
- Heading (pickers): 15 strong `FG`, then 10 of space.
- Input: `BG_ELEVATED` well, radius 12, 44 high, 1 px `ACCENT` alpha 70 outline,
  typed text 14 (proportional in the command palette, monospace in the
  pickers), 13 `FG_DIM` hint, no frame on the inner text edit.
- Rows: 34-36 high, radius 8. Selected row `alpha(blend(PANEL_BG_ALT, ACCENT, 0.28), 200)`
  with `FG` label; other rows `FG_SOFT`, with an `alpha(PANEL_BG_ALT, 160)` hover
  fill. Label 13, detail 11 `FG_DIM`. A 4.5 px dot in the workspace accent may
  lead a row.
- Section headers 10.5 `FG_DIM`. Key hints at the bottom are 10 monospace in
  `BG_ELEVATED` chips (radius 4, margin 5 x 2, 1 px `BORDER_SUBTLE` alpha 160)
  followed by a 10.5 `FG_DIM` description.

### Search overlay

Toolbar search is a separate pattern, not the overlay card above. The input
lives inline in the toolbar (36 high, monospace 13 text, radius 10 shell with
a radius 9 `BG_ELEVATED` core, a 3 px accent glow ring and a 22 px icon badge at
the left; the border blends toward `ACCENT` by 0.32 idle, 0.5 hovered and 0.78
focused). Its results open in a dropdown right-aligned below it, with no
backdrop dimming:

- Frame: `PANEL_BG`, radius 14, 1 px `ACCENT` alpha 60 outline, plus a 1.5 px
  alpha 18 ring at radius 15. 600 wide, at most 12 rows, inner padding 12 x 10.
- Header: `Aa` and `.*` toggles (22 high, radius 5, 10 label; active fill
  `blend(PANEL_BG_ALT, ACCENT, 0.35)` with `FG`, inactive `BG_ELEVATED` with
  `FG_DIM`), then a 10 `FG_DIM` status line.
- Rows 32 high, radius 6, with a 24 high section header in 9.5 monospace
  `FG_DIM`; panel title 12 proportional (`ACCENT` when selected, `FG_SOFT`
  otherwise), matching line 10.5 monospace `FG_DIM`, and a 9.5 monospace count
  badge (radius 4, 0.5 px `BORDER_SUBTLE` alpha 180 outline). The selected row
  fill is `alpha(blend(PANEL_BG_ALT, ACCENT, 0.28), 200)` and the hover fill is
  `alpha(PANEL_BG_ALT, 160)`, as in the overlay card.

### Text field

- In dialogs: 38 high, full width, text 15, margin (12, 10), hint text in
  `FG_DIM`, label above at 14 strong `FG`. The fill is egui's text-edit
  background (`BG`), so a field reads as a well in the surface behind it.
- A field that opens a picker looks the same but is a button with a right-side
  "Browse..." label (13 `FG_SOFT`); an empty value shows the placeholder in
  `FG_DIM`.
- Overlay inputs (above) draw their own well and disable the built-in frame.

### Buttons

| Kind | Fill | Outline | Label | Radius | Where |
|---|---|---|---|---|---|
| Primary (emphasized chrome) | `blend(PANEL_BG_ALT, ACCENT, 0.28)` | 1 px `blend(BORDER_STRONG, ACCENT, 0.72)` | `FG` 11.5 | 10 | Toolbar "update available" |
| Primary (creation dialog) | solid `ACCENT` | none | `BG` 14 strong | 10 | Dialog confirm ("Start cloud"), min 120 x 40 |
| Primary (accounts dialog) | `blend(PANEL_BG_ALT, ACCENT, 0.35)` (`#334473` dark, `#B0B8DE` light) | egui default | `FG` (default) | 10 | Accounts dialog save ("Save settings" / "Save and start"), 148 x 40; `FG` contrast 7.79 dark, 8.55 light |
| Secondary / chrome | `PANEL_BG_ALT` | 1 px `alpha(BORDER_SUBTLE, 210)` | `FG_SOFT` 11 | 10 | Toolbar and sidebar actions, 30 high |
| Secondary (creation dialog) | egui default widget fill | egui default | 14 | 10 | "Cancel", min 120 x 40 (the accounts dialog's `Cancel` is min 80 x 40 with egui's default radius) |
| Danger | `blend(PANEL_BG_ALT, PALETTE_RED, 0.22)` | 1 px `blend(BORDER_STRONG, PALETTE_RED, 0.68)` | `FG` 11 | 10 | Destructive actions in chrome |
| Destructive confirm | `BTN_CLOSE` | default | default | default | Final "Close ..." confirmation |
| Quiet text button | none (`frame(false)`) | none | `FG_DIM` / `FG_SOFT`, `PALETTE_RED` for destructive | none | Sidebar row actions, "x" close (16, `FG_DIM`) |
| Accent text button | `blend(PANEL_BG_ALT, ACCENT, 0.08)` | 1 px `blend(BORDER_SUBTLE, ACCENT, 0.3)` | `ACCENT` 11 | 8 | "+ Add ..." in settings |

Use one primary per view. Buttons are never wider than their content plus the
12 x 6 padding unless a `min_size` gives them a consistent rhythm (toolbar 30
high; creation dialog 120 x 40). Icon-only buttons need a tooltip.

Remote browser orientation uses two 26 px device-outline buttons immediately
after the recording controls, with a 6 px radius matching those media controls.
The portrait icon is 10 x 16 px; landscape is 16 x 10 px. A short inset home
indicator reinforces the orientation. Selected styling uses egui's existing
accent selection; disabled controls use egui's disabled scope. Tooltips and
accessible button names identify Portrait and Landscape. Status remains in words
below the toolbar, and the toolbar wraps to preserve the address field on narrow
panels.

### Segmented control / tabs

The settings tab bar is the segmented pattern: a row with 4 px gaps of buttons
at radius 8, 12 text. Selected: fill `blend(PANEL_BG_ALT, ACCENT, 0.20)`, 1 px
outline `blend(BORDER_SUBTLE, ACCENT, 0.5)`, label `FG`. Unselected:
transparent fill, no outline, label `FG_DIM`. Choice toggles inside forms (for
example storage tier) use `Button::selected(true)` at radius 8 and 12.5 text,
which takes egui's selection styling (`alpha(ACCENT, 54)` fill, `ACCENT`
outline).

### Chips, pills and status frames

- **Choice chip**: radius 8, padding 10 x 6, `PANEL_BG_ALT` fill and 1 px
  `BORDER_SUBTLE` outline. Selected: `blend(PANEL_BG_ALT, ACCENT, 0.22)` and a
  1 px `ACCENT` outline. Hover: `blend(PANEL_BG_ALT, FG, 0.06)`. Primary line 12.5
  `FG`, secondary line 10.5 `FG_DIM`, optional status dot in a state color.
- **Pill** (read-only tag): radius 10, margin 8 x 2, fill `alpha(color, 34)`,
  11 strong text in the same color (`color` is a state color).
- **Status frame**: radius 8, margin 10, fill `alpha(color, 24)`, 1 px outline
  `alpha(color, 90)`, title 13.5 strong in the state color, body 12.5 `FG_SOFT`.
- **Selectable card**: radius 10, margin 12, `PANEL_BG_ALT` fill with 1 px
  `BORDER_SUBTLE`. Hover: outline `FG_DIM`. Selected: fill
  `blend(PANEL_BG_ALT, ACCENT, 0.14)` and a 2 px `ACCENT` outline. Keyboard
  focus: 2 px `FG` outline. The whole card is the click target with a pointing
  hand cursor.
- **Checkbox** (dialogs): 16 px box, radius 4. Unchecked: 1.5 px `FG_DIM` edge
  (`FG_SOFT` on hover). Checked: solid `ACCENT` with a 2 px `BG` check mark.
  Disabled uses `FG_DIM` in place of `ACCENT`.
- **Section card** (settings): `PANEL_BG`, 1 px `BORDER_SUBTLE`, radius 10,
  margin 16, followed by 12 of space; heading above it 13 strong `FG_SOFT`.

### Panels and workspaces

- Terminal panel: body `PANEL_BG` blended with its accent by 0.06 when
  focused, radius 16. Titlebar: `blend(PANEL_BG_ALT, accent, 0.28)` focused or
  0.10 unfocused. Title `FG` focused and `FG_SOFT` unfocused, 13. Focus adds a
  short 44 x 2.5 accent bar under the title, a 1.8 px border and a 3 px glow.
- The accent is the workspace accent when the panel belongs to a workspace and
  `ACCENT` (focused) or `BORDER_STRONG` (unfocused) otherwise.
- Workspace labels on the canvas and sidebar use `blend(PANEL_BG_ALT, accent, t)`
  with `t` = 0.08 rest, 0.14 active, 0.18 hover, 0.22 dragging, radius 10, and a
  1 px accent outline (alpha 90 rest, 160 active or hover).
- Sidebar rows: 6 px inset, radius 10. Focused row `blend(.., accent, 0.22)`
  alpha 200 plus a 2 px accent edge; active workspace 0.12 alpha 140; hover
  `alpha(PANEL_BG_ALT, 160)`.
- Panel titlebar and sidebar "Move to Workspace" menus focus a search field
  once on open. Names filter case-insensitively; a bounded scroll list keeps
  other menu actions reachable as the workspace count grows. Rows use 12 pt
  `FG_SOFT` labels, `FG` for the keyboard selection, and workspace-colored dots.
  The current workspace has a disabled row labeled "(current)". Arrow keys
  select eligible destinations, Enter moves the panel, and Escape cancels.
  Long names truncate with a full-name tooltip. Sidebar menus are sublayers
  above the sidebar chrome, so search fields and results remain clickable.

## Terminal

- Background is `PANEL_BG`; default foreground is `FG`. Cells whose color is the
  default take a fast path with no color conversion.
- Colors come from the [palette](#terminal-palette-ansi-0-15) unless the
  application sets its own (indexed or true color). Dim text is drawn at 0.82
  brightness (`gamma_multiply`); "dim" named colors at alpha 196. Inverse swaps
  foreground and background; hidden text is drawn in the background color.
- Selection replaces the cell colors with `alpha(ACCENT, 76)` over `PANEL_BG`
  and `FG` text. The cursor is `CURSOR`: block at 0.8 opacity, underline and
  beam at 0.9, hollow (1.2 px stroke at 0.82) when the panel is unfocused.
- Every foreground is passed through `ensure_terminal_text_contrast` (3.6:1
  floor) against its actual cell background, unless the cell is hidden.
- Grid is 13 px JetBrains Mono at line height 1.3. Do not change metrics in
  chrome code; terminal layout, resize and PTY size all derive from them.

## States, focus and keyboard

| State | Treatment |
|---|---|
| Rest | Neutral fill and `BORDER_SUBTLE` outline, `FG_SOFT` label |
| Hover | Fill steps toward the accent: egui widgets `blend(PANEL_BG_ALT, ACCENT, 0.16)` with `FG` label; rows `alpha(PANEL_BG_ALT, 160)`; cards outline `FG_DIM`; whole-card click targets set a pointing-hand cursor |
| Pressed | egui widgets `blend(PANEL_BG_ALT, ACCENT, 0.22)` |
| Selected | Accent-tinted fill (0.14-0.35, see the ladder) plus an `ACCENT` outline or edge; label promoted to `FG` |
| Keyboard focus | High-contrast outline: 2 px `FG` on cards, `FG` 1.5 px ring expanded 2 px on checkboxes; focused panels get the accent border and glow |
| Disabled | Use egui's disabled rendering (`add_enabled`, `add_enabled_ui`, `ui.disable()`); do not hand-mix a gray. Dialogs disable their whole body while a picker or a pending action owns input |
| Error | `PALETTE_RED` text beneath the field or dialog body; erroneous cards use `blend(BORDER_SUBTLE, PALETTE_RED, 0.5)` as the outline |
| Success / warning | `PALETTE_GREEN` / `PALETTE_YELLOW` dot, pill or status frame with words |
| Busy | Rotating arc spinner in `ACCENT` (radius 12, 2 px stroke) with an optional label and a `FG_DIM` detail line |

Focus behavior:

- Overlays focus their text field on open, counted in frames rather than time
  (`focus_once`): a slow first frame still focuses, and once the person moves
  focus elsewhere the field never takes it back. Do not use a fixed timer.
- A dialog with a title field focuses it once when it opens, unless a pointer
  press is in flight. Focus returns to the launching field when a child picker
  closes.
- Escape closes the top-most overlay or dialog and is consumed so it does not
  also reach a panel beneath. Enter in a dialog's title field submits a valid
  form.
- Keep every action reachable with the keyboard as well as the pointer:
  custom-drawn controls such as selectable cards take keyboard focus and
  respond to Enter and Space.
- Give a custom-drawn control an accessible name and role through
  `widget_info` (the selectable card reports its role, selected state, label
  and price).

## Motion and repaint rules

Horizon is quiet at rest. Motion is meant to report a state or follow the
person's input. Examples in the current UI:

- Canvas pan and zoom the person drives, and egui's own short hover and collapse
  transitions.
- The loading spinner (1.2 s per turn, repainting about every 32 ms while
  visible).
- The three-dot "working" indicator on an agent panel (1 Hz pulse).
- The microphone glyph in a panel titlebar: a slow pulse (about 0.8 Hz) in red
  with an expanding ring while recording, and in `PALETTE_YELLOW` while a
  transcription is in flight.
- The history meter in a panel titlebar, which eases to its new fill over
  0.16 s.

Motion is not decoration: a new animation needs a state to report and a redraw
budget, and must stop repainting when that state ends.

Rules from the [UI Feature Perf Checklist](../../AGENTS.md#ui-feature-perf-checklist)
that apply to design decisions:

- Identify the redraw surface before building: what pointer movement, hover,
  animation, terminal output or config change updates the feature.
- Pointer-only frames are a budget. A hover effect must not add unconditional
  per-frame work across every panel or workspace; compute lazily on interaction,
  gate by on-screen visibility, or cache by stable keys.
- Cache repeated static decoration (grids, minimap, badges, panel chrome) as
  meshes or shapes rather than rebuilding them per frame.
- Do not add broad `request_repaint` or animation loops for passive polish. Use
  `request_repaint_after` with a real reason and interval, and only while the
  thing is visible and active.
- Avoid eager text layout, string building or list construction for every
  visible panel each frame in menus, tooltips, badges and summary labels.
- Any new always-visible overlay must be included in the perf trace and the live
  screenshot check.
- Off-screen culling is fine; do not skip rendering a panel just because its
  workspace is not the active one if it can still be on screen.

## Do and don't

Do:

- Take every color from `theme::*()` and derive tints with `theme::blend` and
  `theme::alpha`; add a token only when a color is reused across unrelated
  widgets.
- Check a new color pair in both themes against the [contrast](#contrast)
  tables before merging; a pair that works in dark can fail in light.
- Use `FG` for anything that must be read, `FG_SOFT` for supporting text, and
  `FG_DIM` only for optional hints and captions.
- Label state in words and shape as well as color (a dot beside "In stock", an
  icon plus "Error").
- Use `BG` as the label color on a solid `ACCENT` fill.
- Use one accent (`ACCENT`) for selection and primary action; use the workspace
  accent for things that belong to a workspace.
- Reuse the radius ladder (8, 10, 12, 16, 20) and the spacing values above.
- Truncate long text with an ellipsis and keep dialog layouts steady as content
  changes.
- Keep chrome text at 11 pt or larger except for fixed micro labels (tags and
  units), and never below 8.5.
- Verify UI changes in both themes with a live screenshot, plus a resize pass,
  as described in AGENTS.md.

Don't:

- Don't hard-code `Color32::from_rgb` values, hex strings or a second palette in
  widgets. A few legacy call sites exist; do not copy them.
- Don't put `FG_DIM` on a tinted or selected fill, or use it for values, errors
  or required instructions.
- Don't put `FG` on a solid `ACCENT` fill (2.45 and 2.75).
- Don't rely on `BORDER_SUBTLE` or `BORDER_STRONG` alone to show a control or a
  state.
- Don't invent extra blend amounts; snap to the ladder (0.05-0.10, 0.12-0.22,
  0.28-0.35).
- Don't use color as the only carrier of meaning, particularly light-mode green.
- Don't pick a workspace accent by index from a raw table; call
  `theme::workspace_accent`.
- Don't hand-mix a disabled color; use egui's disabled rendering.
- Don't add a repaint loop, a per-frame per-panel scan, or an eager layout in
  hover paths for polish.
- Don't add a new modal or overlay style; reuse the modal, overlay and search
  recipes above.

## Reproducing the numbers

Token values come straight from the `DARK_THEME` and `LIGHT_THEME` constants in
`crates/horizon-ui/src/theme.rs`; workspace accents come from
`WORKSPACE_COLORS` in `crates/horizon-core/src/workspace.rs` and
`WORKSPACE_ACCENTS_LIGHT` in `theme.rs`. Contrast ratios use the same formula as
`contrast_ratio` in `theme.rs` (WCAG 2.x relative luminance). To recompute any
ratio:

```python
import math


def lin(c):
    s = c / 255
    return s / 12.92 if s <= 0.04045 else ((s + 0.055) / 1.055) ** 2.4

def lum(rgb):
    r, g, b = rgb
    return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)

def contrast(a, b):
    hi, lo = sorted((lum(a), lum(b)), reverse=True)
    return (hi + 0.05) / (lo + 0.05)

def blend(base, tint, amount):  # same as theme::blend
    amount = min(max(amount, 0.0), 1.0)
    # Rust's f32::round rounds halves away from zero; Python's round() does not.
    mix = lambda b, t: min(max(math.floor(b * (1 - amount) + t * amount + 0.5), 0), 255)
    return tuple(mix(base[i], tint[i]) for i in range(3))

# Example: FG_SOFT on PANEL_BG_ALT in light mode
print(round(contrast((80, 89, 100), (240, 236, 228)), 2))  # 6.03
```

When a token in `theme.rs` changes, recompute the affected tables in this
document in the same PR.
