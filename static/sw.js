// Offline support. Every file is taken from the network when there is one, so a new
// build shows up at once, and from this cache when there is not. Online games
// (addresses under api/) always go to the server.
const CACHE = 'sixtyfour-1';
const PIECES = ['wK', 'wQ', 'wR', 'wB', 'wN', 'wP', 'bK', 'bQ', 'bR', 'bB', 'bN', 'bP'];
const SOUNDS = ['move1', 'move2', 'move3', 'move4', 'move5', 'clack1', 'clack2', 'clack3', 'tick1', 'tick2', 'thump', 'topple', 'slide', 'setup'];
const FILES = [
  './', 'pkg/app.js', 'pkg/app_bg.wasm', 'worker.js', 'manifest.webmanifest', 'icon.svg', 'icon-192.png', 'icon-512.png',
  ...PIECES.flatMap((p) => [`pieces/${p}.svg`, `pieces/merida/${p}.svg`, `pieces/chessnut/${p}.svg`]),
  ...SOUNDS.map((s) => `sounds/${s}.wav`),
];

self.addEventListener('install', (e) => {
  self.skipWaiting();
  // A file that fails to download must not stop the rest from being kept.
  e.waitUntil(caches.open(CACHE).then((c) => Promise.all(FILES.map((f) => c.add(f).catch(() => {})))));
});

self.addEventListener('activate', (e) => {
  e.waitUntil(
    caches.keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener('fetch', (e) => {
  const url = new URL(e.request.url);
  if (e.request.method !== 'GET' || url.origin !== location.origin || url.pathname.includes('/api/')) return;
  e.respondWith(
    fetch(e.request)
      .then((r) => {
        if (r.ok) {
          const copy = r.clone();
          caches.open(CACHE).then((c) => c.put(e.request, copy));
        }
        return r;
      })
      .catch(() => caches.match(e.request, { ignoreSearch: true }).then((r) => r || Response.error())),
  );
});
