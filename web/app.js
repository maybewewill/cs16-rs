import {
  cacheAssetsInDb,
  clearAssetDb,
  hydrateAssetsFromDb,
  mountFromDirectoryPicker,
  mountFromDropEvent,
  mountFromFileList
} from './vfs_loader.js';
import { WebGameTransport, WebRtcGuestTransport, WebRtcHostHub } from './transport.js';
import { WebInputController } from './input.js';
import { WebAudioMixer } from './audio.js';

const canvas = document.getElementById('cs16-canvas');
const statusEl = document.getElementById('status-pill');
const assetStatusEl = document.getElementById('asset-status');
const mapSelect = document.getElementById('map-select');
const botsInput = document.getElementById('bots-count');
const wsUrlInput = document.getElementById('ws-url');
const wsMixedHintEl = document.getElementById('ws-mixed-hint');
const masterUrlInput = document.getElementById('master-url');
const serverListSelect = document.getElementById('server-list');
const p2pMaxPlayersSelect = document.getElementById('p2p-max-players');
const panelEl = document.getElementById('control-panel');
const hostingModeBadgeEl = document.getElementById('hosting-mode-badge');
const onboardingCardEl = document.getElementById('onboarding-card');
const adminLinkEl = document.getElementById('link-admin');

let cs16Web = null;
let net = null;
let p2pHost = null;
let audio = null;
let input = null;
let pendingP2pInvite = null;
let hostingMode = { kind: 'static-https', isStaticPages: false, isHttps: false };

function setStatus(text) {
  if (statusEl) statusEl.textContent = text;
}

function setAssetStatus(text) {
  if (assetStatusEl) assetStatusEl.textContent = text;
}

function detectHostingMode(loc) {
  const host = String(loc?.hostname || '').toLowerCase();
  const protocol = String(loc?.protocol || '').toLowerCase();
  const isHttps = protocol === 'https:';
  const isStaticPages =
    host.endsWith('.github.io') ||
    host.endsWith('.pages.dev') ||
    host.endsWith('.netlify.app') ||
    host.endsWith('.vercel.app') ||
    protocol === 'file:';
  if (isStaticPages) {
    return { kind: isHttps ? 'static-https' : 'static-local', isStaticPages: true, isHttps };
  }
  return { kind: 'self-hosted', isStaticPages: false, isHttps };
}

function updateMixedContentHint(rawUrl) {
  if (!wsMixedHintEl) return;
  const val = String(rawUrl || '').trim();
  const isHttps = window.location.protocol === 'https:';
  if (isHttps && /^ws:\/\//i.test(val)) {
    wsMixedHintEl.innerHTML =
      'Browser Mixed Content blocks insecure <code>ws://</code> from an <code>https://</code> page. ' +
      'Use a TLS <code>wss://</code> server/relay URL, or open your local server directly at ' +
      '<code>http://127.0.0.1:27016/</code>.';
    wsMixedHintEl.classList.remove('hidden');
  } else {
    wsMixedHintEl.textContent = '';
    wsMixedHintEl.classList.add('hidden');
  }
}

function applyDualModeDefaults(invite) {
  hostingMode = detectHostingMode(window.location);
  const wsScheme = window.location.protocol === 'https:' ? 'wss' : 'ws';

  if (hostingMode.isStaticPages) {
    if (hostingModeBadgeEl) {
      hostingModeBadgeEl.textContent = hostingMode.isHttps ? 'GitHub Pages / HTTPS' : 'Static Launcher';
    }
    if (onboardingCardEl) onboardingCardEl.classList.remove('hidden');
    if (wsUrlInput && !invite.wsUrl) {
      wsUrlInput.value = '';
      wsUrlInput.placeholder = 'wss://your-server.example/ws or P2P invite link';
    }
    if (masterUrlInput) {
      masterUrlInput.value = '';
      masterUrlInput.placeholder = 'https://master.example/servers (optional signaling/master)';
    }
  } else {
    if (hostingModeBadgeEl) hostingModeBadgeEl.textContent = 'Self-Hosted Server';
    if (adminLinkEl) adminLinkEl.style.display = 'inline-block';
    if (wsUrlInput && !invite.wsUrl && window.location.host) {
      wsUrlInput.value = `${wsScheme}://${window.location.host}/ws`;
    }
    if (masterUrlInput && window.location.origin && window.location.origin !== 'null') {
      masterUrlInput.value = `${window.location.origin}/servers`;
    }
  }
  updateMixedContentHint(wsUrlInput?.value || '');
}

