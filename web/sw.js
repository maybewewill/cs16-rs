const CACHE_NAME = 'cs16-rs-shell-v5';
const SHELL_ASSETS = [
  './',
  './index.html',
  './app.js',
  './vfs_loader.js',
  './transport.js',
  './input.js',
  './audio.js',
  './manifest.webmanifest',
  './favicon.ico',
  './icons/favicon-32.png',
  './icons/apple-touch-icon.png',
  './icons/icon-192.png',
  './icons/icon-512.png',
  './pkg/cs_web.js',
  './pkg/cs_web_bg.wasm',
  './playground/',
  './playground/index.html',
  './playground/playground.js'
];

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches.open(CACHE_NAME).then((cache) =>
      Promise.allSettled(SHELL_ASSETS.map((url) => cache.add(url)))
    )
  );
  self.skipWaiting();
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches.keys().then((keys) =>
      Promise.all(
        keys
          .filter((key) => key !== CACHE_NAME)
          .map((key) => caches.delete(key))
      )
    )
  );
  self.clients.claim();
});

function isPortalSubpath(pathname) {
  return (
    pathname.includes('/playground/') ||
    pathname.endsWith('/playground') ||
    pathname.includes('/docs/') ||
    pathname.endsWith('/docs') ||
    pathname.includes('/mods/') ||
    pathname.endsWith('/mods')
  );
}

self.addEventListener('fetch', (event) => {
  if (event.request.method !== 'GET') return;
  const url = new URL(event.request.url);
  if (url.origin !== self.location.origin) return;

  // Never intercept API calls or return stale root fallbacks for /playground/, /docs/, /mods/.
  if (url.pathname.includes('/api/')) return;

  const networkFirst =
    event.request.mode === 'navigate' ||
    isPortalSubpath(url.pathname) ||
    url.pathname.endsWith('.html') ||
    url.pathname.endsWith('.json');

  if (networkFirst) {
    event.respondWith(
      fetch(event.request)
        .then((response) => {
          if (response && response.status === 200) {
            const copy = response.clone();
            caches.open(CACHE_NAME).then((cache) => cache.put(event.request, copy));
          }
          return response;
        })
        .catch(() => caches.match(event.request))
    );
    return;
  }

  event.respondWith(
    caches.match(event.request).then((cached) => {
      const fetched = fetch(event.request)
        .then((response) => {
          if (response && response.status === 200) {
            const copy = response.clone();
            caches.open(CACHE_NAME).then((cache) => cache.put(event.request, copy));
          }
          return response;
        })
        .catch(() => cached);
      return cached || fetched;
    })
  );
});
