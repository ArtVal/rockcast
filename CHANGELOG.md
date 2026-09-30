# Changelog

## Unreleased

- Fix station table horizontal overflow: strict column geometry clamping to viewport bounds
  (`compute_table_geometry`), responsive column layout, and two-row chip filter layout.
- Fix scroll controls: floating «Наверх» (top) and «К играющей» (locate playing) buttons now render
  in a dedicated foreground area without event drop-through; playing station resolution prioritizes
  active `playback_station_id`. New PNG asset `icon_locate.png`.
- Fix voice search pagination: voice search queries now automatically trigger catalog search with
  infinite scroll support, preserve the playing station at the top of the search results, and
  display full total matching counts instead of locking to 10 candidates.
- RC-7 UI polish: unified type scale and 16 px margins; 44 px station rows with always-visible
  play buttons and an animated now-playing row; redesigned player deck (transport on the panel
  axis, full-height clickable spectrum, volume moved into the status footer); card-style
  «Account & devices» window; UI icons are PNG assets only; fixed ICY track titles being
  overwritten by the placeholder hint right after start.

- RM-004-F: RockCast vendors the approved schema-v1 baseline catalog release
  2026.08.2 (sha256: 3fa20dca94fc059bd433a47b9fba9bb6d5e5e1aa2957a5ffb58b2a7b20b1d74d).
  The local-first loader verifies its manifest, version, and canonical checksum before use,
  preserves the primary stream playback URL, and retains alternate stream metadata.
- RM-004-F: JSON overrides now follow the existing environment, executable, current-directory,
  and app-data source precedence.
- RM-004-I: Completed retirement of the legacy `stations.txt` transition adapter; catalog overrides
  exclusively use `stations.v1.json`.
