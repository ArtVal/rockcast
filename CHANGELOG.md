# Changelog

## Unreleased

- RM-004-F: RockCast vendors the approved schema-v1 baseline catalog release
  2026.08.2 (sha256: 3fa20dca94fc059bd433a47b9fba9bb6d5e5e1aa2957a5ffb58b2a7b20b1d74d).
  The local-first loader verifies its manifest, version, and canonical checksum before use,
  preserves the primary stream playback URL, and retains alternate stream metadata.
- RM-004-F: JSON overrides now follow the existing environment, executable, current-directory,
  and app-data source precedence. Legacy TXT overrides remain available only through RM-004-I:
  remove them after one schema-v1 release cycle, never before 2026-10-31.
