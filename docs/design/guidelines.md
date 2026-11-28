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
| Canvas vs panel | The canvas backdrop is the darkest layer; panels and raised fills are progressively lighter | Panels are the lightest (near white); the canvas is a warmer, slightly darker paper tone |
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
| `BG` | `#070A10` | `#F3F0EA` | Canvas fill under the [backdrop](#canvas-backdrop), text-input well (`extreme_bg_color`), text color on solid accent fills |
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
| `GRID_DOT` | rgba(190, 200, 230, 41) | rgba(120, 105, 80, 56) | Canvas dot grid |
| `CANVAS_SKY_TOP` | `#04060B` | `#F7F5F0` | Canvas backdrop, top of the sky |
| `CANVAS_SKY_MID` | `#080D1B` | `#F3EFE8` | Canvas backdrop, 46 percent of the height |
| `CANVAS_SKY_HORIZON` | `#131A36` | `#F3E6D8` | Canvas backdrop, the horizon at 72 percent of the height |
| `CANVAS_GROUND` | `#05070C` | `#E6DDCF` | Canvas backdrop, the ground at the foot of the canvas |
| `CANVAS_GLOW_WARM` | rgba(255, 150, 92, 51) | rgba(240, 150, 86, 61) | Canvas backdrop, the centre of the glow |
| `CANVAS_GLOW_COOL` | rgba(150, 110, 235, 26) | rgba(120, 130, 225, 26) | Canvas backdrop, the outer glow |
| `CANVAS_HORIZON_LINE` | rgba(255, 196, 150, 61) | rgba(214, 120, 60, 71) | Canvas backdrop, the horizon line |

### Canvas backdrop

The canvas backdrop is one sheet of colour behind the dot grid. It has three
layers, from the back to the front:

1. The sky and the ground: one vertical gradient over the whole canvas
   rectangle. The stops are a share of the canvas height:

   | Share | Token |
   |---|---|
   | 0 percent | `CANVAS_SKY_TOP` |
   | 46 percent | `CANVAS_SKY_MID` |
   | 72 percent | `CANVAS_SKY_HORIZON` |
   | 100 percent | `CANVAS_GROUND` |

2. The glow: an ellipse that the middle of the horizon line holds. Its
   horizontal radius is 1.30 times the canvas width and its vertical radius is
   0.26 times the canvas height. Its alpha runs from its centre outward:

   | Share of the radius | Colour |
   |---|---|
   | 0 | `CANVAS_GLOW_WARM` |
   | 46 percent | `CANVAS_GLOW_COOL` |
   | 76 percent and outward | the cool colour at alpha 0 |

   The glow is a mesh of 24 rings and 96 segments, so its alpha falls with no
   visible edge. The canvas clips it to the canvas rectangle.

3. The horizon line: one horizontal line of 1 point at 72 percent of the canvas
   height, in `CANVAS_HORIZON_LINE`. Its alpha is 0 at the left edge and at the
   right edge of the canvas. The alpha rises to its full value at 20 percent of
   the width and it stays there to 80 percent of the width.

A measure is a share of the canvas rectangle of the window, so the horizon
stands at the same share of every canvas: the canvas of the root window, the
canvas of a detached workspace window and the canvas behind the first start
page. The canvas starts under the toolbar. The backdrop does not show under the
sidebar, the dock, the Settings panel or the settings bar.

The backdrop is fixed to the screen. A pan or a zoom of the canvas moves the
dot grid and it does not move the backdrop.

The shapes depend only on the canvas rectangle, the pixels per point and the
theme. `canvas_backdrop.rs` keeps them, and a frame where none of the three
changed paints the shapes of the frame before and builds nothing.

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
| `BG` on solid `ACCENT` (dialog confirm button label, checkbox mark) | 6.65 | 5.33 |
| `FG` on solid `ACCENT` (do not use) | 2.45 `*` | 2.75 `*` |
| `FG` on danger button fill | 9.38 | 9.88 |
| `FG` on selected tab fill (`blend .20`) | 10.37 | 10.72 |
| `FG_DIM` unselected tab label on `BG_ELEVATED` | 4.59 | 3.48 `*` |
| `FG` on selected list row (`blend .28`) | 8.88 | 9.51 |
| `FG_SOFT` on selected list row (`blend .28`) | 5.47 | 4.05 `*` |
| `FG` on active search toggle (`blend .35`) | 7.79 | 8.55 |
| `FG_SOFT` on hovered widget fill (`blend .16`) | 6.80 | 4.83 |
| `ACCENT` text on `+ Add` button fill (`blend .08`) | 5.17 | 4.61 |
| `FG` / `FG_DIM` on a menu search field (`BG`) | 16.28 / 4.78 | 14.68 / 3.25 `*` |
| Menu search focus outline `alpha(ACCENT, 200)` on `BG` | 4.44 | 3.58 |
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
| Weakest agent line on `BG` (clear) | 7.73 | 3.13 |
| `ACCENT` outline / focus | 6.18 | 5.86 |
| `FG` focus ring | 15.13 | 16.13 |
| Primary button outline on `BG_ELEVATED` | 4.99 | 4.47 |

A line of an agent is a graphic and not a separator, so the light theme lifts the
weakest of its accents to 3:1 at the clear strength (see
[Agent lines](#agent-lines)).

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

### Weights

Inter is a variable face. One file holds every weight, and the rasteriser draws
a weight only when the layout asks for the `wght` coordinate of that weight.
`typeface::Weight` names the weights of the approved mockups. Do not write a
`wght` value in a call site.

| Weight | `wght` | Where it is used |
|---|---|---|
| Regular | 400 | Body text, secondary lines, hints, key labels, the second line of a heading |
| Medium | 500 | Sidebar row titles |
| Semibold | 600 | Row titles, the label of a primary button, the step that is under way, the title bar of a focused panel, the sidebar brand and its captions, Settings headings |
| Bold | 650 | The heading of a card, the title of a tile |
| Black | 700 | The heading of the first start page, a caption in upper case |

`egui::RichText::strong` is not a weight in this toolkit version. It paints the
same glyphs in the strong colour of the visuals. Text that must look heavier
asks for a weight: `typeface::rich` for a stock widget and `typeface::format` or
`typeface::Weight::coords` for a layout job. The word "strong" in the table
below means such a weight.

Both faces of the interface ask for normal hinting (`FontTweak::hinting_target`).
A stem at 9 to 14 screen points then lands on a pixel. A fallback face keeps
the default hinting.

Both faces keep the horizontal fit of the hinter off
(`SmoothHinting::preserve_linear_metrics`), so every glyph keeps the ink and
the advance of the unhinted face. This rasteriser makes the ink of Inter
lighter and wider with the fit on: the stem of a 12.5 point `l` is then two
columns of a quarter ink instead of one column and two fading edges. On
JetBrains Mono the fit changes neither the ink nor the advance at 13 and 12.25
points, so the terminal keeps its cells either way. One rule covers both faces.

Both faces turn the sub-pixel bins of the rasteriser off
(`FontTweak::subpixel_binning`). With the bins on, the toolkit rasterises a
glyph at the quarter pixel of its pen position, so one letter of a run gets a
picture with its stem in one column and the next letter a picture with the same
stem in two columns at half ink. That grey double is what a reader sees as a
soft stem, and it appears at every zoom, including 100 %. The toolkit rounds
the ink of every glyph to a whole device pixel anyway, so the bins only buy a
picture that no longer matches the place it is drawn. With the bins off, one
glyph has one picture at any pen position.

### Text under the canvas zoom

The canvas is one layer under a scale transform. A text shape under a layer
transform keeps the glyph pictures that were made for scale 1 and scales only
their rectangles. Therefore every text that the canvas zoom draws goes through
`zoom_text`, and no call site multiplies a font size by a zoom itself.

- `zoom_text::layout_job`, `layout_no_wrap`, `text` and `text_shape` take a
  painter or the terminal's font rasteriser. They lay the text out at the size
  that it takes on the screen and hand the painter the glyph offsets of that
  size, divided by the zoom that made the pictures. One glyph texel then meets
  one pixel.
- A glyph corner of the canvas sits on a whole device pixel at rest, at every
  zoom and at either display scale. The toolkit moves a shape into the frame of
  the window before it rounds a text position, so the line origin lands on the
  window's pixel grid. Each glyph offset is a whole pixel of the screen size,
  divided by the zoom that made its picture and multiplied back by the layer
  transform of that same zoom. `zoom_text` has a test that tessellates a card
  line, a title and three terminal runs at zoom 0.67 and 0.94 with a fractional
  pan offset and asserts that every glyph corner is within 0.01 of a whole
  device pixel and that no glyph picture is stretched.
- The layout stays the layout of zoom 1. The rectangle, the line height, the
  cell grid of a terminal and the height estimate of a card are the values of
  zoom 1, so a part of a card keeps its size and its height at every zoom.
- The lines of a text are the lines of zoom 1. A job of this toolkit breaks its
  lines on the pixel-rounded position of a glyph, so a text can break at one
  zoom and not at another. `layout_job` therefore lays the job out at the sizes
  of zoom 1 first, takes the text of each of its rows, and lays those rows out
  as one no-wrap line each at the screen size. A line that zoom 1 elided keeps
  its elision and does not grow back. The terminal, which lays one run out at a
  time, has no wrap width and needs none of this.
- The screen sizes are quarter points. A zoom that moves keeps the pictures of
  the zoom at which the gesture started for 120 ms, then makes the pictures of
  the new size once and asks for one frame. A still window asks for no frame.
- The state keeps at most 64 screen sizes. The toolkit clears its whole glyph
  atlas when it passes 80 % fill, so the pictures of a long zoom session cannot
  fill it.
- Zoom 1 takes the plain path. The look of a display of scale 1 does not change.
- A stock widget takes its text from `zoom_text::widget_text`, not from
  `RichText`. A label or a button that a stock widget lays out itself takes the
  font size of zoom 1 and the layer transform then scales a finished picture of
  every letter. `widget_text` hands the widget a galley that is laid out at the
  screen size and keeps the size and the line of zoom 1, so the part stays where
  it is and its letters stay sharp. The test of a part that takes it tessellates
  the part in a canvas pass at a zoom and asserts that every glyph corner is
  within 0.01 of a whole device pixel and that no picture is stretched.
- A widget that lays out its own text takes a pane of `screen_pane` instead. A
  drag value and the markdown viewer draw their text with the fonts of the
  context, so no galley reaches them and `widget_text` cannot help. A pane is a
  layer of its own, directly above the layer of its panel, whose transform is
  the identity: the widget inside it lays its text out at the size that the text
  has on the screen, and the pane is laid out in the coordinates of the screen
  at the zoom times the layout of zoom 1. `screen_pane::add_widget` takes the
  place that such a widget takes in the layout of the panel, reserves it, and
  draws the widget in a pane over that place, so a row keeps its place, its
  size and its line at every zoom. The pane can be a couple of points wider than
  the row of zoom 1 times the zoom, because the glyph advances of the screen
  size are rounded to the whole device pixel. At zoom 1 the plain path runs and
  the look of a display of scale 1 does not change.

### The floor of a Fit of a workspace

A Fit of a workspace chooses the zoom at which the workspace fits the canvas,
never one above 1. It never chooses one under the floor of
`canvas.fit_floor_zoom`, which is 0.85 by default and is one setting of the
Settings window, General, Canvas.

| Value | Size at the floor | Source |
|---|---|---|
| Smallest text of a card | 10.5 x 0.85 = about 9 screen points | `CAPTION_FONT` in `block_widget/metrics.rs` |
| Text of a terminal | 13 x 0.85 = about 11 screen points | JetBrains Mono 13 in this document |
| Panel titlebar title | 13 x 0.85 = about 11 screen points | Inter 13 in this document |

A workspace that does not fit the canvas at the floor keeps the floor: its top
left corner meets the top left margin of the canvas, and the rest of it is off
the canvas. The minimap draws the whole workspace with the rectangle of the
view, so the person reads where the rest of it is and pans there. A Fit never
shows a workspace at a zoom where its text cannot be read, and it never upscales
the canvas either.

### Text of the canvas that a stock widget still draws

A stock widget of the toolkit draws its own text through the fonts of the
context, so this text is scaled by the canvas zoom and not laid out for the
screen size. These parts still draw that text:

| Part | What a fix would take |
|---|---|
| The address field of a browser panel | Done: the panel gives `TextEdit` a layouter, as the editor does |
| The notice strip of a terminal, and the banners of a browser panel | Done: the sentence and its buttons take `zoom_text::widget_text` |
| The chrome of a browser panel beside the address field: the engine name, the profile pill, the chip, the elapsed time of a recording, the notice lines and the hand-off banner | Done: they take `zoom_text::widget_text` |
| The mode bar of the editor | Done: its two modes, its file name and its error line take `zoom_text::widget_text` |
| The placeholder of a browser panel: its state line and the note under it | Paint both lines with `zoom_text` inside the centred area, and keep the response for the tooltip |
| The preview of the editor | Done: round 50 draws it in a pane of `screen_pane` at the size of the screen. Round 53 shows the pictures of a note: each picture is drawn at its natural size times the zoom, in the same pane, so its proportion to the text is the proportion of zoom 1 |
| The rows and the controls of a device panel | Done: the labels take `panel_text` and therefore `zoom_text::widget_text`, and the seven drag values of the controls draw through `screen_pane::add_widget` |
| Every popup of a panel | A popup is its own layer; the canvas transform would have to reach it first |

An overlay or a popup that is not under the canvas transform is already sharp,
so the fix is not to add the transform to it. The list above is the set of parts
that are under the transform and still take the toolkit's font size.

The cell grid of a terminal stays a logical grid: its columns and rows are the
values of zoom 1 and only the view scales them, so a zoom never resizes the
shell. The runs of a grid are drawn at the screen size, and each run origin is
rounded to the same whole pixel grid as every other text. The cell size is not
rounded to whole pixels, because a column would then be up to half a pixel
away from its share of the panel: at zoom 0.94 a cell of 7.80 points is 7.33
device pixels, and 80 whole-pixel columns would be 560 pixels wide against
586.9, and 30 rows of 16 would overflow the 476.6 that they have. A run keeps
the letter spacing that zoom 1 has, where the advance of 7.80 points already
puts glyphs on rounded device pixels with the same sub-pixel remainder.

### Scale

Horizon does not override egui's named text styles (defaults for egui 0.36:
Body and Button 13, Heading 18, Small 9, Monospace 13). Widgets that need a
specific size set it explicitly with `RichText::size` or `FontId`, so the
sizes actually used form this scale. Size is in logical points. The weight comes
from the `Weight` table above.

| Size | Weight | Where it is used | Color |
|---|---|---|---|
| 28 | strong | Startup session screen product name | `FG` |
| 26 | strong | Dialog and page titles ("Sessions", the saved session label), loading-screen product name | `FG` |
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
| 12 | Default widget radius in the egui style, overlay input wells, summary cards, agent chips of the sidebar |
| 10 | Buttons (primary, chrome, danger, 40 px dialog buttons), section cards, selector cards, workspace labels, pills, toolbar search input shell, sidebar work item and machine rows, the control `+ Rent a machine` |
| 11 | The row `Front desk` of the sidebar |
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
| 8 | Gap between form rows in a dialog; sidebar row horizontal inset; air before the state word of a work item row |
| 6 | Gap between preset cards |
| 4 | Gap between tabs in the tab bar |

Fixed metrics:

| Metric | Value |
|---|---|
| Root toolbar height | 46 |
| Toolbar buttons | 30 high, 8 gap, 14 horizontal padding |
| Sidebar width | 210 at every window size |
| Panel titlebar height | 34 |
| Panel padding | 8 |
| Panel resize handle | 32 screen points, independent of canvas zoom (bounded by panel extent). The painted mark is six dots in the corner. Those dots keep the same screen size when the corner can hold them. The square is the hit target and has no fill. A terminal scrollbar stops above this square, so the foot of its track stays reachable. A panel too short for both keeps a 24 point track and gives the gap up. On a card panel the hit target stops above the footer: its height is the on-screen height of the footer and the panel padding under it, at most 32 and at least 10 screen points, so a button of the card never loses its press to a resize grab. |
| Line between an agent and a panel it controls | 1.5 screen points wide at every canvas zoom, with a filled dot of radius 3 at the agent end and a ring of radius 3 with a 1.5 point stroke at the other end |
| Canvas dot grid | 22 spacing and 2.3 dot diameter at 100% zoom. Both scale with zoom; the spacing doubles until it is at least 14 on screen (so zooming out shows a coarser grid, not none) and the dot diameter is clamped to 1-5. The colour is `GRID_DOT` |
| Canvas horizon line | 1 point thick, at 72 percent of the canvas height |
| Text field in a dialog | 38 high, text margin (12, 10) |
| Menu search field | 32 high, radius 8, text inset 28 left and 10 right |
| Overlay text input | 44 high |
| Command palette | 500 wide, 36 row, 28 section header, at most 12 visible rows |
| Search dropdown | 600 wide, 36 high toolbar input, 32 row, 24 section header, at most 12 visible rows |
| Directory picker | 520 wide, 34 row, at most 460 tall |

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

1. **Canvas**: the backdrop of the canvas (the sky and the ground, the glow and
   the horizon line, see [Canvas backdrop](#canvas-backdrop)) and the dot grid.
   The window fills `BG` under the backdrop.
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

### Agent lines

One thin curve runs from the edge of an agent panel to the edge of each panel
that the agent controls: a card that it drew, a page that it drives, and a
terminal or a preset panel that it opened for the person. The pairs come from
the owner and the opener facts of the board, so a panel without one carries no
line. The owner of a card, the owner of a page and the opener of a panel are the
`horizon:<local id>` actor of the agent panel, so two agent panels of one kind
each keep the lines of the panels that they control. A panel that a session of an
older build saved holds no actor, and its mark says only the kind of its opener:
the mark names one panel when one agent panel of that kind stands in the
workspace, and no panel when two do.

- Colour: the workspace accent of the agent, so two agents of one workspace
  differ. The agent takes the accent of its place among the agent panels of its
  workspace, which keeps the colour while the board does not change. A line is a
  graphic that a person must see, so the light theme keeps a floor of 3 to 1
  against the light canvas at the clear strength: the accent of a line that reads
  weaker there, the deep yellow, the teal and the peach, is darkened in the
  colour of the line alone. The accent of the workspace keeps the colour of its
  fills, outlines and tabs, and the dark theme keeps every line, because each of
  its accents already stands at 7.7 to 1 or above on the dark canvas.
- Width: 1.5 screen points at every canvas zoom. The end marks are 3 screen
  points: a filled dot at the agent end and a ring at the other end.
- Geometry: the line leaves the side of the panel that faces the other panel, at
  a right angle, as a cubic Bezier. The starts of the lines of one agent spread
  along that side in the order of their ends, so those lines do not cross near
  the panel. Two panels that overlap, and two whose facing edges stand closer
  than 8 points, carry no line: the dot and the ring of a shorter line would
  merge into one mark. The two panels of a pair decide that, never the two
  places that the starts take: a start follows the number of lines of one side
  and the height of its panel, so a card beside its agent would draw its line in
  one frame and lose it in the next. Each control point pulls at most half of
  the way along the axis between the two facing edges, so a short line is a
  plain curve in the gap: it never passes the other panel and never folds back
  under the two panels as a curl beside the agent. A long line pulls 0.45 of
  the distance between its two ends, between 40 and 260 points.
- Strength: the lines of the agent in focus are clear (alpha 230), the lines of
  every other agent are faint (alpha 46), and at a canvas zoom of 0.6 or less
  every line takes one middle strength (alpha 153). The agent in focus is the
  agent panel under the pointer, else the agent of the panel under the pointer,
  else the agent panel that holds the keyboard, else the agent that controls the
  panel that holds the keyboard. With no agent in focus every line is faint.
- Place: the lines sit above the canvas dot grid and the workspace frame and
  below the panels, so a line that passes behind a panel is hidden by it. They
  are drawn on the canvas of the root window only: a panel of a detached
  workspace, a hidden panel, a minimized panel and a panel behind a fullscreen
  panel carries no line.
- Cost: the shapes are cached under the pairs, the places of the panels, the
  view, the agent in focus and the theme. A frame where none of those changed
  draws the cached shapes and builds none. The setting **Lines between an agent
  and its panels** in Settings, General, Canvas turns the lines off.

## Components

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
- Canvas preset picker, above its agent entries: when the workspace that the
  picker makes a terminal in is on a machine, one line in 11 `FG_DIM`, 2 above
  the `Agents` header: `Agents start on this computer. Terminals start on
  <machine>.` A workspace on this computer adds no line. The line is built when
  the menu opens, not on a frame that draws it.

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
- A menu search field does the same. See [Menu search field](#menu-search-field).

### Buttons

| Kind | Fill | Outline | Label | Radius | Where |
|---|---|---|---|---|---|
| Primary (emphasized chrome) | `blend(PANEL_BG_ALT, ACCENT, 0.28)` | 1 px `blend(BORDER_STRONG, ACCENT, 0.72)` | `FG` 11.5 | 10 | Toolbar "update available" |
| Secondary / chrome | `PANEL_BG_ALT` | 1 px `alpha(BORDER_SUBTLE, 210)` | `FG_SOFT` 11 | 10 | Toolbar and sidebar actions, 30 high |
| Danger | `blend(PANEL_BG_ALT, PALETTE_RED, 0.22)` | 1 px `blend(BORDER_STRONG, PALETTE_RED, 0.68)` | `FG` 11 | 10 | Destructive actions in chrome |
| Destructive confirm | `BTN_CLOSE` | default | default | default | Final "Close ..." confirmation |
| Quiet text button | none (`frame(false)`) | none | `FG_DIM` / `FG_SOFT`, `PALETTE_RED` for destructive | none | Sidebar row actions, "x" close (16, `FG_DIM`) |
| Accent text button | `blend(PANEL_BG_ALT, ACCENT, 0.08)` | 1 px `blend(BORDER_SUBTLE, ACCENT, 0.3)` | `ACCENT` 11 | 8 | "+ Add ..." in settings |

Use one primary per view. Buttons are never wider than their content plus the
12 x 6 padding unless a `min_size` gives them a consistent rhythm (toolbar 30
high). Icon-only buttons need a tooltip.

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
at radius 8, 12 text, and 6 px of horizontal button padding so the six short
labels stay on one row at the default panel width. Selected: fill `blend(PANEL_BG_ALT, ACCENT, 0.20)`, 1 px
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
- **Connection mark** (SSH panel titlebar, left of the history meter and the
  close control): pill of the state color, 10 proportional text, a fill of
  `alpha(color, 72)` on a sixth of the color and a 1 px outline `alpha(color, 140)`.
  The word carries the state, so the color is never the only signal:
  `Connecting...` in `PALETTE_YELLOW`, `Connected` in `PALETTE_GREEN`,
  `Reconnecting` in `PALETTE_YELLOW` while the panel waits to run its command
  again after a lost link, and `Disconnected` in `PALETTE_RED`. A reconnect names
  its attempt from the third one on, `Reconnecting (3)`, and the mark keeps the
  color of the plain word. A panel that waits keeps the last screen in its body
  until the command starts again, and the same screen stays while the new
  connection runs and until the first byte of it draws over the screen. The
  Machines row of that machine shows the same state.
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
- **Settings panel**: 30 percent of the window, between 340 and 900 points. It
  keeps one rectangle on every tab, and the canvas ends at its left edge. A row
  of a tab wraps or cuts its text inside the panel. A name or a path that a row
  cuts shows the whole text in its hover text. When a row cannot keep its
  actions beside the text, the actions take the line under the text.

### A lost link

One vocabulary names a lost connection in every panel, so a person reads the same
words in a terminal, in a device viewer, in a browser panel and in the Server tab.

- The noun is **the link**. A state sentence names what the link goes to:
  `The link to lab-1 is lost.`, `The link to the desktop is lost.`,
  `The link to the browser is lost.`
- The actor of a retry is **Horizon**: `Horizon tries lab-1 again.`,
  `Horizon tries again by itself.`, `Horizon tries again in 4 s (attempt 2).`
  A plain SSH panel that no timer reaches says that typed text is not sent, and
  it offers `Reconnect`.
- A cause sentence names the other side and starts with **The**: `The desktop
  link timed out`, `The computer that runs the desktop refused the connection`,
  `The server did not answer at its address.`
- A mark names the state and not the cause: `Connecting...`, `Reconnecting`,
  `Disconnected`, `Server away`.
- Do not write "connects again" or "reaches again" for a retry that Horizon
  starts by itself, and do not name the panel or the viewer as the actor.

### Card panel

A card panel shows a document that an agent composed. Horizon owns the frame
around it: the state chip, the footer and the status frames. The document owns
the body. Sizes come from `crates/horizon-ui/src/block_widget/metrics.rs` and
the rules from the modules beside it.

- Body: 16 of space above the first component, 16 on both sides, 16 below the
  last one, and 16 between two components. The body scrolls; the footer does
  not.
- Spacing grid: every gap and padding of `metrics.rs` is a multiple of 4 points,
  and 2 points only for a hair gap inside a row, such as between the two lines
  of a list row. One set of radii serves every part: 12 for a frame inside a
  card (the board, the callout, the box of a `kv`), 8 for a control (a row, a
  pill, an item of a board, a button, a badge, the state chip, a status frame),
  4 for a small part (a tag) and 2 for the bar of a progress.
- Heading: title 18 strong `FG`; optional second line 12.5 `FG_SOFT`, 4 below
  the title; optional badge on the title row, right aligned, 12 to its left.
  The title and the second line take the lines that their words need.
- Badge: 24 high, radius 8, padding 12 on both sides, text 11.5 in the tone
  colour, 6 px dot at 4 from the text. A state tone (`ok` `PALETTE_GREEN`,
  `warn` `PALETTE_YELLOW`, `danger` `PALETTE_RED`, `info` `PALETTE_CYAN`,
  `primary` `ACCENT`) fills with `alpha(colour, 28)`, outlines with
  `alpha(colour, 90)` and carries a filled dot. The neutral tone is a raised
  pill: `PANEL_BG_ALT` fill, `BORDER_SUBTLE` outline, `FG_SOFT` text and no dot.
- Label and value box (`kv`): `BG_ELEVATED` fill, 1 px `BORDER_SUBTLE`, radius
  12, 12 of horizontal inset. A pair is at least 34 high: label 12.5 `FG_SOFT`
  at the top left, value 12.5 `FG` or monospace 12 when the agent marks the
  value `mono`, 8 of space above and below the text. A 0.5 px
  `alpha(BORDER_SUBTLE, 179)` line separates two pairs. The box keeps one
  column of labels, as wide as its widest label and at most 40% of the row, and
  the value takes the rest. Both halves keep the lines that their words need: a
  value of one line keeps the right edge of its row, and a value of more lines
  starts at the left edge of the value column, where its lines line up. A pair
  grows with its taller half, so no word of a pair is cut.
- Body text: 12.5 `FG_SOFT`, line height 1.5 x the size, wrapped.
- One line, cut with an ellipsis when it does not fit, because each names a
  control or a state and not a sentence: a link label and the host under it, a
  badge text, a button label, a caption, the label and the time of a step, the
  state chip and the footer. A text that is cut shows whole while the pointer
  rests on it: a button label and the title of a pill of a list carry that hover
  text today. The heading, its second line, body text, both
  halves of a `kv` pair and both lines of a list row take the lines that their
  words need.
- List: rows radius 8, 8 apart. A row is 12 above and below its text, 12 of
  horizontal inset, and a 14 px radio mark 12 left of the text. Every row of one
  list takes the height of its tallest row, so the marks and the values of the
  list keep one column. Unselected:
  `PANEL_BG_ALT` fill, 1 px `BORDER_SUBTLE`, hollow mark in `BORDER_STRONG`.
  Selected: 2 px `ACCENT` outline, `blend(BG_ELEVATED, ACCENT, 0.18)` fill and a
  filled `ACCENT` mark with a `BG` centre. A selected row of a card without its
  agent keeps that look in a dimmed form: 2 px `alpha(ACCENT, 120)` outline,
  `blend(BG_ELEVATED, ACCENT, 0.09)` fill and the same mark in
  `alpha(ACCENT, 120)`. Hover on a live row:
  `blend(PANEL_BG_ALT, ACCENT, 0.10)`. Title 13 `FG`, second line 11.5 in
  `FG_SOFT` or in the tone colour of that row. The title and the second line
  keep the lines that their words need, and the row grows with them; the value
  or the badge keeps the right edge of the row.
- Progress: label row 11.5 `FG_SOFT`, label on the left and the detail on the
  right, then 8, then a bar 4 high, radius 2, track `PANEL_BG_ALT`, fill
  `ACCENT` at the percentage of the width.
- Buttons: 36 high, radius 8, 8 apart, in document order from the left. A
  `danger` button keeps the right edge of its row; when the row is too narrow
  the destructive buttons move to a right-aligned row of their own, and the
  ordinary ones wrap. Primary: solid `ACCENT` with `BG` text and 16 of
  horizontal padding (12 for every other button); only the first primary button
  of a card is drawn primary and a later one is drawn neutral. Danger: `PALETTE_RED` text,
  `alpha(PALETTE_RED, 23)` fill and `alpha(PALETTE_RED, 90)` outline. Every
  other button: `PANEL_BG_ALT` fill, 1 px `BORDER_SUBTLE`, `FG` text. Disabled:
  `PANEL_BG_ALT` fill, `BORDER_SUBTLE` outline and `FG_DIM` text whatever the
  tone.
- Divider: 1 px `BORDER_SUBTLE`, full width.
- Link: the label is 13 `ACCENT` with a 1 px `ACCENT` line under it, cut with an
  ellipsis; the host of the URL is 11.5 `FG_DIM` 2 below the label, cut with an
  ellipsis too. A pointer on the label turns into a pointing hand and the hover
  text shows the whole URL and `Opens in a browser panel` on a second line. A
  link stays live while its agent is gone, because Horizon opens the page: only
  a host dialog over the card dims it to `FG_DIM`. A press opens a browser panel
  of the card's workspace beside the card and records no click. A press that
  opens no page says why in a line of its own under the host of the address,
  11.5 `PALETTE_RED`, cut with an ellipsis: `Could not open: <reason>`. The
  card keeps that line until the next press that opens the page or the next
  document, and the reason is also the line that the sidebar notice shows.
- State chip (titlebar, left of the close control): 20 high, radius 8,
  `BG_ELEVATED` fill, 1 px `BORDER_SUBTLE`, 8 of horizontal inset, text 10.5
  monospace `FG_SOFT`, a 6 px dot 4 left of the text. `<agent> listens` with a
  filled `PALETTE_GREEN` dot while a click wait is open; `<agent>` with a hollow
  `FG_SOFT` dot while the agent is in the workspace but not waiting; `No agent`
  with a hollow dot when the owner is gone. The chip hides rather than cover
  the title.
- Footer: 30 high, 1 px `BORDER_SUBTLE` top border, 11 `FG_SOFT` (not
  `FG_DIM`, which does not reach 4.5:1 on a panel), 16 of horizontal inset. Left
  `Card drawn by the agent <name>`, or `Card drawn by an agent` when the owner
  is gone; right `updated HH:MM` in local time from the last create or update.
- Mark of a body that holds more: while the content continues under the lower
  edge of the body, the last 32 points of the body fade into the body colour,
  and the last 20 of them are a strip in the full body colour. The words
  `More below` stand centred on that strip, in 11 `FG_SOFT`, with a small down
  mark 8 to their left, so that no faded content shows under them. The mark is a
  control: a press on it takes one page of the scroll, with no animation. A body
  that already shows its end asks for its foot instead: a press moves the card up
  by the room that the lower edge of the view hides, with the margin of a jump,
  and keeps the zoom of the moment. The top of a card that is taller than the
  view then leaves it, because the person asked for what is below. The mark goes
  with the foot.
  Horizon draws it above the footer, so a document can neither draw it, cover it
  nor hide it. A card whose body holds all of its content draws no mark, and the
  mark announces `More below, press to scroll` to a screen reader.
- Status frame: radius 8, 12 above and below, 12 on both sides,
  `alpha(colour, 23)` fill, 1 px `alpha(colour, 90)` outline, strong line 12.5
  in the colour and a second line 12 `FG_SOFT` at line height 1.45. A card
  without its agent shows one at the top of the body in `warn`; a card that
  refused a click shows one in `info`.
- A heading keeps at least 60 per cent of its row. A badge that would leave it
  less than that gives up the row and stands on its own row under the second
  line, left aligned, so a narrow card cuts neither the name nor the badge.
- A document that asks for the wide card with a board opens as wide as its
  board needs and the canvas view allows: the half of the view, between 230
  plus 260 for each wave and 230 plus 480 for each wave, plus the 16 of body
  padding on both sides, and 2400 at most. A view that cannot give the least
  width opens a narrow card. A wide document without a board opens at the half
  of the view, between 1100 and 2400.
- The board draws the grid only when every wave keeps a column of at least 260
  points: the body needs 230 plus 260 for each wave. Under that, also inside a
  card that draws the wide form, it draws the list of the narrow form, 560
  points wide at most, at the left edge of the body.
- A column of the grid is 480 points wide at most, and a grid that needs less
  than the body keeps the left edge of the body; the callout, the log and the
  buttons keep the full width of the body. An item of the grid takes one line of
  evidence, and the list of the narrow form takes two; a longer evidence ends
  with an ellipsis and shows whole while the pointer rests on the item.
- A new card opens 420 wide and, once the body has measured its content, as
  tall as that content needs, between 140 points and the ceiling of the card.
  The ceiling of a narrow card is the height of the canvas of the window at the
  fit of the card, not less than 560 points and not more than 900 points: the
  smallest window keeps a card that it can show, and a tall window shows a tall
  card whole. The ceiling of a wide card is 1200 points, or the room that the
  canvas view shows at the zoom of the moment when that is more. A window that
  changes its size changes no card; the next document of a card reads the room
  of that moment. A card that the person resized by hand keeps that size, a card
  that is a tile of an arranged workspace keeps the size of its preset, and a
  card that is taller than its ceiling scrolls its body above a footer that
  stays at the bottom. The status frames are part of what the content needs: the
  card fits again when one appears or goes, such as the frame of an agent that
  left and the frame of a refused click.
- A new card stands beside its agent panel with the top edges level, or below it
  with the left edges level when the place beside is taken. When both places are
  taken it continues the same shape: below the column of the agent panel with
  the left edges level, or beside its row with the top edges level. It covers no
  panel of its workspace in any of these places.

### First start page

The page that shows while the board is empty has two forms, and the orchestrator
dock chooses the form.

- A window that shows the orchestrator dock draws the page of the front desk
  ([the page of the front desk](#the-page-of-the-front-desk)). The setting
  `features.orchestrator_dock` is on at first, so this is the page of a first
  start.
- A window with no dock draws the page of four starts. The dock is off when the
  setting is off, when the environment switch is `0`, or when the window leaves
  less than 320 points of canvas beside it.
- Both forms show on the conditions of the page of four starts, and both stand in
  the canvas rectangle that the dock leaves. The block of a page is centred in
  that rectangle.

#### The two texts of the toolbar

- The toolbar says `work items, agents and machines` in `FG_DIM` at 10.5 after the
  name `Horizon`.
- The field of the toolbar search holds the hint `Search` in `FG_DIM` at 12.5
  while it holds no query.

#### The page of the front desk

The page sends the person to the conversation at the right of the window.

- The block of the page is centred in the canvas rectangle that the dock leaves,
  and the middle of the block stands at 46 percent of the height of the canvas. A
  block that is taller than the canvas keeps the top pad of 44 points and scrolls
  under it.
- The heading is `Start at the front desk`: 28 points, weight 700, `FG`, centred
  in the block.
- The line under the heading stands 8 points under it, centred, with a width of
  560 points at most: 14 points, `FG_SOFT`,
  `Write what you want done in the conversation at the right. Each task becomes a
  work item with its own orchestrator, and its work shows on this canvas.`
- The steps stand 32 points under that line, in one row of three, centred as a
  group, with 40 points between two steps. A step is 200 points wide, and its
  texts are aligned at its left edge.
- The chip of a step is 22 by 22 points at radius 11, with a 1 px outline of
  `alpha(ACCENT, 140)`. Its digit is `ACCENT` at 11.5 semibold, centred in the
  chip.
- The title of a step stands 10 points under its chip: 13.5 semibold, `FG`. A
  sentence stands 4 points under the title: 12.5, `FG_SOFT`, at most 3 rows.
- A line of 1 px `BORDER_SUBTLE` joins two chips at the height of the middle of
  the chip. It starts 10 points right of the chip and ends 10 points left of the
  next chip.
- The steps stand in one column when the block is narrower than 728 points, which
  is 3 x 200 + 2 x 40 + 2 x 24. The column is aligned at the left edge of the
  block, with 20 points between two steps and no joining line.
- The steps, in their order:

  | Number | Title | Sentence |
  |---|---|---|
  | 1 | `Write a task` | `One message can hold several tasks. Say what done means.` |
  | 2 | `Press Start` | `The front desk shows the work items first. Nothing starts and nothing costs before your press.` |
  | 3 | `Answer what needs you` | `A question shows at the top of the sidebar with its buttons, wherever you look.` |

- The link stands 32 points under the steps, centred: `More ways to start` in
  `ACCENT` at 12.5 semibold, with the chevron of a tile action (two lines of
  1.6 px, 5 points, 8 points after the text). A pointer over it shows the
  pointing hand.
- A press on the link shows the four starts of the page 24 points under the link.
  The four starts take their layout of the width at hand, and the link reads
  `Fewer ways to start`. A second press hides them again. The state of the link
  belongs to the run, and no file keeps it.
- The four starts work as they do on the page of four starts.
- The row of shortcuts of this computer stands 28 points under the link, as the
  page of four starts draws it.
- The page takes no keyboard focus when it appears, and it moves no focus into
  the field of the front desk.
- The page keeps its layout in the cache of the first start page, with the same
  keys and the form of the page and the state of the link. A frame that changes
  none of them lays nothing out and asks for no repaint.

#### The page of four starts

The page of four starts answers the size of the window.

- The block of the page takes the canvas width less 60 points on each side, and
  1 480 points at most. It stands in the middle of a wider canvas, so a large
  screen does not stretch a sentence or a row over 4 000 points. The pads of the
  page are 60 on both sides, 44 above the heading and 28 under the row of
  shortcuts.
- The line under the heading wraps at 720 points, and each of its rows is
  centred in the block.
- The block is centred in the height of the canvas when the canvas holds it with
  the top pad above and the same pad under it. A block that needs more room keeps
  the top pad, so the page never stands under the titlebar.
- The drawing band of a tile is 92 points high and holds a drawing that is drawn
  for 210 points of width. A band that is narrower or shorter draws the drawing
  smaller, and no shape of a drawing leaves its band.
- A canvas that does not hold the whole block first gives the band the compact
  height of 30 points, then no band, so the four choices stay reachable without
  a scroll for as long as the numbers allow. A canvas that holds none of the
  three forms gives no band and scrolls: the wheel, the trackpad, Page Down and
  Page Up move the page, and a tile that Tab reaches below the canvas comes into
  view.
- While the page shows, the wheel belongs to the page: a vertical wheel or
  trackpad scroll moves the page, a horizontal scroll moves nothing, and a
  Ctrl/Cmd wheel moves nothing, because the page has nothing to zoom. The canvas
  under the page neither pans nor zooms for these wheels, and a page that the
  window holds does not move at all.
- A row of the section **Pick up where you left off** keeps the program of the
  session at the start of its detail and the time and the state at its end. A
  folder that does not fit is cut in its middle with an ellipsis. The press of a
  row keeps 74 points of width, and the detail keeps a gap of 8 points to it.
  The title of a row uses the width that the row has: a wide row shows the whole
  stored title, and a narrow row cuts it at the end of its room with an ellipsis.
  The store keeps 160 characters of a title, which is what the widest row shows.

### Panels and workspaces

- Terminal panel: body `PANEL_BG` blended with its accent by 0.06 when
  focused, radius 16. Titlebar: `blend(PANEL_BG_ALT, accent, 0.28)` focused or
  0.10 unfocused. Title `FG` focused and `FG_SOFT` unfocused, 13. Focus adds a
  short 44 x 2.5 accent bar under the title, a 1.8 px border and a 3 px glow.
- The accent is the workspace accent when the panel belongs to a workspace and
  `ACCENT` (focused) or `BORDER_STRONG` (unfocused) otherwise.
- Workspace labels on the canvas use `blend(PANEL_BG_ALT, accent, t)`
  with `t` = 0.08 rest, 0.14 active, 0.18 hover, 0.22 dragging, radius 10, and a
  1 px accent outline (alpha 90 rest, 160 active or hover).
- The sidebar has five groups, from the top to the bottom: the row `Front desk`,
  the group `NEEDS YOU`, the group `WORK ITEMS`, the group `MACHINES` and the
  group `AGENTS`. The footer stays at the bottom edge and does not move. The gap
  between two groups is 14. The groups above the footer scroll when they are
  taller than the room that the footer leaves.
- The row `Front desk` shows only while the orchestrator dock is visible. It is 8
  in from each side, 38 high, has radius 11 and a padding of 10. The gap above it
  is 12. A glyph 16 x 16 stands at its left: a rounded rectangle 12 x 8 with
  radius 2.5 and a top 3 under the top of the glyph, a stem 3 under the centre of
  the rectangle, and a foot 6 wide under the stem, all of them in lines of 1.5 in
  `ACCENT`. The gap after the glyph is 10, then the label `Front desk` in Inter 13
  at weight 620. The state word of the front desk conversation stands at the right
  edge, 10 in, in JetBrains Mono 10 `FG_DIM`, and it is empty while that word is
  `not connected`. While the dock shows the front desk, the row has the fill
  `alpha(ACCENT, 28)` and a 1 px outline `alpha(ACCENT, 102)`, and its label is
  `FG`. Otherwise the row has no fill and no outline, its label is `FG_SOFT`, and
  a hover adds the fill `PANEL_BG_ALT`. A press shows the front desk in the dock,
  as the back control of the header does. The accessible name is `Front desk`.
- Every group heading is a caption: the name in upper case, Inter 10 at weight
  620 in `FG_DIM`, with an extra letter spacing of 1.1, 16 from the left edge of
  the sidebar. The count of the group follows 8 after the name, in JetBrains Mono
  10 at weight 400 in `FG_DIM`; a group with no count shows none. A caption is 22
  high. The gap from a caption to the first row of its group is 6.
- The icon controls of a caption stand at the right edge, 12 in and 4 apart. A
  control is 22 x 22 with radius 7 and no fill. A hover adds the fill
  `PANEL_BG_ALT` and a 1 px `BORDER_SUBTLE` outline. The glyph is drawn in lines
  of 1.5 in `FG_SOFT`, and in `FG` during the hover. There are two glyphs: plus,
  two lines 9 long that cross at the centre; and fit, the four corner marks of a
  square 10 x 10, each one two lines 3 long. Each control has a hover text, and
  the same words are its accessible name.
- The group `NEEDS YOU` keeps the items of the questions. A frame draws the group
  only while a card of the board asks a question, so a window with no question has
  no caption there and no height. A question is a card that is open, whose
  document is not `finished`, and that holds a `callout` with at least one button
  that nobody pressed. The first such callout of the card is its question. The
  group shows the three oldest questions, the oldest first: the time of the last
  update of the card, then the order of the board. The caption carries the count
  of the questions. With more than three questions, the line `and <N> more`
  follows the third item, at 11 pt in `FG_DIM`, 16 from the left edge. Items
  stand 6 apart.
- One item is a box 8 in at each side of the sidebar, radius 12, with a padding
  of 10 on all sides. The fill is `alpha(PALETTE_YELLOW, 23)` and the outline is
  1 px `alpha(PALETTE_YELLOW, 97)`: `PALETTE_YELLOW` is the colour of the
  needs-you dot of a work item row. Line 1 is the name of the workspace in Inter
  12 at weight 620 in `FG`, one line with an ellipsis. Line 2 stands 7 under it:
  the text of the callout in Inter 11.5 `FG_SOFT`, at most two lines with an
  ellipsis. The buttons stand 7 under line 2.
- The buttons of an item are the first two buttons of the callout, in its order.
  A button is 26 high, radius 8, with 10 of padding at each side of its label, and
  its label is Inter 11.5 on one line, cut to 24 characters. The first button is
  `ACCENT` with its label in `BG`, the colour of the window background, at weight
  620; the second is `PANEL_BG_ALT` with a 1 px `BORDER_SUBTLE` outline and its
  label in `FG`. Two buttons stand 6 apart, and a second button that does not fit
  beside the first goes on the row under it, 6 below. A callout with more than two
  buttons adds the control `Open` in the form of the second button.
- A press on a button of an item is the press on that button of the card: it
  records the same click, on the one path of the clicks of a card, and the
  question leaves the group on the next read. A press on the body of the item and
  a press on `Open` show the card: its workspace becomes the active one and the
  canvas reveals the panel, on the path of a workspace row. A pointer over a
  button or over the body shows the pointing hand, and a line that the item cut
  shows its whole text in the hover text. A screen reader says
  `<workspace>: <text>` for the item.
- The group `WORK ITEMS` is the group `Workspaces` of the older form. Its caption
  carries the count of the workspaces and two icon controls: fit, whose hover text
  is `Fit all in view` and whose press is the press of the button `Fit`; and plus,
  whose hover text is `New work item` and whose press is the press of the button
  `New`. The fit takes no press while no workspace is attached to the canvas. The
  text buttons `New` and `Fit` are gone.
- A work item row is 8 in from each side, 46 high, has radius 10 and no outline.
  A press, a drag and a menu of the row keep the rules of the row of the older
  form. The tile is 30 x 30 with radius 9, its fill is
  `blend(PANEL_BG_ALT, accent, 0.26)`, or 0.42 for the active row, and it shows
  the first letters of the first two words of the name in Inter weight 700. A
  work item row shows no badge on its tile: the word under its name carries the
  state.
- The name of a work item row is Inter 13 at weight 620 in `FG`. It keeps the
  whole width of the row, to 8 in from the right edge of the card. The second
  line holds the state word and, 8 after it, the facts of the older form: the
  count of the agents that wait and `on <machine>`, in Inter 12 at weight 400 in
  the secondary colour of the sidebar. The facts show only while 36 or more is
  left for them. A row with no state word and no fact shows the name alone, in
  the middle of the row. The name and the line never hold text of the screen of
  a panel.
- The state word stands at the start of the second line, under the name, in
  Inter 10.5 at weight 400. A dot 6 x 6 of the same colour stands 5 before it.
  The sidebar is narrow, so a word at the right edge of the row would cut the
  name. The word is the
  word of the orchestrator of the workspace when it has one: `working` in
  `ACCENT`, `needs you` in `PALETTE_YELLOW`, `failed` in `PALETTE_RED` and `done`
  in `ACCENT`. A workspace with no word of its orchestrator shows `needs you` or
  `done` when its attention state carries a badge, and it shows no word otherwise.
  The badge `NEW WINDOW` of a detached workspace stands at the right end of the
  row and keeps its rule.
- The active work item row has the fill `alpha(<workspace accent>, 23)` and a bar
  3 wide of the workspace accent at the left edge of the sidebar, from 10 under
  the top of the row to 10 over its bottom, with radius 3 on its two right
  corners. A row under the pointer that is not active has the fill
  `alpha(PANEL_BG_ALT, 128)`. The drag and drop of the rows, their menus and the
  reveal of a press keep their rules.
- The panel rows of a work item are folded at the start of a run. A chevron
  stands at the right end of the row, 6 in: 18 x 18, drawn in lines of 1.5 in
  `FG_DIM` and in `FG` during the hover. It points right while the row is folded
  and down while the row is open. It shows while the pointer is on the row or the
  row is open. Its hover text is `Show panels` or `Hide panels`, and the same
  words are its accessible name. A press on it opens or folds the panel rows of
  that row and does nothing else. The texts of the row end 22 earlier while the
  control shows. A work item with no panel has no control. While the setting of the
  accordion is on, opening one work item folds the others. The fold is a state of
  the run and it is not saved with the session. A panel row keeps its form.
- The group `MACHINES` has the caption `MACHINES` with the count of the machines,
  and no count at zero. Its icon control plus has the hover text `Set up a
  machine`, and its press is the press of the button `Set up`. The text button
  `Set up`, the line of the server and the paragraph `No machines yet. Set up
  rents one, or adds the Horizon server.` are gone from this group; the footer
  says the state of the server.
- A machine row is 8 in from each side, 40 high, has radius 10 and a padding of
  10. A box 24 x 24 with radius 7 and a 1 px `BORDER_SUBTLE` outline stands at its
  left. Inside the box are two rounded rectangles 9 x 3.2 with radius 1, and 1.6
  apart, drawn in lines of 1.3 in `FG_SOFT`; the colour of the state of the
  machine takes the place of `FG_SOFT` when the older row shows a state colour:
  `PALETTE_YELLOW` while it reconnects, `PALETTE_GREEN` when it is connected and
  `PALETTE_RED` when it is disconnected. The gap after the box is 10. The name is
  Inter 12.5 at weight 620 in `FG`; the line under it is Inter 10.5 in the
  secondary colour, and it holds the repository and the provider line of the
  record. The `pending` mark and the summary of a folded machine keep their words.
  The fold control of the agents of the machine stands at the right end of the
  row, 6 in, with the words of the older form. The states, the menus and the
  presses of the row keep their rules, and a hover adds the fill
  `alpha(PANEL_BG_ALT, 128)`. A row that the record gives a price would show it at
  the right edge in JetBrains Mono 10.5 `FG_SOFT` as `<amount>/h`; a record of
  this version carries no price, so the row shows none.
- The control `+ Rent a machine` stands under the rows of the machine section, and
  it stands there with no machine as well. It is 12 in from each side, 34 high,
  has radius 10, no fill, and a 1 px dashed outline in `BORDER_STRONG` with a dash
  of 4 and a gap of 3. The label is Inter 12 in `FG_SOFT`, centred. A hover draws
  the label in `FG` and adds the fill `alpha(PANEL_BG_ALT, 128)`. Its press, its
  disabled states and its hover texts keep the rules of the button of the older
  form.
- The group `AGENTS` has the caption `AGENTS` with no count. The four programs are
  chips that wrap: 14 in from each side, 6 apart, and 6 between two lines. A chip
  is 24 high, has radius 12, a 1 px `BORDER_SUBTLE` outline, and a padding of 8 at
  its left and 10 at its right. A dot 7 x 7 stands at its left, and the name of
  the program follows 6 after the dot, in Inter 11.5 at weight 400. A program of
  this computer has a filled dot in `PALETTE_GREEN` and its name in `FG`. A
  program that this computer does not have, and a program that the search still
  looks for, has a hollow dot with a 1.5 px outline in `FG_DIM` and its name in
  `FG_SOFT`. The hover text is `<name>: on this computer`, `<name>: not found on
  this computer` or `<name>: looking`, and the same words are the accessible name
  of the chip. A chip takes no press.
- The footer stays at the bottom edge of the sidebar, under the scroll area of the
  groups: a 1 px `BORDER_SUBTLE` line across the sidebar, a padding of 10 above
  its line and 12 under it, and 16 at each side. A dot 7 x 7 in `FG_DIM` stands at
  the left, and the line of the server of the older form follows 8 after it, in
  Inter 11 in `FG_DIM`, on one line with an ellipsis; a line that the footer cuts
  shows its whole text in its hover, and the hover also says the reason of the
  last attempt at the link. The link `Set up` stands at the right edge, in Inter
  11 in `ACCENT`, and a hover underlines it. Its press is the press of the button
  `Set up`.

- The agents of a machine hang under its row, in the form and the painter of
  the panel rows of a workspace: a 1 px tree guide (`BORDER_STRONG` alpha 200)
  from the state dot of the machine, the two letters of the kind, the name of
  the last part of the folder of the pane, and at the right the word and the dot
  of the state that a local agent panel shows (`Working ●`, `Needs you`, `Done`,
  or nothing). A row is 30 high, and the rows of the last agent end the guide.
  Instead of the rows, a machine whose link is down shows one line 11 in the
  muted text of a row: `3 agents · trying again`, whose hover says the whole
  sentence `Last seen: 3 agents. Horizon tries again.` A machine with no `tmux`
  shows `No tmux on this machine.`, and a refusal shows its typed reason. A
  machine with no agent shows no row and no line, and its row paints no chevron.
  The row menu of a machine is the one it had.
- A workspace that lives on a machine says so on the second line of its row:
  `on <machine>`, after the count of the agents that wait, as in
  `1 needs you · on <machine>`. The name line is unchanged, so a long name
  cannot cut the machine off the row. The line is low-emphasis Inter 12 weight
  400, is cached with the other row texts, and is read again at once when the
  place of the workspace changes. A workspace on this computer shows the count
  alone, and no second line while no agent waits.
- The workspace menu holds a `Place` section, between `Arrange Panels` and
  `Move to Main Window`/`Open in New Window`: the group label `Place` in 11
  `FG_DIM` with the hint "A new terminal of this workspace opens there. The
  panels that are open stay where they are.", then `This computer` and one row
  for each machine of the list. Rows are 12 pt `FG_SOFT`; the current place
  carries the check mark U+2713 in front of its label. A place whose machine was
  forgotten keeps its checked row, dimmed in `FG_DIM`, with `(gone)` after the
  name and a hover text that says the machine is not in the list; the person can
  pick another place from the same menu.
- The row menu of a machine holds `Put the active workspace here` above the
  connect action, with the hint "Its new terminals open on this machine. The
  panels that are open stay where they are."
- A terminal of a placed workspace carries the titlebar chip of the existing
  card style with the words `on <machine>`: a hollow `FG_SOFT` dot, `BG_ELEVATED`
  fill, `BORDER_SUBTLE` stroke, and the chromeless label in JetBrains Mono at
  the card chip size. A panel that an agent opened for the person keeps its
  `opened by <agent>` chip instead.
- `Close All Panels` in the workspace menu asks first when the workspace holds a
  panel of a machine: the confirm line says `End <n> tmux session(s) of this
  workspace's machine?` in 11 `FG_DIM`, above the red `Close All Panels` action
  and `Cancel`. A workspace with no panel of a machine closes at once.
- Panel titlebar and sidebar "Move to Workspace" menus focus a search field
  once on open. Names filter case-insensitively; a bounded scroll list keeps
  other menu actions reachable as the workspace count grows. Rows use 12 pt
  `FG_SOFT` labels, `FG` for the keyboard selection, and workspace-colored dots.
  The current workspace has a disabled row labeled "(current)". Arrow keys
  select eligible destinations, Enter moves the panel, and Escape cancels.
  Long names truncate with a full-name tooltip. Sidebar menus are sublayers
  above the sidebar chrome, so search fields and results remain clickable.
  The search control is a menu search field.

### Menu search field

A menu search field is the search control at the top of a menu. The **Move to Workspace**
menu uses this field. Paint the well in the menu code. Do not use the default text frame.

The default text frame paints `selection.stroke` when the field has focus. That
stroke is 1 px of `ACCENT` at full opacity. The stroke is the color of selected
text. On a menu, the stroke shows as a hard blue ring. Do not use that ring as
the outline of a control.

The menu fill is `PANEL_BG`. The well is an inset on that fill.

- The well is 32 high and as wide as the menu. The corner radius is 8.
- The fill is `BG`.
- The rest outline is 1 px `alpha(BORDER_STRONG, 200)`, on the inside of the well.
  This outline is only a separator. The inset fill and the search mark identify the control.
- The hover outline is 1 px `alpha(ACCENT, 160)`, on the inside.
- The focus outline is 1 px `alpha(ACCENT, 200)`, on the inside. Contrast of this
  outline on `BG` is 4.44 in the dark theme and 3.58 in the light theme.
- The search mark is 15 from the left edge, at the vertical center. The mark is
  `FG_SOFT`. The mark is `FG` when the field has focus or the query is not empty.
- The text edit uses `Frame::NONE`. Typed text is 13 proportional `FG`. The hint is 12 `FG_DIM`.
- Set `weak_text_color` on the text-edit child to `FG_DIM`. egui replaces a hint
  color with `weak_text_color`, so a color on the hint text has no effect.
- Text starts 28 from the left and stops 10 from the right.
- A click on the mark or on the padding focuses the field. The click does not close the menu.
- The well is not a keyboard focus target. The text edit is the only focus target of the field.
- The menu focuses the field one time, when the menu opens.
- A 1 px line of `alpha(FG, 28)` crosses the top of a focused well. The line
  starts 12 from each side. At rest and on hover the line is `alpha(FG, 16)`.
  The line matches the toolbar search highlight. The line does not replace the focus outline.

### Orchestrator dock

The dock is the conversation column at the right edge of the root window.

The setting `features.orchestrator_dock` decides whether the dock shows. It is on
at first, so a new window starts at the front desk, and a store that holds `false`
keeps it off. The environment switch of a run wins over the setting:
`HORIZON_ORCHESTRATOR_DOCK=1` shows the dock and `HORIZON_ORCHESTRATOR_DOCK=0`
hides it, whatever the setting says. No other value of the variable is a switch.

- The width is 27 percent of the window, between 360 and 540 points.
- The dock keeps the right edge of the window. It starts at the bottom of the
  toolbar and ends at the bottom of the window. An open Settings panel stands to
  the left of the dock.
- The dock does not draw in a frame where the canvas keeps less than 320 points.
  The canvas width is the window width minus the sidebar (210 points when it
  shows), the dock and the Settings panel.
- The surface is `BG_ELEVATED`. A 1 px `BORDER_SUBTLE` line is at its left edge.
  The content stays inside the rectangle of the dock.
- The dock shows one of two conversations: the front desk, which belongs to no
  workspace, or the conversation of a workspace. The header has two forms, and
  each form has a mark at its left or none.
- The header has 16 points of horizontal padding and 12 points of vertical
  padding. A 1 px `BORDER_SUBTLE` line is under it. The name is 13.5 Semibold
  `FG` and ends with an ellipsis. The line under the name is 11.5 Regular
  `FG_SOFT`. It takes the whole width of the header, because the chip and the
  cost stand above it on the row of the name.
- The header of a workspace has the tile of that workspace 26 by 26 points, as
  the row of the workspace list draws it, and the line under the name reads
  `Orchestrator · Claude Code on this computer`.
- The header of a workspace also has a back control 8 points left of the tile,
  which returns the dock to the front desk. It is 22 by 22 points at radius 7.
  It has no fill at rest, and it draws a chevron that points left: two lines of
  1.5 px in `FG_SOFT`, 5 points wide and 9 points high, centred on the control.
  A pointer over it fills the control with `PANEL_BG_ALT` and turns the lines to
  `FG`, and it shows the pointing hand. Its hover text is `Front desk`, and that
  text is also its accessible name. The air between the control and the tile is
  8 points, so the tile, the name and the second line stand 30 points right of
  their place without the control. The name elides earlier by those 30 points.
  Nothing else of the header changes.
- The header of the front desk has no mark: no tile and no back control. Its name
  is `Front desk` at the left padding of the header, and the line under it reads
  `Starts and watches the work items`. The state chip and the cost stay at the
  right edge, as for a workspace.
- The state chip stands at the right edge of the header, on the row of the name,
  in both forms. It has the form of the chip of a card: 20 points high, radius 8,
  a `BG_ELEVATED` fill, a 1 px `BORDER_SUBTLE` outline, 8 points of horizontal
  inset, a dot of 6 points 4 points left of the text, and the text at 10.5
  JetBrains Mono
  `FG_SOFT`. The chip draws the same painter as the chip of a card
  (`block_widget/chip.rs`). The text and the dot of each state are:

  | State | Text | Dot |
  |---|---|---|
  | no bridge yet | `not connected` | hollow `FG_SOFT` |
  | start on the worker | `starting` | filled `PALETTE_YELLOW` |
  | bridge runs, no turn | `ready` | filled `PALETTE_GREEN` |
  | a turn runs | `working` | filled `ACCENT` |
  | the person stopped it | `stopped` | hollow `FG_SOFT` |
  | the program ended | `ended` | filled `PALETTE_RED` |

- The cost of the conversation stands 10 points left of the chip, on the same
  centre line, while it is above zero: the sum of the ended turns with two
  decimals and `USD`, as `0.03 USD`, at 11 JetBrains Mono and `FG_SOFT`. A header
  that is too narrow for the name, the cost and the chip cuts the name with an
  ellipsis first, and then hides the cost. The chip stays.
- The empty state is vertically centred between the header and the composer,
  with 28 points of horizontal padding. Its title is 15 Bold `FG`. The two
  sentences under the title are 13.5 Regular `FG_SOFT`. They wrap at a line
  height of 1.55 times the size. It draws only while the conversation that the
  dock shows holds no entry.
- The empty state of a workspace has the title `Say what you want done.`, and the
  two sentences read `The orchestrator reads the repository, makes a plan and
  shows it as cards. It asks before a machine is rented.`
- The empty state of the front desk has the title `What should be done?`, and the
  two sentences read `Write one message with one task or more. The front desk
  starts a work item for each task and tells you when one needs you.`
- Two example rows stand under the two sentences, 12 points below them and 8
  points apart. The rows of a workspace are `Fix #1987 on a rented machine` and
  `Why is the cold start slow? Measure it first.`. The rows of the front desk are
  `Fix #1987 and make the cold start faster` and `What waits
  for me?`. A row takes the full width of the empty state block, with radius 10,
  a `PANEL_BG` fill, a 1 px `BORDER_SUBTLE` outline, a padding of 12 by 9 points,
  and its text at 12.5 Regular `FG` on one line with an ellipsis. A pointer over
  a row outlines it in `FG_DIM` and shows the pointing hand. A click puts the
  text of that row into the empty draft of the conversation that the dock shows
  and gives the keyboard to the field. A draft that holds text stays as the
  person wrote it. The click sends nothing.
- The list is a vertical scroll area between the header and the composer, with
  16 points of padding and 16 points between two entries. New entries stand at
  the bottom, and the list follows the bottom while the person has not scrolled
  up. One frame lays out at most the last 200 entries, and one notice above them
  names the entries that it does not draw.
- An entry of the person is a bubble at the right of the list, at most 86
  percent of the list width: a `PANEL_BG_ALT` fill, a 1 px `BORDER_SUBTLE`
  outline, radius 12, a padding of 12 by 9 points, and the text at 13.5 `FG`
  with a line height of 1.5 times the size, wrapped.
- An entry of the agent stands at the left of the list, at most 96 percent of
  its width: a bar of 3 points along the full height of the text, 10 points of
  air, and the text at 13.5 `FG` with a line height of 1.55 times the size,
  wrapped. The bar is a vertical gradient from `ACCENT` at its top to
  `workspace_accent(6)`, the mauve of the workspace accents, at its bottom. It is
  a mesh of two colours with a round cap of 1.5 points at each end. The text is
  plain text: no markdown, no link and no path is active, and the person selects
  and copies it.
- An entry of a tool is one row, indented 13 points: a dot of 6 points, the name
  of the tool at 12 JetBrains Mono `FG_SOFT`, and what the tool was asked to do
  at 12 JetBrains Mono `FG_DIM`, cut with an ellipsis to one line. The dot is
  `ACCENT` while the tool runs, `PALETTE_GREEN` when it ends and `PALETTE_RED`
  when it fails. A tool that failed draws the first line of its result under the
  request, in the column of the request: 11.5 JetBrains Mono `PALETTE_RED`, one
  line with an ellipsis. A tool that is done draws no such line. Two tool rows
  that follow each other stand 4 points apart.
- An entry of a notice of Horizon is centred at 10.5 JetBrains Mono `FG_DIM`,
  with a 1 px `BORDER_SUBTLE` line at each side that takes the free width.
- An entry of a card is one reference row, indented 13 points like a tool row and
  at most 96 percent of the list width: one row of 34 points, radius 10, a fill of
  `alpha(ACCENT, 18)`, a 1 px outline of `alpha(ACCENT, 115)` and 10 points of
  padding at each side. The glyph of a card stands at its left: 22 by 16 points, a
  rounded rectangle of radius 4 with a 1.5 px `ACCENT` outline and one 1.5 px
  `ACCENT` line 3 points under its top edge, inset 4 points at each side. 10
  points right of the glyph is the heading of the card at 12.5 Medium `FG`, on one
  line with an ellipsis. At the right end, 10 points inside the row, reads
  `on the canvas` at 11 Regular `FG_DIM`. A pointer over the row outlines it in
  `ACCENT` and shows the pointing hand, and a press brings the card into view. A
  row whose card is no longer on the board has no fill, an outline of
  `BORDER_SUBTLE`, its heading in `FG_DIM` and `closed` at its right end, and a
  press on it does nothing.
- An entry of a work item that the front desk started is one reference row of the
  form and the size of a card row, with the mark of its workspace in place of the
  glyph of a card: a tile of 20 by 20 points, painted by the one helper of a
  workspace tile, so a person reads one mark for one workspace. 10 points right of
  the tile is the name of the work item at 12.5 Medium `FG`, on one line with an
  ellipsis. At the right end, 10 points inside the row, reads the state word of
  that work item in lower case, `idle`, `working`, `needs_you`, `failed` or
  `done`, at 11 Regular `FG_DIM`. A pointer over the row outlines it in `ACCENT`
  and shows the pointing hand, and a press shows that workspace in the dock and
  makes it the active one. A row whose workspace is gone has no fill, an outline
  of `BORDER_SUBTLE`, its name and its tile in `FG_DIM`, no state word, and a
  press on it does nothing.
- An entry of a proposal of the front desk is one box and the controls of the
  person, indented 13 points like a tool row, at the full width of the list minus
  those 13 points. The box has radius 12, 12 points of padding, a fill of
  `alpha(ACCENT, 18)` and a 1 px outline of `alpha(ACCENT, 115)`.
  - The caption `The front desk wants to start` at 11 Regular `FG_DIM` stands at
    the top padding.
  - 8 points under it starts the first item, and each next item follows 8 points
    after the one before it. An item holds the title at 12.5 Semibold `FG` on one
    line with an ellipsis, under it the first line of the task at 11.5 Regular
    `FG_SOFT` in at most 2 lines, and, when the item names a folder, one more line
    with the folder at 10.5 JetBrains Mono `FG_DIM`, cut with an ellipsis at its
    left so the end of the path shows.
  - 10 points under the last item stand the two controls while the person did not
    decide. Each is 28 points high at radius 8 with 12 points of horizontal
    padding and a label at 12 Semibold; 8 points of air stand between them. The
    first control reads `Start 1 work item` or `Start <N> work items`, and it has
    an `ACCENT` fill with its label in `BG`. The second reads `Not now`, and it
    has a `PANEL_BG_ALT` fill, a 1 px `BORDER_SUBTLE` outline and its label in
    `FG`. A pointer over a control turns its outline to `FG_DIM` for the second
    control and deepens the fill of the first, and it shows the pointing hand.
  - A proposal that the person decided draws no control: 10 points under its last
    item stands one line at 11.5 Regular `FG_DIM`, which reads `Started`,
    `Not started` or `Replaced by a newer proposal`. A proposal that a restart
    loaded as open reads `Not started` and takes no press, because the program
    that proposed it is gone.
- A message of the person that the front desk passed carries the caption
  `via the front desk` above its bubble: 10.5 Regular `FG_DIM`, right aligned with
  the right edge of the bubble, 2 points above it.
- While a turn is busy and no text of the agent came for it, one row stands at
  the left: the spinner of the interface and `working` at 12 `FG_DIM` after it.
  Only a frame that draws this row asks for the frame of the next step.
- The composer keeps the bottom edge of the dock. From its top down: a 1 px
  `BORDER_SUBTLE` line, 12 points of padding, the field, 10 points of gap, the
  row of the chips, and 14 points of padding to the bottom edge. The left and
  right padding is 16 points. The composer of the front desk has no row of
  chips: the field takes the place of that row, and the 14 points of bottom
  padding follow the field.
- The field takes the full width of the composer. It is a multiline text edit at
  radius 12, with a `BG` fill and a 1 px `BORDER_STRONG` outline, which becomes a
  1 px `ACCENT` outline while the field holds the keyboard. Its hint is
  `Write to the orchestrator`, and `Write to the front desk` for the front desk.
  Its text is 13.5 `FG`, with a margin of 14 points
  at its left and 12 points above and below it. The text ends 8 points before the
  control. The field is at least 48 points high, which holds one row. It grows to
  6 rows upward, and it clips a longer draft. The row of the chips keeps its
  place at the bottom.
- The control stands inside the field at its right end: 34 by 34 points at
  radius 9, 7 points from the right inner edge of the field. Its vertical centre
  is the centre of a field of one row, and a taller draft keeps it 7 points above
  the bottom inner edge. The control shows what a press will do:
  - A draft that can go out, and no turn that runs: a filled `ACCENT`, the arrow
    glyph in `BG`, and the pointing hand. A pointer over it fills it with
    `blend(ACCENT, FG, 0.12)`.
  - An empty draft, or no active workspace: a `PANEL_BG_ALT` fill, a 1 px
    `BORDER_SUBTLE` outline and the arrow glyph in `FG_DIM`. With no active
    workspace the hover text is `The orchestrator is not connected yet`.
  - A turn that runs: the stop form of a filled square, a `PANEL_BG_ALT` fill, a
    1 px `BORDER_STRONG` outline and the glyph in `FG`.

  Enter in the field sends the draft, Shift+Enter makes a new line, and a click on
  the control sends it. A draft of white space only sends nothing. While the
  conversation is busy a press stops the program of that conversation; the field
  keeps the keys, and Enter then sends nothing.
- The three read-only chips are under the field of a workspace, left aligned and
  8 points apart. The front desk draws no chip and keeps no room for one.
  They wrap to the next row when the dock is too narrow for them. A chip has
  radius 8, a `PANEL_BG_ALT` fill, a 1 px `BORDER_SUBTLE` outline and a padding of
  10 by 5 points. Its label line is 10.5 `FG_DIM` and its value line is 12.5 `FG`.
  A chevron of 5 points in `FG_DIM` stands 6 points right of the value, because a
  later part makes the chips a choice. The chips are read-only in this part, and
  the hover text of each of them is `Choice comes later`. The rows are
  `Orchestrator` / `Claude Code`, `Workers` / `not chosen` and `Place` / `this
  computer`.
- The start and the stop of a bridge run on the worker of the dock, a thread named
  `orchestrator-dock`, and never on the frame. The worker is separate from the
  coordination worker of the browser jobs, so a stop never delays a browser job.
- The minimap hangs on the bottom right corner of the canvas. The dock and an
  open Settings panel never stand on it. The map keeps the rule of the minimap:
  it draws only in a canvas of at least three times its width and twice its
  height, which is 996 by 384 points for the default map of 332 by 192 points.
  The dock makes the canvas narrower, so a narrow window keeps no map beside the
  dock.
- The field takes the keyboard only after a click in it. The dock takes no other
  widget of the window, so a terminal panel keeps the keys when the dock appears.
- The dock shows one conversation at a time. The start of Horizon shows the front
  desk. A press on a row of the workspace list, also on the row that is already
  active, and every change of the active workspace show that workspace. The back
  control of the header and the command `Show Front Desk` show the front desk. A
  board without a workspace shows the front desk.
- While the dock is visible and shows the front desk, the workspace list draws no
  row as active and opens no panel row under a workspace. The active workspace of
  the board and the focus of the canvas do not change: this is a rule of the
  drawing. A dock that is not visible leaves the list as it is.

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
- The microphone icon of speech input in the title bar of a panel: red with a
  pulsing ring while the take of that panel records, and pulsing amber while that
  take is transcribed. It is one small shape, so a frame of it lays out no text.
- The pill of speech input: one sentence at the bottom left of the panel of a
  take, drawn while the sentence stays. Its frames are a take that pulses and one
  frame at the end of the six seconds of a sentence of a failure.
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
- Do not give a menu search field the default text frame. That frame paints a
  1 px full-opacity `ACCENT` ring when the field has focus.

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
