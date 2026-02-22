const metaApiBase = document.querySelector('meta[name="api-base"]');
const apiBase = metaApiBase && metaApiBase.content ? metaApiBase.content : '';

const DB_NAME = 'webhookpush';
const DB_VERSION = 1;
const REQUESTS_STORE = 'requests';
const PENDING_STORE = 'pending_chunks';
const MAX_TEST_BYTES = 100 * 1024;
const MAX_TEST_COUNT = 25;
const MAX_TEST_DELAY_MS = 10_000;

const els = {
  subscribeBtn: document.getElementById('subscribe-btn'),
  unsubscribeBtn: document.getElementById('unsubscribe-btn'),
  status: document.getElementById('subscription-status'),
  webhookUrl: document.getElementById('webhook-url'),
  copyUrlBtn: document.getElementById('copy-url-btn'),
  clearBtn: document.getElementById('clear-btn'),
  requestList: document.getElementById('request-list'),
  requestEmpty: document.getElementById('request-empty'),
  detailEmpty: document.getElementById('detail-empty'),
  detailView: document.getElementById('detail-view'),
  detailMethod: document.getElementById('detail-method'),
  detailPath: document.getElementById('detail-path'),
  detailTime: document.getElementById('detail-time'),
  detailSize: document.getElementById('detail-size'),
  detailHeaders: document.getElementById('detail-headers'),
  detailBody: document.getElementById('detail-body'),
  detailNote: document.getElementById('detail-note'),
  copyHeadersBtn: document.getElementById('copy-headers-btn'),
  copyBodyBtn: document.getElementById('copy-body-btn'),
  copyFullBtn: document.getElementById('copy-full-btn'),
  testSize: document.getElementById('test-size'),
  testCount: document.getElementById('test-count'),
  testDelay: document.getElementById('test-delay'),
  testToggleBtn: document.getElementById('test-toggle-btn'),
  testBody: document.getElementById('test-body'),
  testRunBtn: document.getElementById('test-run-btn'),
  testOutput: document.getElementById('test-output'),
  deliveryOutput: document.getElementById('delivery-output'),
};

let currentSubscription = null;
let requestsCache = [];
let selectedId = null;

init();

async function init() {
  bindEvents();
  await registerServiceWorker();
  currentSubscription = loadStoredSubscription();
  await refreshRequests();
  updateSubscriptionUI();
  attachServiceWorkerMessages();
}

function bindEvents() {
  els.subscribeBtn.addEventListener('click', onSubscribe);
  els.unsubscribeBtn.addEventListener('click', onUnsubscribe);
  els.copyUrlBtn.addEventListener('click', () => copyText(els.webhookUrl.textContent));
  els.clearBtn.addEventListener('click', clearHistory);
  els.copyHeadersBtn.addEventListener('click', () => copyText(els.detailHeaders.textContent));
  els.copyBodyBtn.addEventListener('click', () => copyText(els.detailBody.textContent));
  els.copyFullBtn.addEventListener('click', () => {
    const selected = requestsCache.find((item) => item.id === selectedId);
    if (selected) {
      copyText(JSON.stringify(selected, null, 2));
    }
  });
  els.testToggleBtn.addEventListener('click', toggleTestPanel);
  els.testRunBtn.addEventListener('click', runTest);
  els.testSize.addEventListener('input', validateTestInputs);
  els.testCount.addEventListener('input', validateTestInputs);
  els.testDelay.addEventListener('input', validateTestInputs);
}

async function registerServiceWorker() {
  if (!('serviceWorker' in navigator)) {
    showStatus('Service workers not supported in this browser.', true);
    return;
  }
  try {
    await navigator.serviceWorker.register('/sw.js', { scope: '/' });
    await navigator.serviceWorker.ready;
  } catch (err) {
    showStatus('Failed to register service worker.', true);
    console.error(err);
  }
}

function attachServiceWorkerMessages() {
  if (!('serviceWorker' in navigator)) return;
  navigator.serviceWorker.addEventListener('message', async (event) => {
    if (event.data?.type === 'new-request') {
      await refreshRequests();
      selectRequest(event.data.id);
      if (event.data.partial) {
        setDeliveryOutput(
          `Delivery: partial data received for request ${event.data.id}.`,
          true
        );
      } else if (event.data.id) {
        setDeliveryOutput(
          `Delivery: complete request received (${event.data.id}).`,
          false
        );
      }
    }
  });
}

