import {
  cacheAssetsInDb,
  clearAssetDb,
  hydrateAssetsFromDb,
  mountFromDirectoryPicker,
  mountFromDropEvent,
  mountFromFileList
} from './vfs_loader.js';
import { WebGameTransport } from './transport.js';
import { WebInputController } from './input.js';
import { WebAudioMixer } from './audio.js';

const canvas = document.getElementById('cs16-canvas');
const statusEl = document.getElementById('status-pill');
const assetStatusEl = document.getElementById('asset-status');
const mapSelect = document.getElementById('map-select');
const botsInput = document.getElementById('bots-count');
const wsUrlInput = document.getElementById('ws-url');
const masterUrlInput = document.getElementById('master-url');
const serverListSelect = document.getElementById('server-list');
const panelEl = document.getElementById('control-panel');

let cs16Web = null;
let net = null;
let audio = null;
let input = null;

function setStatus(text) {
  if (statusEl) statusEl.textContent = text;
}

function setAssetStatus(text) {
  if (assetStatusEl) assetStatusEl.textContent = text;
}

function refreshMaps() {
  if (!cs16Web || !mapSelect) return;
  const maps = cs16Web.listMaps?.() || [];
  mapSelect.innerHTML = '';
  for (const m of maps) {
    const opt = document.createElement('option');
    opt.value = m;
    opt.textContent = m;
    if (m === 'de_dust2') opt.selected = true;
    mapSelect.appendChild(opt);
  }
}

async function mountBatch(loaderPromise) {
  if (!cs16Web) return;
  setAssetStatus('Mounting Half-Life folder...');
  const mounted = await loaderPromise;
  cs16Web.finishMount?.();
  await cacheAssetsInDb(mounted);
  refreshMaps();
  setAssetStatus(`Mounted ${mounted.length} files (${cs16Web.listMaps?.().length || 0} maps)`);
}

function ensureSize() {
  if (!canvas || !cs16Web) return;
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const w = Math.max(1, Math.floor(window.innerWidth * dpr));
  const h = Math.max(1, Math.floor(window.innerHeight * dpr));
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
    cs16Web.resize?.(w, h);
  }
}

