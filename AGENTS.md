# RockCast agent instructions

## Graphify

Use Graphify only when the user explicitly invokes `/graphify` or when a genuinely large,
cross-repository architecture investigation needs relationship traversal that `rg` and targeted file
reading cannot provide. For routine code search, roadmap/status questions, concrete bugs, and
single-repository work, use `rg` and read only the relevant files.

Do not run `graphify update .` after ordinary changes. Never use Graphify merely because
`graphify-out/` exists or is dirty.

After every Graphify invocation, including failure, timeout, or interruption, inspect Python
processes created by that invocation. Gracefully stop only processes positively identified as
Graphify-owned and verify they exited. Never terminate unrelated Python processes.

## UI assets

Never draw UI icons or glyphs with painter code (egui shapes, vector helpers, emoji text
glyphs). If a graphic can be an asset, it must be an asset: add it to
`scripts/generate_icons.py`, generate the PNG into `assets/icon_*.png`, and render it through
`src/app/icons.rs` as a tinted texture.

Static icons are drawn in near-white `(243, 238, 233)` on transparency, like the existing
icons: the UI tints textures at draw time (ACCENT when idle, WHITE on hover), so colored
assets multiply with the tint into an unreadable blob.