function parseInviteParams(search, pageProtocol, pageOrigin = '') {
  const params = new URLSearchParams(search || '');
  const rawJoin = (params.get('join') || '').trim();
  const rawRoom = (params.get('room') || '').trim();
  const rawP2p = (params.get('p2p') || '').trim();
  const rawSignal = (params.get('signal') || '').trim();
  const rawWt = (params.get('wt') || '').trim();
  const rawWtHash = (params.get('wtHash') || '').trim();
  const room = /^[A-Za-z0-9_-]{1,32}$/.test(rawRoom) ? rawRoom : '';
  const p2pRoom = /^[A-Za-z0-9_-]{1,32}$/.test(rawP2p) ? rawP2p : '';
  const fallbackSignal = /^https?:\/\//i.test(pageOrigin) ? pageOrigin : '';
  const signalOrigin = /^https?:\/\/[A-Za-z0-9._:-]+(\/[A-Za-z0-9._~:/?#[\]@!$&'()*+,;=%-]*)?$/i.test(rawSignal)
    ? rawSignal.replace(/\/+$/, '')
    : fallbackSignal;
  const webTransportUrl = /^https:\/\/[A-Za-z0-9._:\[\]-]+(\/[A-Za-z0-9._~:/?#[\]@!$&'()*+,;=%-]*)?$/i.test(rawWt)
    ? rawWt
    : '';
  const wtCertHash = /^[A-Za-z0-9+/_=:-]{43,96}$/.test(rawWtHash) ? rawWtHash : '';
  if (!rawJoin) {
    return { wsUrl: '', joinLabel: '', room, p2pRoom, signalOrigin, webTransportUrl, wtCertHash };
  }
  const scheme = pageProtocol === 'https:' ? 'wss://' : 'ws://';
  if (/^(wss?|https):\/\/[A-Za-z0-9._:\[\]-]+(\/[A-Za-z0-9._~:/?#[\]@!$&'()*+,;=%-]*)?$/i.test(rawJoin)) {
    return { wsUrl: rawJoin, joinLabel: rawJoin, room, p2pRoom, signalOrigin, webTransportUrl, wtCertHash };
  }
  if (/^[A-Za-z0-9._-]+(:\d{1,5})?$/.test(rawJoin)) {
    return {
      wsUrl: `${scheme}${rawJoin}/ws`,
      joinLabel: rawJoin,
      room,
      p2pRoom,
      signalOrigin,
      webTransportUrl,
      wtCertHash
    };
  }
  return { wsUrl: '', joinLabel: '', room, p2pRoom, signalOrigin, webTransportUrl, wtCertHash };
}

function closeActiveTransports() {
  net?.close();
  net = null;
  p2pHost?.close();
  p2pHost = null;
}

function resolveSignalOrigin() {
  const master = masterUrlInput?.value?.trim() || '';
  if (/^https?:\/\//i.test(master)) {
    return master.replace(/\/(servers|lobbies)\/?$/i, '').replace(/\/+$/, '');
  }
  if (!hostingMode.isStaticPages && (window.location.protocol === 'http:' || window.location.protocol === 'https:')) {
    return window.location.origin;
  }
  return '';
}

async function connectToWsUrl(url, transportOpts = {}) {
  const trimmed = String(url || '').trim();
  if (!trimmed) return;
  updateMixedContentHint(trimmed);
  if (window.location.protocol === 'https:' && /^ws:\/\//i.test(trimmed)) {
    setStatus('Blocked by HTTPS Mixed Content — use wss:// or local http://');
    return;
  }
  audio?.unlock();
  cs16Web?.unlockAudio?.();
  closeActiveTransports();
  cs16Web?.connectRemote?.();
  const sameOriginInfo = !hostingMode.isStaticPages
    && (window.location.protocol === 'http:' || window.location.protocol === 'https:')
    ? `${window.location.origin}/api/server-info`
    : null;
  net = new WebGameTransport(trimmed, {
    onPacket: (pkt) => cs16Web?.pushNetPacket?.(pkt),
    onStateChange: (st) => setStatus(`Net: ${st}`),
    webTransportUrl: transportOpts.webTransportUrl || null,
    serverCertificateHashes: transportOpts.wtCertHash || transportOpts.serverCertificateHashes || null,
    serverInfoUrl: transportOpts.serverInfoUrl || sameOriginInfo
  });
  await net.connect();
  panelEl?.classList.add('collapsed');
  input?.lockPointer();
}

async function connectToP2pRoom(room, signalOrigin) {
  if (!room || !signalOrigin) {
    throw new Error('Missing P2P room or signaling URL');
  }
  audio?.unlock();
  cs16Web?.unlockAudio?.();
  closeActiveTransports();
  cs16Web?.connectRemote?.();
  net = new WebRtcGuestTransport(signalOrigin, room, {
    onPacket: (pkt) => cs16Web?.pushNetPacket?.(pkt),
    onStateChange: (st) => setStatus(`P2P (${room}): ${st}`)
  });
  await net.connect();
  panelEl?.classList.add('collapsed');
  input?.lockPointer();
}

function refreshMaps(preferredMap = '') {
  if (!cs16Web || !mapSelect) return;
  const maps = cs16Web.listMaps?.() || [];
  mapSelect.innerHTML = '';
  const defaultPick =
    (preferredMap && maps.includes(preferredMap) && preferredMap) ||
    (maps.includes('de_dust2') && 'de_dust2') ||
    (maps.includes('dm_rusthouse') && 'dm_rusthouse') ||
    maps[0] ||
    '';
  for (const m of maps) {
    const opt = document.createElement('option');
    opt.value = m;
    opt.textContent = m;
    if (m === defaultPick) opt.selected = true;
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
  if (pendingP2pInvite) {
    const { p2pRoom, signalOrigin } = pendingP2pInvite;
    pendingP2pInvite = null;
    connectToP2pRoom(p2pRoom, signalOrigin).catch((err) => {
      setStatus(`P2P invite failed: ${err.message}`);
    });
  }
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
  const invite = parseInviteParams(
    window.location.search,
    window.location.protocol,
    window.location.origin
  );
  applyDualModeDefaults(invite);
  if (invite.wsUrl && wsUrlInput) {
    wsUrlInput.value = invite.wsUrl;
    updateMixedContentHint(invite.wsUrl);
  }
  wsUrlInput?.addEventListener('input', () => updateMixedContentHint(wsUrlInput.value));

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
    setAssetStatus(`Restored ${cached} cached assets (${cs16Web.listMaps?.().length || 0} maps)`);
  }
  if (invite.p2pRoom && invite.signalOrigin) {
    if (cached > 0) {
      setStatus(`Joining P2P room ${invite.p2pRoom}...`);
      connectToP2pRoom(invite.p2pRoom, invite.signalOrigin).catch((err) => {
        setStatus(`P2P invite failed: ${err.message}`);
      });
    } else {
      pendingP2pInvite = { p2pRoom: invite.p2pRoom, signalOrigin: invite.signalOrigin };
      setStatus(`P2P invite ready (${invite.p2pRoom}) — mount Half-Life folder to join`);
    }
  } else if (invite.wsUrl) {
    const roomNote = invite.room ? ` (room ${invite.room})` : '';
    if (cached > 0) {
      setStatus(`Joining ${invite.joinLabel}${roomNote}...`);
      connectToWsUrl(invite.wsUrl, {
        webTransportUrl: invite.webTransportUrl,
        wtCertHash: invite.wtCertHash
      }).catch((err) => {
        setStatus(`Invite connect failed: ${err.message}`);
      });
    } else {
      setStatus(`Invite ready: ${invite.joinLabel}${roomNote} — mount Half-Life folder`);
    }
  }

  document.getElementById('btn-instant-demo')?.addEventListener('click', () => {
    audio.unlock();
    cs16Web.unlockAudio?.();
    closeActiveTransports();
    const bots = Math.max(0, Math.min(15, Number(botsInput?.value || 9)));
    const t0 = performance.now();
    const res = cs16Web.startInstantDemo?.(bots);
    const ms = Math.max(1, Math.round(performance.now() - t0));
    if (!res || !res.ok) {
      setStatus(`Instant Demo failed: ${res?.error || 'unavailable'}`);
      return;
    }
    const demoMap = res.map || 'dm_rusthouse';
    refreshMaps(demoMap);
    const kb = Math.round((res.bytes || 0) / 1024);
    setAssetStatus(`Instant Demo active (${res.files || 0} procedural files, ${kb} KiB in ${ms} ms — zero Steam files needed)`);
    panelEl?.classList.add('collapsed');
    input.lockPointer();
    setStatus(`Instant Demo: ${demoMap} (${bots} bots)`);
  });

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
    closeActiveTransports();
    const map = mapSelect?.value || 'de_dust2';
    const bots = Math.max(0, Math.min(15, Number(botsInput?.value || 9)));
    cs16Web.startLocal?.(map, bots);
    panelEl?.classList.add('collapsed');
    input.lockPointer();
    setStatus(`Offline: ${map} (${bots} bots)`);
  });

  document.getElementById('btn-host-p2p')?.addEventListener('click', async () => {
    audio.unlock();
    cs16Web.unlockAudio?.();
    closeActiveTransports();
    const map = mapSelect?.value || 'de_dust2';
    const maxPlayers = Math.max(2, Math.min(16, Number(p2pMaxPlayersSelect?.value || 10)));
    const bots = Math.max(0, Math.min(maxPlayers - 1, Number(botsInput?.value || 0)));
    const signalOrigin = resolveSignalOrigin();
    if (!signalOrigin) {
      setStatus('Enter a signaling/master HTTPS URL above (or host via cs-server --web-port)');
      return;
    }
    const ok = cs16Web.startP2pHost?.(map, bots, maxPlayers);
    if (!ok) {
      setStatus('Mount Half-Life folder before hosting P2P room');
      return;
    }
    const room = `p2p-${Math.random().toString(36).slice(2, 8)}`;
    p2pHost = new WebRtcHostHub({
      signalBaseUrl: signalOrigin,
      room,
      map,
      maxPlayers,
      onPacket: (peerId, pkt) => cs16Web?.pushP2pPacket?.(peerId, pkt),
      onPeerOpen: (peerId) => setStatus(`P2P Room ${room}: peer #${peerId} joined`),
      onPeerClose: (peerId) => setStatus(`P2P Room ${room}: peer #${peerId} left`),
      onStateChange: (st) => setStatus(`P2P: ${st}`)
    });
    try {
      await p2pHost.start();
      const shareUrl = `${window.location.origin}${window.location.pathname}?p2p=${encodeURIComponent(room)}&signal=${encodeURIComponent(signalOrigin)}`;
      if (wsUrlInput) wsUrlInput.value = shareUrl;
      let copied = false;
      try {
        await navigator.clipboard?.writeText(shareUrl);
        copied = true;
      } catch {}
      setStatus(
        copied
          ? `P2P Room ${room} live — share link copied!`
          : `P2P Room ${room} live: ${shareUrl}`
      );
      panelEl?.classList.add('collapsed');
      input.lockPointer();
    } catch (err) {
      closeActiveTransports();
      cs16Web.disconnect?.();
      setStatus(`P2P host failed: ${err.message}`);
    }
  });

  document.getElementById('btn-connect')?.addEventListener('click', async () => {
    const url = wsUrlInput?.value?.trim();
    if (!url) return;
    if (/^https?:\/\/.*[?&]p2p=/i.test(url)) {
      const parsed = new URL(url);
      const inviteUrl = parseInviteParams(parsed.search, parsed.protocol, parsed.origin);
      if (inviteUrl.p2pRoom && inviteUrl.signalOrigin) {
        await connectToP2pRoom(inviteUrl.p2pRoom, inviteUrl.signalOrigin);
        return;
      }
    }
    try {
      await connectToWsUrl(url);
    } catch (err) {
      setStatus(`Connect failed: ${err.message}`);
    }
  });

  document.getElementById('btn-disconnect')?.addEventListener('click', () => {
    closeActiveTransports();
    cs16Web.disconnect?.();
    panelEl?.classList.remove('collapsed');
    setStatus('Disconnected');
  });

  document.getElementById('btn-refresh-servers')?.addEventListener('click', async () => {
    const endpoint = masterUrlInput?.value?.trim();
    if (!endpoint || !serverListSelect) {
      setStatus('Enter a master/server directory URL first');
      return;
    }
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
          updateMixedContentHint(wsUrlInput.value);
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
      updateMixedContentHint(wsUrlInput.value);
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
    if (p2pHost) {
      const p2pOut = cs16Web.drainP2pPackets?.();
      if (Array.isArray(p2pOut)) {
        for (const item of p2pOut) {
          p2pHost.send(item.peerId, item.data);
        }
      }
    }
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
