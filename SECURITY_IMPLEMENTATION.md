# Security Implementation Notes (Backend)

This document lists every security-relevant behavior implemented in the backend, including items required by the project plan and additional safeguards added during implementation. It is written from a “zero‑knowledge” and production‑safety perspective.

**Zero‑Knowledge Data Handling (Project Plan Requirement)**
- Webhook payloads are stored **temporarily** in a disk‑backed queue until delivery and then deleted. Payloads are never logged. (`src/handlers.rs`, `src/queue.rs`)
- Subscription metadata (PushSubscription, creation timestamp, delete token) is stored in the main `redb` file; the disk queue uses a **separate** `redb` file for temporary payloads. (`src/models.rs`, `src/db.rs`, `src/queue.rs`)
- The Web Push protocol encryption (ECDH + HKDF + AES‑128‑GCM) is performed by the `web-push` crate, so payloads are end‑to‑end encrypted for the browser. The server only sees plaintext in memory while encrypting. (`src/push.rs`)
Note: temporary disk queuing trades strict zero‑knowledge for reliability under load; payloads persist only until delivery.

**Transport Security**
- `PUBLIC_BASE_URL` must be HTTPS for any non‑localhost deployment; the server fails fast if misconfigured. Localhost `http://` is allowed for development. (`src/main.rs`)
- Subscription endpoints must be HTTPS and parse as valid URIs. (`src/handlers.rs`)

**Origin & Browser Exposure Controls**
- CORS is configurable and defaults to `http://localhost:3000`. For production, only the frontend origin should be listed. (`src/config.rs`, `src/main.rs`)
- `CORS_ORIGINS=*` is supported but explicit; it is never the default. (`src/config.rs`)

**Subscription Validation (Input Security)**
- Subscription endpoint length capped (2048 chars) to avoid oversized inputs. (`src/handlers.rs`)
- Only known push service hosts are accepted (allowlist) to reduce SSRF and off‑target delivery. Default allowlist includes:
  `fcm.googleapis.com`, `updates.push.services.mozilla.com`, `wns.windows.com`, `notify.windows.com`, `web.push.apple.com`. (`src/config.rs`, `src/handlers.rs`)
- `p256dh` and `auth` keys are base64url‑decoded and validated for exact byte lengths (65 and 16). (`src/handlers.rs`)

**Authentication / Authorization**
- Unsubscribe requires a delete token via `X-Delete-Token` header; it is not exposed in the URL to avoid leak via logs or referrers. (`src/handlers.rs`)
- Delete tokens are random UUIDv4 values (CSPRNG), stored server‑side and never recalculated. (`src/handlers.rs`, `src/models.rs`)

**Abuse Prevention & DoS Resistance**
- Rate limiting is enforced per UUID to prevent webhook spam. (`src/rate_limiter.rs`, `src/handlers.rs`)
- Maximum webhook payload size is enforced (`MAX_PAYLOAD_BYTES`, default 100KB). Requests larger than this return `413`. (`src/config.rs`, `src/handlers.rs`)
- Request body read is bounded by both size and timeout (`WEBHOOK_READ_TIMEOUT_MS`), mitigating slowloris‑style attacks. (`src/handlers.rs`, `src/config.rs`)
- `/api/subscribe` is capped with `DefaultBodyLimit` to prevent large subscription bodies. (`src/main.rs`)

**Push Delivery Safety**
- Web Push responses indicating expired or invalid subscriptions automatically delete the stored subscription (cleanup of dead endpoints). (`src/push.rs`, project plan section 11)
- Push payloads are chunked to stay under 4KB limits; each chunk is individually encrypted. (`src/handlers.rs`)
- A small inter‑chunk delay (default 50ms) reduces push service throttling risks. (`src/config.rs`, `src/handlers.rs`)

**Data Retention**
- Subscriptions are automatically purged based on TTL (`SUBSCRIPTION_TTL_DAYS`). (`src/db.rs`, `src/main.rs`)

**Secrets & Configuration Hygiene**
- VAPID private key and other secrets are loaded from environment variables; `.env` files are excluded from git. (`src/config.rs`, `.gitignore`)
- `.env.example` contains placeholders only (no secrets). (`.env.example`)

**Logging**
- Error logs do not include webhook bodies or headers. Logs include only operational errors (push failures, cleanup failures, expired subscriptions). (`src/push.rs`, `src/main.rs`)

**Project Plan Security Coverage**
- End‑to‑end encrypted delivery via Web Push is implemented. (Plan §2.2, §2.2.4)
- Short URL mapping via server‑side storage is implemented, with deletion and TTL cleanup. (Plan §3, §9)
- Payload chunking (~3KB data chunks, JSON envelope) is implemented. (Plan §5)
- Rate limiting per UUID and correct response codes are implemented. (Plan §8.1, §9 Phase 3)
- Push subscription expiry handling (delete on push error) is implemented. (Plan §11, §9 Phase 3)

If you want any of these tightened (longer UUIDs, stronger delete tokens, IP-based rate limits, or stricter CORS), we can add them, but the current implementation matches the plan and adds additional safeguards beyond it.

**Not Yet Implemented / Security Gaps to Track (with Reasons)**
- No global IP-based rate limiting or bot protection (CAPTCHA) for `/api/subscribe` or `/hook`.<br>
  Reason: we intentionally kept rate limiting per‑UUID to minimize shared state and tuning complexity; global IP limits are an operational decision.

- No content-type allowlist on webhook requests (any content-type is accepted).<br>
  Reason: webhook providers use varied content-types; strict allowlists would reject valid payloads and reduce compatibility.

- No explicit sanitization of header values before sending to the browser; the frontend must escape/encode before rendering.<br>
  Reason: to preserve zero‑knowledge semantics the server does not transform payloads; UI must treat all fields as untrusted and escape on render.

- No encryption-at-rest for the subscription database or the disk queue (`redb` files stored in plaintext on disk).<br>
  Reason: encryption-at-rest is best handled by disk‑level encryption/KMS; adding app‑level encryption increases complexity and CPU cost.

- No audit log for failed delete-token attempts or other suspicious activity.<br>
  Reason: logging is intentionally minimal to reduce exposure risk; audit logging adds data handling and retention concerns.

- TLS termination is assumed to be handled by a reverse proxy/host; the server itself does not manage certificates.<br>
  Reason: production deployments typically terminate TLS at the edge (nginx, Caddy, Cloudflare) rather than inside the app.
