## en
- **Host Hub Sync**: Added `GET /api/native/v1/hub/snapshot`, `PUT/DELETE /api/native/v1/hub/profiles/{id}`, and `GET /api/native/v1/hub/covers/{app_id}` (`hub.sync`) with optimistic revision concurrency (`If-Match` / `409 revision_conflict`), lazy SHA-256 cover caching (`304 Not Modified`), per-device isolation, and read-only host override visibility.