async function main() {
  if ('serviceWorker' in navigator) {
    navigator.serviceWorker.register('./sw.js').catch(() => {});
  }

  try {
    const wasmMod = await import('./pkg/cs_web.js');
    await wasmMod.default();
  } catch (err) {
    setStatus('WASM bundle missing — run: cargo xtask web-build');
    console.warn('Could not load ./pkg/cs_web.js:', err);
    return;
  }

  cs16Web = window.cs16Web;
  if (!cs16Web) {
    setStatus('cs16Web runtime failed to register');
    return;
  }

  ensureSize();
  window.addEventListener('resize', ensureSize);

  try {
    await cs16Web.init(canvas);
    setStatus(`Ready (${cs16Web.rendererInfo?.() || 'WebGPU/WebGL2'})`);
  } catch (err) {
    setStatus(`GPU init failed: ${err}`);
  }

  audio = new WebAudioMixer((path) => cs16Web.readFile?.(path));
  input = new WebInputController(canvas, cs16Web, {
    onGesture: () => {
      audio.unlock();
      cs16Web.unlockAudio?.();
    }
  });

  const cached = await hydrateAssetsFromDb(
    (p, b) => cs16Web.mountFile?.(p, b),
    (n) => setAssetStatus(`Loading cached assets (${n})...`)
  );
  if (cached > 0) {
    cs16Web.finishMount?.();
    refreshMaps();
    setAssetStatus(`Restored ${cached} cached assets`);
  }

  document.getElementById('btn-pick-dir')?.addEventListener('click', () => {
    mountBatch(
      mountFromDirectoryPicker(
        (p, b) => cs16Web.mountFile?.(p, b),
        (n, cur) => setAssetStatus(`Reading ${n}: ${cur}`)
      )
    ).catch((err) => setAssetStatus(`Picker failed: ${err.message}`));
  });

  const dirInput = document.getElementById('webkit-dir-input');
  document.getElementById('btn-pick-fallback')?.addEventListener('click', () => dirInput?.click());
  dirInput?.addEventListener('change', (e) => {
    const files = e.target.files;
    if (files && files.length > 0) {
      mountBatch(
        mountFromFileList(
          files,
          (p, b) => cs16Web.mountFile?.(p, b),
          (n, cur) => setAssetStatus(`Reading ${n}: ${cur}`)
        )
      ).catch((err) => setAssetStatus(`Import failed: ${err.message}`));
    }
  });

  document.getElementById('btn-clear-cache')?.addEventListener('click', async () => {
    await clearAssetDb();
    setAssetStatus('IndexedDB cache cleared');
  });

  window.addEventListener('dragover', (e) => e.preventDefault());
  window.addEventListener('drop', (e) => {
    e.preventDefault();
    mountBatch(
      mountFromDropEvent(
        e,
        (p, b) => cs16Web.mountFile?.(p, b),
        (n, cur) => setAssetStatus(`Dropped ${n}: ${cur}`)
      )
    ).catch((err) => setAssetStatus(`Drop failed: ${err.message}`));
  });

  document.getElementById('btn-start-local')?.addEventListener('click', () => {
    audio.unlock();
    cs16Web.unlockAudio?.();
    const map = mapSelect?.value || 'de_dust2';
    const bots = Math.max(0, Math.min(15, Number(botsInput?.value || 4)));
    cs16Web.startLocal?.(map, bots);
    panelEl?.classList.add('collapsed');
    input.lockPointer();
    setStatus(`Offline: ${map} (${bots} bots)`);
  });

  document.getElementById('btn-connect')?.addEventListener('click', async () => {
    audio.unlock();
    cs16Web.unlockAudio?.();
    const url = wsUrlInput?.value?.trim();
    if (!url) return;
    net?.close();
    cs16Web.connectRemote?.();
    net = new WebGameTransport(url, {
      onPacket: (pkt) => cs16Web.pushNetPacket?.(pkt),
      onStateChange: (st) => setStatus(`Net: ${st}`)
    });
    await net.connect();
    panelEl?.classList.add('collapsed');
    input.lockPointer();
  });

  document.getElementById('btn-disconnect')?.addEventListener('click', () => {
    net?.close();
    net = null;
    cs16Web.disconnect?.();
    panelEl?.classList.remove('collapsed');
    setStatus('Disconnected');
  });

  document.getElementById('btn-refresh-servers')?.addEventListener('click', async () => {
    const endpoint = masterUrlInput?.value?.trim();
    if (!endpoint || !serverListSelect) return;
    setStatus('Querying master server...');
    try {
      const resp = await fetch(endpoint);
      if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
      const list = await resp.json();
      serverListSelect.innerHTML = '';
      if (!Array.isArray(list) || list.length === 0) {
        const opt = document.createElement('option');
        opt.value = '';
        opt.textContent = '(no servers registered)';
        serverListSelect.appendChild(opt);
      } else {
        for (const s of list) {
          const opt = document.createElement('option');
          const host = String(s.addr || '').split(':')[0] || '127.0.0.1';
          opt.value = `ws://${host}:27016/ws`;
          opt.textContent = `${s.addr} — ${s.map || '?'} (${s.players ?? 0}/${s.max ?? 0})`;
          serverListSelect.appendChild(opt);
        }
        if (wsUrlInput && serverListSelect.options[0]?.value) {
          wsUrlInput.value = serverListSelect.options[0].value;
        }
      }
      setStatus(`Master: ${Array.isArray(list) ? list.length : 0} server(s)`);
    } catch (err) {
      setStatus(`Master error: ${err.message}`);
    }
  });

  serverListSelect?.addEventListener('change', () => {
    if (wsUrlInput && serverListSelect.value) {
      wsUrlInput.value = serverListSelect.value;
    }
  });

  document.getElementById('btn-fullscreen')?.addEventListener('click', () => input.toggleFullscreen());
  document.getElementById('btn-toggle-panel')?.addEventListener('click', () => {
    panelEl?.classList.toggle('collapsed');
  });

  for (const el of document.querySelectorAll('[data-touch-bit]')) {
    const bit = Number(el.getAttribute('data-touch-bit'));
    el.addEventListener('touchstart', (e) => {
      e.preventDefault();
      audio.unlock();
      cs16Web.unlockAudio?.();
      input.touchButtons |= bit;
    });
    el.addEventListener('touchend', (e) => {
      e.preventDefault();
      input.touchButtons &= ~bit;
    });
  }

  function frame(nowMs) {
    input.pollGamepadAndTouch();
    cs16Web.stepAndRender?.(nowMs);
    const outPackets = cs16Web.takeNetPackets?.();
    if (net && Array.isArray(outPackets)) {
      for (const pkt of outPackets) {
        net.send(pkt);
      }
    }
    const audioEvents = cs16Web.takeAudioEvents?.();
    if (audio && audioEvents && audioEvents.length > 0) {
      audio.processEvents(audioEvents);
    }
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
}

main();