async function onSubscribe() {
  if (!('Notification' in window) || !('serviceWorker' in navigator)) {
    showStatus('Push notifications are not supported in this browser.', true);
    return;
  }

  const permission = await Notification.requestPermission();
  if (permission !== 'granted') {
    showStatus('Notification permission is required to subscribe.', true);
    return;
  }

  try {
    const config = await fetchJson(`${apiBase}/api/config`);
    const appServerKey = urlBase64ToUint8Array(config.public_key);
    const registration = await navigator.serviceWorker.ready;
    let subscription = await registration.pushManager.getSubscription();
    if (!subscription) {
      subscription = await registration.pushManager.subscribe({
        userVisibleOnly: true,
        applicationServerKey: appServerKey,
      });
    }

    const response = await fetch(`${apiBase}/api/subscribe`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(subscription),
    });

    if (!response.ok) {
      const errText = await response.text();
      throw new Error(errText || 'Subscription failed');
    }

    const data = await response.json();
    currentSubscription = data;
    storeSubscription(data);
    updateSubscriptionUI();
    showStatus('Subscribed and ready for webhooks.', false);
  } catch (err) {
    console.error(err);
    showStatus(`Subscribe failed: ${err.message}`, true);
  }
}

async function onUnsubscribe() {
  if (!currentSubscription) return;

  try {
    const response = await fetch(
      `${apiBase}/api/subscribe/${currentSubscription.uuid}`,
      {
        method: 'DELETE',
        headers: { 'X-Delete-Token': currentSubscription.delete_token },
      }
    );

    if (!response.ok && response.status !== 404) {
      const errText = await response.text();
      throw new Error(errText || 'Unsubscribe failed');
    }

    const registration = await navigator.serviceWorker.ready;
    const subscription = await registration.pushManager.getSubscription();
    if (subscription) {
      await subscription.unsubscribe();
    }

    clearStoredSubscription();
    currentSubscription = null;
    updateSubscriptionUI();
    showStatus('Unsubscribed.', false);
  } catch (err) {
    console.error(err);
    showStatus(`Unsubscribe failed: ${err.message}`, true);
  }
}

async function refreshRequests() {
  requestsCache = await getAllRequests();
  requestsCache.sort((a, b) => b.timestamp.localeCompare(a.timestamp));
  renderRequestList();
  if (selectedId) {
    selectRequest(selectedId);
  }
}

function renderRequestList() {
  els.requestList.innerHTML = '';
  if (!requestsCache.length) {
    els.requestEmpty.classList.remove('hidden');
    return;
  }
  els.requestEmpty.classList.add('hidden');

  requestsCache.forEach((item) => {
    const li = document.createElement('li');
    li.className = 'request-item';
    li.dataset.id = item.id;

    const badge = document.createElement('span');
    const method = item.partial ? 'PARTIAL' : (item.method || 'UNKNOWN');
    badge.textContent = method;
    badge.className = `badge ${method.toLowerCase()}`;
    if (item.partial) badge.classList.add('partial');

    const main = document.createElement('div');
    main.className = 'request-main';

    const path = document.createElement('div');
    path.className = 'path';
    path.textContent = item.partial
      ? item.note || 'Partial delivery'
      : buildPath(item.path, item.query_string);

    const meta = document.createElement('div');
    meta.className = 'request-meta';
    meta.textContent = `${formatTimestamp(item.timestamp)} • ${formatSize(
      item.content_length || 0
    )}`;

    main.appendChild(path);
    main.appendChild(meta);

    li.appendChild(main);
    li.appendChild(badge);

    li.addEventListener('click', () => selectRequest(item.id));

    els.requestList.appendChild(li);
  });
}

