# Changelog

## Unreleased

- Bound the Cast LAN relay to two simultaneous HTTP clients; excess clients receive HTTP 503
  without a waiting queue. This is a resource cap for one receiver plus the optional spectrum tap,
  not multi-receiver playback support.
- Allow the playback soak example to use real RockServer station IDs through
  `ROCKCAST_SOAK_<CODEC>_ID`.
- Switch to server-driven catalog and RockServer-only WebP icons: removed bundled
  `assets/catalog/*` files and local catalog loading. Initial catalog loads directly
  from RockServer (<300 ms) with rock station priority, with pagination and search across
  all 16,000+ stations. Station icons load strictly from RockServer WebP endpoints when
  `favicon_url` is provided, removing third-party homepage favicon scraping and blind 404s.


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

## Superseded catalog implementation notes

The following RM-004 entries describe the earlier local-first catalog implementation. RockCast
later transitioned to the server-driven catalog described above and in [status](docs/status.md).

- RM-004-F: RockCast vendors the approved schema-v1 baseline catalog release
  2026.08.2 (sha256: 3fa20dca94fc059bd433a47b9fba9bb6d5e5e1aa2957a5ffb58b2a7b20b1d74d).
  The local-first loader verifies its manifest, version, and canonical checksum before use,
  preserves the primary stream playback URL, and retains alternate stream metadata.
- RM-004-F: JSON overrides now follow the existing environment, executable, current-directory,
  and app-data source precedence.
- RM-004-I: Completed retirement of the legacy `stations.txt` transition adapter; catalog overrides
  exclusively use `stations.v1.json`.
