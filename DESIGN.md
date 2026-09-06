---
version: alpha
colors:
  ink: "#17211f"
  accent: "#0c766c"
  accentDark: "#07574f"
  surface: "#ffffff"
  soft: "#edf4f0"
  line: "#d9e3de"
  danger: "#b43d3d"
  warning: "#a7641a"
typography:
  sans:
    fontFamily: "Inter, ui-sans-serif, system-ui, sans-serif"
  data:
    fontFamily: "ui-monospace, SFMono-Regular, monospace"
rounded:
  card: "12px"
  control: "7px"
spacing:
  page: "clamp(24px, 5vw, 72px)"
components:
  status: { textAndColor: true }
  dialog: { focus: "app-owned confirmation" }
---

## Overview

Sift Console is a local operator tool for maintaining code indexes. It should
feel like a quiet instrument panel: dense enough for paths, commits, and
counters, with clear action boundaries around expensive or destructive work.
The interface is a product surface for one local operator, not a marketing
page.

## Colors

Deep green establishes the local service and teal marks safe interaction.
Danger and warning colors always appear with text labels and are reserved for
failed operations, removal, rebuild cost, or uncertain freshness. Dark mode
changes surfaces and contrast while keeping the semantic roles stable.

## Typography

System sans-serif keeps the console readable across local installations.
Paths, commit identifiers, and measurements use a monospace face so values can
be scanned and compared without ornamental typography.

## Layout

The desktop layout uses a compact dark navigation rail and a spacious working
canvas. Repository details use two columns for freshness and actions, then
full-width reports. At 800px the rail becomes a top navigation row; at 390px
cards stack and actions remain full width.

## Elevation & Depth

Panels use a restrained border and a soft light-theme shadow. Dark mode relies
on surface contrast and removes the shadow so diagnostic content remains calm.

## Shapes

Cards use a 12px radius and controls use a 7px radius. Status badges are pills
because they are compact labels; buttons and forms are not pill-shaped.

## Components

Forms preserve values through server errors and focus the first invalid field.
Dialogs are app-owned, keyboard reachable, and describe the exact consequence
before a rebuild or registration removal. Indeterminate indexing progress never
inventes a percentage. Null resource values render as Unavailable.

## Do's and Don'ts

- Do label commit alignment separately from working-tree freshness.
- Do make expensive operations explicit and reversible where possible.
- Do keep source contents and query text out of the console UI history.
- Don't present matching HEAD as proof that all working-tree changes are indexed.
- Don't use color alone to communicate lifecycle or operation outcome.
