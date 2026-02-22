# Security Implementation Notes (Backend)

This document lists every security-relevant behavior implemented in the backend, including items required by the project plan and additional safeguards added during implementation. It is written from a “zero‑knowledge” and production‑safety perspective.

**Zero‑Knowledge Data Handling (Project Plan Requirement)**
- Webhook payloads are never persisted to disk or logged. The server builds a JSON payload in memory and immediately encrypts/sends it via Web Push. (`src/handlers.rs`, `src/push.rs`)
- The only data stored in `redb` is the PushSubscription, creation timestamp, and delete token. No webhook body or headers are stored server‑side. (`src/models.rs`, `src/db.rs`)
- The Web Push protocol encryption (ECDH + HKDF + AES‑128‑GCM) is performed by the `web-push` crate, so payloads are end‑to‑end encrypted for the browser. The server only sees plaintext in memory while encrypting. (`src/push.rs`)

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
- No global IP-based rate limiting or bot protection (CAPTCHA) for `/api/subscribe` or `/hook`.
  Reason: initial scope focuses on per-UUID limits and keeping the backend lightweight; global rate limiting adds state and tuning requirements.
- No content-type allowlist on webhook requests (any content-type is accepted).
  Reason: webhook providers send varied content-types; strict allowlists risk breaking legitimate payloads without a clear benefit for Phase 1–3.
- No explicit sanitization of header values before sending to the browser; the frontend must escape/encode before rendering.
  Reason: server is “zero‑knowledge” and avoids mutating payloads; UI should treat all fields as untrusted and escape on render.
- No encryption-at-rest for the subscription database (`redb` file stored in plaintext on disk).
  Reason: stored data is only subscription metadata (no webhook bodies); encryption-at-rest is an infrastructure decision (disk encryption / KMS).
- No audit log for failed delete-token attempts or other suspicious activity.
  Reason: logging is minimized to avoid any risk of sensitive data exposure; can be added if operational monitoring is required.
- TLS termination is assumed to be handled by a reverse proxy/host; the server itself does not manage certificates.
  Reason: standard production deployments terminate TLS at the edge (nginx, Caddy, Cloudflare, etc.) rather than in the app.
