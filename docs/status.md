# RockCast status

## Station icons MVP

Implemented for the pre-RockServer-icon phase (2026-08-26).

- RockCast fetches a valid station `favicon_url` directly from the station's
  HTTP(S) server. If that field is absent, it may fetch the conventional
  `/favicon.ico` from the configured official `homepage_url`; it does not
  scrape homepage HTML.
- Fetching, bounded response reads, image decoding, and disk cache I/O run in
  the existing `BackgroundRuntime`, never on the egui thread.
- ICO, JPEG, and PNG payloads are accepted, bounded to 512 KiB on the wire and
  decoded to a maximum 64px thumbnail. Invalid, oversized, unsupported, or
  failed payloads keep the existing text-only station row.
- Successful thumbnails are cached in the platform app-data directory under
  `station-icons`. The cache filename is a safe hex-encoded station key and
  the stored source URL invalidates stale metadata. Requests are attempted at
  most once per station/source identity per app session.
- RockServer and voice station DTO adapters preserve optional `homepage` and
  `favicon` fields for this client-side MVP. No RockServer endpoint or database
  migration is part of this change.

The embedded catalog currently has no homepage/favicon metadata for its
stations, so those rows intentionally remain text-only until catalog or
RockServer metadata supplies a permitted source URL.

## MVP-001-C — zero-configuration official RockServer client

Implemented and locally verified on 2026-08-26.

- Official releases use `https://alex.vault57.ru` without user configuration.
- Public search uses `POST /v1/search` without Bearer authorization. Voice
  preserves TLS by mapping HTTPS to WSS and uses `/v1/voice/stream`, also
  without Bearer authorization.
- RockServer URL/token controls and persisted RockServer settings were removed.
  Legacy JSON fields are ignored and scrubbed during settings migration.
- Endpoint, optional Bearer token, and streaming-mode overrides exist only for
  debug/test runtime through `ROCKCAST_DEV_ROCKSERVER_*`; release builds ignore
  them, and their values are neither displayed nor logged.
- The embedded catalog is delivered before the public request. A failed or
  empty public response continues through the existing local catalog + Radio
  Browser path, so local selection and playback do not depend on RockServer.

The client follows the deployed RockServer runtime contract from MVP-001-B.
Legacy `/api/v1` aliases remain intentionally unused because they are
Bearer-protected. No RockServer or OpenAPI repository was changed here. If the
published OpenAPI still applies global Bearer security to these allowlisted
`/v1` operations, that documentation/runtime mismatch remains an external
contract-documentation issue, not a reason for the client to send a token.
