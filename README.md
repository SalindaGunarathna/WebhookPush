# WebhookPush

WebhookPush is a zero‑knowledge webhook testing tool. The server accepts incoming webhooks, encrypts them using the browser’s push subscription keys, and delivers them via Web Push. The server never stores webhook payloads.

Security details are documented in `SECURITY_IMPLEMENTATION.md`.

**Core Idea**
1. Browser subscribes to Web Push and sends a `PushSubscription` to the server.
2. Server stores only the subscription metadata and returns a short webhook URL.
3. Any HTTP request sent to that URL is serialized, chunked, encrypted, and pushed to the browser.
4. The browser decrypts and stores the webhook locally (IndexedDB, frontend phase).

**Tech Stack**
- Rust 2024 + Axum
- `redb` embedded KV store (single file)
- `web-push` for RFC‑compliant encryption and delivery
- `tokio` async runtime

## Setup (Windows)

Required:
1. **Rust toolchain**
```powershell
winget install -e --id Rustlang.Rustup
```
(Alternative: install from https://rustup.rs)

2. **Visual Studio Build Tools (C++ toolchain)**
```powershell
winget install -e --id Microsoft.VisualStudio.2022.BuildTools
```
Make sure **MSVC v143** and **Windows 10/11 SDK** are selected.

3. **OpenSSL via vcpkg** (required by `web-push`)
```powershell
cd C:\
git clone https://github.com/microsoft/vcpkg
.\vcpkg\bootstrap-vcpkg.bat
.\vcpkg\vcpkg install openssl:x64-windows-static-md
.\vcpkg\vcpkg integrate install
setx VCPKG_ROOT "C:\vcpkg"
```
(Alternative: prebuilt OpenSSL binaries and set `OPENSSL_DIR`, but vcpkg is recommended.)

4. **Restart your terminal**, then confirm:
```powershell
rustc --version
cargo --version
```

Optional:
- **Node.js** (only if you plan to build a separate frontend later)
- **ngrok** (for external webhook testing)

## Example Flow (Local)

1. Create `.env` (copy from `.env.example`) and set:
   - `VAPID_PUBLIC_KEY`
   - `VAPID_PRIVATE_KEY`
   - `PUBLIC_BASE_URL=http://localhost:3000`
   - Local development can use HTTP on `localhost`; production must use HTTPS.

   Generate VAPID keys (example using the `web-push` CLI):
   ```bash
   npx web-push generate-vapid-keys
   ```
   Then copy the keys into `.env`.

2. Start server (backend serves the frontend from the same origin):
```bash
cargo run
```

3. Open the UI:
```
http://localhost:3000
```

4. Fetch config (optional check):
```bash
curl http://localhost:3000/api/config
```

5. Subscribe from browser (frontend generates real `PushSubscription`).

6. Send a test webhook:
```bash
curl -X POST http://localhost:3000/hook/<uuid> \
  -H "Content-Type: application/json" \
  -d '{"hello":"world"}'
```

7. You can also test directly from the UI using the **Test Webhook** panel.

## Endpoints

**GET `/`**
- Simple landing response confirming backend is running.
- Response: `200 OK` (HTML)

**GET `/health`**
- Liveness check.
- Response: `200 OK`

**GET `/api/config`**
- Returns the VAPID public key used by the frontend to create subscriptions.
- Response `200 OK`:
```json
{
  "public_key": "BEl62iUYgUivxIkv69yViEuiBIa-Ib9-SkvMeAtA3LFgDzkGs..."
}
```

**POST `/api/subscribe`**
- Stores a browser `PushSubscription` and returns a short webhook URL.
- Request body:
```json
{
  "endpoint": "https://fcm.googleapis.com/fcm/send/...",
  "expirationTime": null,
  "keys": {
    "p256dh": "BNcRdreALRFXTkOOUHK1EtK2wtaz5Ry4YfYCA_0QTpQtUb...",
    "auth": "tBHItJI5svbpC7htP8Nw=="
  }
}
```
- Response `200 OK`:
```json
{
  "uuid": "a1b2c3d4e5f6",
  "url": "http://localhost:3000/a1b2c3d4e5f6",
  "delete_token": "f1d2d2f924e986ac86fdf7b36c94bcdf"
}
```
Notes:
- This endpoint expects a real browser subscription. Hand‑crafted values will fail validation.
- Request body size is capped (8KB by default).

**DELETE `/api/subscribe/:uuid`**
- Deletes a subscription.
- Requires header `X-Delete-Token`.
- Response:
  - `204 No Content` on success
  - `401 Unauthorized` if token missing
  - `403 Forbidden` if token invalid
  - `404 Not Found` if UUID missing

**ANY `/hook/:uuid`** and **ANY `/:uuid`**
- Accepts incoming webhooks (any method).
- Serializes the request, chunks to fit push payload limits, encrypts, and sends via Web Push.
- Response codes:
  - `200 OK` delivered
  - `404 Not Found` unknown UUID
  - `413 Payload Too Large` exceeds `MAX_PAYLOAD_BYTES`
  - `429 Too Many Requests` rate limit exceeded
  - `502 Bad Gateway` push service rejected or subscription expired



## Automated Testing (Payload + Chunking)

Use the PowerShell helper script to send payloads of specific sizes:

1. Small payload (no chunking):
```powershell
.\scripts\test_webhook.ps1 -Uuid <uuid> -Bytes 1024
```

2. Large payload (forces chunking but stays under 100KB):
```powershell
.\scripts\test_webhook.ps1 -Uuid <uuid> -Bytes 50000
```

3. Repeat multiple times:
```powershell
.\scripts\test_webhook.ps1 -Uuid <uuid> -Bytes 2048 -Count 5
```

## Environment Configuration

See `.env.example` for a full template.

Required:
- `VAPID_PUBLIC_KEY`: public VAPID key for the server.
- `VAPID_PRIVATE_KEY`: private VAPID key for signing.

Recommended:
- `PUBLIC_BASE_URL`: the public origin for webhook URLs.
  - Local dev: `http://localhost:3000`
  - Production: must be `https://...` for non‑localhost
- `CORS_ORIGINS`: comma‑separated list of allowed frontend origins. Default: `http://localhost:3000`.
- `ALLOWED_PUSH_HOSTS`: allowlist for push service endpoints.

Optional tuning:
- `MAX_PAYLOAD_BYTES` (default 102400)
- `CHUNK_DATA_BYTES` (default 2400)
- `CHUNK_DELAY_MS` (default 50)
- `SUBSCRIPTION_TTL_DAYS` (default 30)
- `RATE_LIMIT_PER_MINUTE` (default 60)
- `WEBHOOK_READ_TIMEOUT_MS` (default 3000)
- `DB_PATH` (default `webhookpush.redb`)
- `BIND_ADDR` (default `0.0.0.0:3000`)
- `STATIC_DIR` (default `frontend`)

## Security Notes

All security decisions, safeguards, and known gaps are documented in:
- `SECURITY_IMPLEMENTATION.md`
