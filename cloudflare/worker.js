const UUID_PATH = /^\/[0-9a-f]{12}$/i;

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const path = url.pathname;

    if (isBackendPath(path)) {
      return proxyTo(request, env.BACKEND_ORIGIN);
    }

    return proxyTo(request, env.PAGES_ORIGIN);
  },
};

function isBackendPath(path) {
  if (path === '/health') return true;
  if (path === '/api' || path.startsWith('/api/')) return true;
  if (path === '/hook' || path.startsWith('/hook/')) return true;
  if (UUID_PATH.test(path)) return true;
  return false;
}

function proxyTo(request, origin) {
  const targetUrl = new URL(request.url);
  const originUrl = new URL(origin);
  targetUrl.protocol = originUrl.protocol;
  targetUrl.host = originUrl.host;

  const proxiedRequest = new Request(targetUrl.toString(), request);
  return fetch(proxiedRequest);
}