function selectRequest(id) {
  selectedId = id;
  const selected = requestsCache.find((item) => item.id === id);
  if (!selected) return;

  els.detailEmpty.classList.add('hidden');
  els.detailView.classList.remove('hidden');

  const method = selected.partial ? 'PARTIAL' : selected.method;
  els.detailMethod.textContent = method || 'UNKNOWN';
  els.detailMethod.className = `badge ${method?.toLowerCase() || ''}`;
  if (selected.partial) els.detailMethod.classList.add('partial');

  els.detailPath.textContent = selected.partial
    ? selected.note || 'Partial delivery'
    : buildPath(selected.path, selected.query_string);

  els.detailTime.textContent = formatTimestamp(selected.timestamp);
  els.detailSize.textContent = formatSize(selected.content_length || 0);

  if (selected.partial) {
    els.detailNote.textContent = selected.note || 'Incomplete payload received.';
    els.detailNote.classList.remove('hidden');
  } else {
    els.detailNote.classList.add('hidden');
  }

  els.detailHeaders.textContent = formatHeaders(selected.headers || {});
  els.detailBody.textContent = formatBody(selected.body || '');
}

async function clearHistory() {
  const db = await openDb();
  await clearStore(db, REQUESTS_STORE);
  requestsCache = [];
  selectedId = null;
  renderRequestList();
  els.detailView.classList.add('hidden');
  els.detailEmpty.classList.remove('hidden');
}

async function runTest() {
  if (!validateTestInputs()) return;

  const size = Math.max(1, Number(els.testSize.value) || 0);
  const count = Math.max(1, Number(els.testCount.value) || 0);
  const delay = Math.max(0, Number(els.testDelay.value) || 0);
  const url = `${apiBase}/hook/${currentSubscription.uuid}`;

  setTestOutput(`Sending ${count} payload(s) of ~${size} bytes...`, false);

  for (let i = 1; i <= count; i += 1) {
    const payload = buildPayload(size);
    try {
      const response = await fetch(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: payload,
      });
      setTestOutput(
        `Request ${i}/${count}: HTTP ${response.status}`,
        response.status >= 400
      );
    } catch (err) {
      console.error(err);
      setTestOutput(`Request ${i}/${count}: failed to send`, true);
    }

    if (delay && i < count) {
      await new Promise((resolve) => setTimeout(resolve, delay));
    }
  }
}

function buildPayload(bytes) {
  const stamp = new Date().toISOString();
  const blob = 'a'.repeat(Math.max(0, bytes));
  return JSON.stringify({ timestamp: stamp, payload: blob });
}

function setTestOutput(message, isError) {
  els.testOutput.textContent = message;
  els.testOutput.style.color = isError ? '#b02d2d' : '';
}

function setDeliveryOutput(message, isError) {
  els.deliveryOutput.textContent = message;
  els.deliveryOutput.style.color = isError ? '#b02d2d' : '';
}

function updateSubscriptionUI() {
  if (currentSubscription) {
    els.status.textContent = 'Subscribed';
    els.webhookUrl.textContent = currentSubscription.url;
    els.copyUrlBtn.disabled = false;
    els.unsubscribeBtn.classList.remove('hidden');
    els.subscribeBtn.classList.add('hidden');
  } else {
    els.status.textContent = 'Not subscribed';
    els.webhookUrl.textContent = 'Subscribe to generate your URL.';
    els.copyUrlBtn.disabled = true;
    els.unsubscribeBtn.classList.add('hidden');
    els.subscribeBtn.classList.remove('hidden');
  }
  validateTestInputs();
}

function showStatus(message, isError) {
  els.status.textContent = message;
  els.status.style.color = isError ? '#b02d2d' : '';
}

function toggleTestPanel() {
  const isHidden = els.testBody.classList.toggle('hidden');
  els.testToggleBtn.textContent = isHidden ? 'Open Test' : 'Hide Test';
  if (!isHidden) {
    setTestOutput('', false);
    setDeliveryOutput('', false);
    validateTestInputs();
  }
}

