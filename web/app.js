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

function parseInviteParams(search, pageProtocol) {
  const params = new URLSearchParams(search || '');
  const rawJoin = (params.get('join') || '').trim();
  const rawRoom = (params.get('room') || '').trim();
  const room = /^[A-Za-z0-9_-]{1,32}$/.test(rawRoom) ? rawRoom : '';
  if (!rawJoin) {
    return { wsUrl: '', joinLabel: '', room };
  }
  const scheme = pageProtocol === 'https:' ? 'wss://' : 'ws://';
  if (/^wss?:\/\/[A-Za-z0-9._:-]+(\/[A-Za-z0-9._~:/?#[\]@!$&'()*+,;=%-]*)?$/.test(rawJoin)) {
    return { wsUrl: rawJoin, joinLabel: rawJoin, room };
  }
  if (/^[A-Za-z0-9._-]+(:\d{1,5})?$/.test(rawJoin)) {
    return {
      wsUrl: `${scheme}${rawJoin}/ws`,
      joinLabel: rawJoin,
      room
    };
  }
  return { wsUrl: '', joinLabel: '', room };
}

async function connectToWsUrl(url) {
  const trimmed = String(url || '').trim();
  if (!trimmed) return;
  audio?.unlock();
  cs16Web?.unlockAudio?.();
  net?.close();
  cs16Web?.connectRemote?.();
  net = new WebGameTransport(trimmed, {
    onPacket: (pkt) => cs16Web?.pushNetPacket?.(pkt),
    onStateChange: (st) => setStatus(`Net: ${st}`)
  });
  await net.connect();
  panelEl?.classList.add('collapsed');
  input?.lockPointer();
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

  const invite = parseInviteParams(window.location.search, window.location.protocol);
  if (invite.wsUrl && wsUrlInput) {
    wsUrlInput.value = invite.wsUrl;
  }

  const cached = await hydrateAssetsFromDb(
    (p, b) => cs16Web.mountFile?.(p, b),
    (n) => setAssetStatus(`Loading cached assets (${n})...`)
  );
  if (cached > 0) {
    cs16Web.finishMount?.();
    refreshMaps();
    setAssetStatus(`Restored ${cached} cached assets`);
  }
  if (invite.wsUrl) {
    const roomNote = invite.room ? ` (room ${invite.room})` : '';
    if (cached > 0) {
      setStatus(`Joining ${invite.joinLabel}${roomNote}...`);
      connectToWsUrl(invite.wsUrl).catch((err) => {
        setStatus(`Invite connect failed: ${err.message}`);
      });
    } else {
      setStatus(`Invite ready: ${invite.joinLabel}${roomNote} — mount Half-Life folder`);
    }
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
    const url = wsUrlInput?.value?.trim();
    if (!url) return;
    await connectToWsUrl(url);
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

      let lobbies = [];
      try {
        const lobbiesUrl = endpoint.replace(/\/servers\/?$/, '/lobbies');
        if (lobbiesUrl !== endpoint) {
          const lresp = await fetch(lobbiesUrl);
          if (lresp.ok) {
            const parsedLobbies = await lresp.json();
            if (Array.isArray(parsedLobbies)) lobbies = parsedLobbies;
          }
        }
      } catch {
        // Master server may not expose /lobbies (e.g., standalone cs-server /servers)
      }

      serverListSelect.innerHTML = '';
      const scheme = window.location.protocol === 'https:' ? 'wss://' : 'ws://';
      const hasServers = Array.isArray(list) && list.length > 0;
      if (!hasServers && lobbies.length === 0) {
        const opt = document.createElement('option');
        opt.value = '';
        opt.textContent = '(no servers or lobbies registered)';
        serverListSelect.appendChild(opt);
      } else {
        if (hasServers) {
          for (const s of list) {
            const opt = document.createElement('option');
            const host = String(s.addr || '').split(':')[0] || '127.0.0.1';
            const webPort = Number(s.web_port) || 27016;
            opt.value = `${scheme}${host}:${webPort}/ws`;
            opt.textContent = `${s.addr} — ${s.map || '?'} (${s.players ?? 0}/${s.max ?? 0})`;
            serverListSelect.appendChild(opt);
          }
        }
        for (const l of lobbies) {
          const opt = document.createElement('option');
          opt.value = `${scheme}${l.host}:${l.port}/ws`;
          opt.textContent = `[Room ${l.room}] ${l.host}:${l.port} — ${l.map || '?'} (${l.players ?? 0}/${l.max_players ?? 16})`;
          serverListSelect.appendChild(opt);
        }
        if (wsUrlInput && serverListSelect.options[0]?.value) {
          wsUrlInput.value = serverListSelect.options[0].value;
        }
      }
      setStatus(`Master: ${hasServers ? list.length : 0} server(s), ${lobbies.length} lobby(s)`);
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
