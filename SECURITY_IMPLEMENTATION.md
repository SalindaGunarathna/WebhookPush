# Security Implementation Notes (Backend)

This document lists every security-relevant behavior implemented in the backend, including items required by the project plan and additional safeguards added during implementation. It is written from a “zero‑knowledge” and production‑safety perspective.

**Zero‑Knowledge Data Handling (Project Plan Requirement)**
- Webhook payloads are never persisted to disk or logged. The server builds a JSON payload in memory and immediately encrypts/sends it via Web Push. (`backend/src/handlers.rs`, `backend/src/push.rs`)
- The only data stored in `redb` is the PushSubscription, creation timestamp, and delete token. No webhook body or headers are stored server‑side. (`backend/src/models.rs`, `backend/src/db.rs`)
- The Web Push protocol encryption (ECDH + HKDF + AES‑128‑GCM) is performed by the `web-push` crate, so payloads are end‑to‑end encrypted for the browser. The server only sees plaintext in memory while encrypting. (`backend/src/push.rs`)

**Transport Security**
- `PUBLIC_BASE_URL` must be HTTPS for any non‑localhost deployment; the server fails fast if misconfigured. Localhost `http://` is allowed for development. (`backend/src/main.rs`)
- Subscription endpoints must be HTTPS and parse as valid URIs. (`backend/src/handlers.rs`)

**Origin & Browser Exposure Controls**
- CORS is configurable and defaults to `http://localhost:3000`. For production, only the frontend origin should be listed. (`backend/src/config.rs`, `backend/src/main.rs`)
- `CORS_ORIGINS=*` is supported but explicit; it is never the default. (`backend/src/config.rs`)

**Subscription Validation (Input Security)**
- Subscription endpoint length capped (2048 chars) to avoid oversized inputs. (`backend/src/handlers.rs`)
- Only known push service hosts are accepted (allowlist) to reduce SSRF and off‑target delivery. Default allowlist includes:
  `fcm.googleapis.com`, `updates.push.services.mozilla.com`, `wns.windows.com`, `notify.windows.com`, `web.push.apple.com`. (`backend/src/config.rs`, `backend/src/handlers.rs`)
- `p256dh` and `auth` keys are base64url‑decoded and validated for exact byte lengths (65 and 16). (`backend/src/handlers.rs`)

**Authentication / Authorization**
- Unsubscribe requires a delete token via `X-Delete-Token` header; it is not exposed in the URL to avoid leak via logs or referrers. (`backend/src/handlers.rs`)
- Delete tokens are random UUIDv4 values (CSPRNG), stored server‑side and never recalculated. (`backend/src/handlers.rs`, `backend/src/models.rs`)

**Abuse Prevention & DoS Resistance**
- Rate limiting is enforced per UUID to prevent webhook spam. (`backend/src/rate_limiter.rs`, `backend/src/handlers.rs`)
- Maximum webhook payload size is enforced (`MAX_PAYLOAD_BYTES`, default 100KB). Requests larger than this return `413`. (`backend/src/config.rs`, `backend/src/handlers.rs`)
- Request body read is bounded by both size and timeout (`WEBHOOK_READ_TIMEOUT_MS`), mitigating slowloris‑style attacks. (`backend/src/handlers.rs`, `backend/src/config.rs`)
- `/api/subscribe` is capped with `DefaultBodyLimit` to prevent large subscription bodies. (`backend/src/main.rs`)

**Push Delivery Safety**
- Web Push responses indicating expired or invalid subscriptions automatically delete the stored subscription (cleanup of dead endpoints). (`backend/src/push.rs`, project plan section 11)
- Push payloads are chunked to stay under 4KB limits; each chunk is individually encrypted. (`backend/src/handlers.rs`)
- A small inter‑chunk delay (default 50ms) reduces push service throttling risks. (`backend/src/config.rs`, `backend/src/handlers.rs`)

**Data Retention**
- Subscriptions are automatically purged based on TTL (`SUBSCRIPTION_TTL_DAYS`). (`backend/src/db.rs`, `backend/src/main.rs`)

**Secrets & Configuration Hygiene**
- VAPID private key and other secrets are loaded from environment variables; `.env` files are excluded from git. (`backend/src/config.rs`, `.gitignore`)
- `.env.example` contains placeholders only (no secrets). (`backend/.env.example`)

**Logging**
- Error logs do not include webhook bodies or headers. Logs include only operational errors (push failures, cleanup failures, expired subscriptions). (`backend/src/push.rs`, `backend/src/main.rs`)

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