function validateTestInputs() {
  if (!els.testBody || els.testBody.classList.contains('hidden')) {
    return false;
  }

  if (!currentSubscription?.uuid) {
    els.testRunBtn.disabled = true;
    setTestOutput('Subscribe first to generate a webhook URL.', true);
    return false;
  }

  const size = Number(els.testSize.value);
  const count = Number(els.testCount.value);
  const delay = Number(els.testDelay.value);

  if (!Number.isFinite(size) || size < 1) {
    els.testRunBtn.disabled = true;
    setTestOutput('Payload size must be at least 1 byte.', true);
    return false;
  }
  if (size > MAX_TEST_BYTES) {
    els.testRunBtn.disabled = true;
    setTestOutput(`Payload size must be <= ${MAX_TEST_BYTES} bytes.`, true);
    return false;
  }
  if (!Number.isFinite(count) || count < 1) {
    els.testRunBtn.disabled = true;
    setTestOutput('Requests to send must be at least 1.', true);
    return false;
  }
  if (count > MAX_TEST_COUNT) {
    els.testRunBtn.disabled = true;
    setTestOutput(`Requests to send must be <= ${MAX_TEST_COUNT}.`, true);
    return false;
  }
  if (!Number.isFinite(delay) || delay < 0) {
    els.testRunBtn.disabled = true;
    setTestOutput('Delay must be 0 or greater.', true);
    return false;
  }
  if (delay > MAX_TEST_DELAY_MS) {
    els.testRunBtn.disabled = true;
    setTestOutput(`Delay must be <= ${MAX_TEST_DELAY_MS} ms.`, true);
    return false;
  }

  els.testRunBtn.disabled = false;
  if (els.testOutput.textContent.startsWith('Payload size must') ||
      els.testOutput.textContent.startsWith('Requests to send') ||
      els.testOutput.textContent.startsWith('Delay must') ||
      els.testOutput.textContent.startsWith('Subscribe first')) {
    setTestOutput('', false);
  }
  return true;
}

function storeSubscription(data) {
  localStorage.setItem('webhookpush_subscription', JSON.stringify(data));
}

function loadStoredSubscription() {
  const raw = localStorage.getItem('webhookpush_subscription');
  if (!raw) return null;
  try {
    return JSON.parse(raw);
  } catch {
    return null;
  }
}

function clearStoredSubscription() {
  localStorage.removeItem('webhookpush_subscription');
}

async function fetchJson(url) {
  const response = await fetch(url);
  if (!response.ok) {
    const errText = await response.text();
    throw new Error(errText || 'Request failed');
  }
  return response.json();
}

function formatHeaders(headers) {
  const entries = Object.entries(headers);
  if (!entries.length) return 'No headers';
  return entries.map(([key, value]) => `${key}: ${value}`).join('\n');
}

function formatBody(body) {
  if (!body) return 'No body';
  try {
    const parsed = JSON.parse(body);
    return JSON.stringify(parsed, null, 2);
  } catch {
    return body;
  }
}

function buildPath(path, query) {
  if (!query) return path || '/';
  return `${path}?${query}`;
}

function formatTimestamp(value) {
  if (!value) return 'Unknown time';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString();
}

function formatSize(bytes) {
  if (!bytes) return '0 B';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

async function copyText(text) {
  if (!text) return;
  try {
    await navigator.clipboard.writeText(text);
    showStatus('Copied to clipboard.', false);
  } catch {
    showStatus('Copy failed.', true);
  }
}

function urlBase64ToUint8Array(base64String) {
  const padding = '='.repeat((4 - (base64String.length % 4)) % 4);
  const base64 = (base64String + padding)
    .replace(/-/g, '+')
    .replace(/_/g, '/');
  const rawData = atob(base64);
  const outputArray = new Uint8Array(rawData.length);
  for (let i = 0; i < rawData.length; ++i) {
    outputArray[i] = rawData.charCodeAt(i);
  }
  return outputArray;
}

function openDb() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, DB_VERSION);
    request.onupgradeneeded = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains(REQUESTS_STORE)) {
        const store = db.createObjectStore(REQUESTS_STORE, { keyPath: 'id' });
        store.createIndex('timestamp', 'timestamp', { unique: false });
        store.createIndex('method', 'method', { unique: false });
      }
      if (!db.objectStoreNames.contains(PENDING_STORE)) {
        const store = db.createObjectStore(PENDING_STORE, {
          keyPath: ['request_id', 'chunk_index'],
        });
        store.createIndex('request_id', 'request_id', { unique: false });
      }
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

function getAllRequests() {
  return openDb().then(
    (db) =>
      new Promise((resolve, reject) => {
        const tx = db.transaction(REQUESTS_STORE, 'readonly');
        const store = tx.objectStore(REQUESTS_STORE);
        const req = store.getAll();
        req.onsuccess = () => resolve(req.result || []);
        req.onerror = () => reject(req.error);
      })
  );
}

function clearStore(db, storeName) {
  return new Promise((resolve, reject) => {
    const tx = db.transaction(storeName, 'readwrite');
    tx.objectStore(storeName).clear();
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
  });
}
