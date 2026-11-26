// Browser transport adapter: prefers HTTP/3 WebTransport datagrams (using explicit
// `serverCertificateHashes` or auto-discovery from `cs-server`'s `/api/server-info`)
// and falls back transparently to RFC 6455 WebSocket if unavailable or rejected.
export const MAX_WEBTRANSPORT_PACKET_BYTES = 4010;

export function decodeCertSha256(value) {
  if (!value) return null;
  if (value instanceof Uint8Array) {
    return value.byteLength === 32 ? value : null;
  }
  if (value instanceof ArrayBuffer) {
    return value.byteLength === 32 ? new Uint8Array(value) : null;
  }
  if (ArrayBuffer.isView(value)) {
    return value.byteLength === 32
      ? new Uint8Array(value.buffer, value.byteOffset, value.byteLength)
      : null;
  }
  if (typeof value !== 'string') return null;
  const trimmed = value.trim();
  if (!trimmed) return null;

  // Colon-separated or plain 64-char hex digest.
  const compactHex = trimmed.replace(/[:\-\s]/g, '');
  if (/^[0-9a-fA-F]{64}$/.test(compactHex)) {
    const out = new Uint8Array(32);
    for (let i = 0; i < 32; i++) {
      out[i] = parseInt(compactHex.slice(i * 2, i * 2 + 2), 16);
    }
    return out;
  }

  // Standard or URL-safe Base64 encoding of 32 bytes.
  const normB64 = trimmed.replace(/-/g, '+').replace(/_/g, '/');
  const padded = normB64.length % 4 === 0
    ? normB64
    : normB64 + '='.repeat(4 - (normB64.length % 4));
  try {
    if (typeof atob === 'function') {
      const bin = atob(padded);
      if (bin.length !== 32) return null;
      const out = new Uint8Array(32);
      for (let i = 0; i < 32; i++) {
        out[i] = bin.charCodeAt(i);
      }
      return out;
    }
    if (typeof Buffer !== 'undefined') {
      const buf = Buffer.from(padded, 'base64');
      return buf.length === 32 ? new Uint8Array(buf) : null;
    }
  } catch {
    return null;
  }
  return null;
}

export function normalizeServerCertificateHashes(input) {
  if (!input) return [];
  const list = Array.isArray(input) ? input : [input];
  const out = [];
  for (const item of list) {
    const raw = item && typeof item === 'object' && !(item instanceof Uint8Array) && !(item instanceof ArrayBuffer) && !ArrayBuffer.isView(item)
      ? item.value
      : item;
    const bytes = decodeCertSha256(raw);
    if (bytes) {
      out.push({ algorithm: 'sha-256', value: bytes });
    }
  }
  return out;
}

function deriveServerInfoUrl(rawUrl) {
  try {
    const parsed = new URL(rawUrl);
    if (parsed.protocol === 'ws:') {
      return `http://${parsed.host}/api/server-info`;
    }
    if (parsed.protocol === 'wss:') {
      return `https://${parsed.host}/api/server-info`;
    }
    if (parsed.protocol === 'http:' || parsed.protocol === 'https:') {
      return `${parsed.origin}/api/server-info`;
    }
  } catch {
    return null;
  }
  return null;
}

export class WebGameTransport {
  constructor(
    url,
    {
      onPacket,
      onStateChange,
      webTransportUrl = null,
      serverCertificateHashes = null,
      serverInfoUrl = null,
      preferWebTransport = true,
      discoveryTimeoutMs = 1500
    } = {}
  ) {
    this.url = url;
    this.onPacket = onPacket || (() => {});
    this.onStateChange = onStateChange || (() => {});
    this.webTransportUrl = webTransportUrl;
    this.serverCertificateHashes = serverCertificateHashes;
    this.serverInfoUrl = serverInfoUrl;
    this.preferWebTransport = preferWebTransport !== false;
    this.discoveryTimeoutMs = Math.max(200, Number(discoveryTimeoutMs) || 1500);
    this.kind = null;
    this.ws = null;
    this.wt = null;
    this.wtWriter = null;
    this.closed = false;
  }

  async _discoverWebTransport() {
    const explicitHashes = normalizeServerCertificateHashes(this.serverCertificateHashes);
    if (this.webTransportUrl && /^https:\/\//i.test(this.webTransportUrl)) {
      return { url: this.webTransportUrl, hashes: explicitHashes };
    }
    if (/^https:\/\//i.test(this.url)) {
      return { url: this.url, hashes: explicitHashes };
    }
    const infoUrl = this.serverInfoUrl || deriveServerInfoUrl(this.url);
    if (!infoUrl || typeof fetch === 'undefined') {
      return null;
    }
    const controller = typeof AbortController !== 'undefined' ? new AbortController() : null;
    const timer = controller
      ? setTimeout(() => controller.abort(), this.discoveryTimeoutMs)
      : null;
    try {
      const resp = await fetch(infoUrl, {
        cache: 'no-store',
        signal: controller ? controller.signal : undefined
      });
      if (!resp.ok) return null;
      const info = await resp.json();
      const wt = info && info.webtransport;
      if (!wt || typeof wt.url !== 'string' || !/^https:\/\//i.test(wt.url)) {
        return null;
      }
      let hashes = explicitHashes;
      if (hashes.length === 0 && wt.self_signed !== false) {
        const discovered = wt.cert_sha256_base64 || wt.cert_sha256_hex;
        hashes = normalizeServerCertificateHashes(discovered);
      }
      return { url: wt.url, hashes };
    } catch {
      return null;
    } finally {
      if (timer) clearTimeout(timer);
    }
  }

  async connect() {
    if (this.preferWebTransport && typeof WebTransport !== 'undefined') {
      const candidate = await this._discoverWebTransport();
      if (candidate && candidate.url) {
        try {
          await this._connectWebTransport(candidate.url, candidate.hashes);
          return;
        } catch (err) {
          console.warn('WebTransport failed, falling back to WebSocket:', err);
        }
      }
    }
    const wsUrl = this.url
      .replace(/^https:\/\//i, 'wss://')
      .replace(/^http:\/\//i, 'ws://');
    await this._connectWebSocket(wsUrl);
  }

  async _connectWebTransport(url, certHashes = []) {
    const hashes = normalizeServerCertificateHashes(certHashes);
    const options = hashes.length > 0 ? { serverCertificateHashes: hashes } : undefined;
    const wt = options ? new WebTransport(url, options) : new WebTransport(url);
    try {
      await wt.ready;
    } catch (err) {
      try {
        wt.close();
      } catch {}
      throw err;
    }
    this.wt = wt;
    this.kind = 'webtransport';
    this.wtWriter = wt.datagrams.writable.getWriter();
    this.onStateChange('connected:webtransport');

    wt.closed.catch(() => {}).finally(() => {
      if (!this.closed) {
        this.close();
      }
    });

    const reader = wt.datagrams.readable.getReader();
    (async () => {
      try {
        while (!this.closed) {
          const { value, done } = await reader.read();
          if (done) break;
          if (value && value.byteLength > 0 && value.byteLength <= MAX_WEBTRANSPORT_PACKET_BYTES) {
            this.onPacket(value instanceof Uint8Array ? value : new Uint8Array(value));
          }
        }
      } catch {
        // Ignore read closure when disconnecting cleanly.
      } finally {
        try {
          reader.releaseLock();
        } catch {}
        this.close();
      }
    })();
  }

  _connectWebSocket(url) {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(url);
      ws.binaryType = 'arraybuffer';
      this.ws = ws;
      this.kind = 'websocket';

      ws.onopen = () => {
        this.onStateChange('connected:websocket');
        resolve();
      };
      ws.onmessage = (ev) => {
        if (ev.data instanceof ArrayBuffer) {
          this.onPacket(new Uint8Array(ev.data));
        }
      };
      ws.onerror = () => {
        if (this.kind === 'websocket' && ws.readyState !== WebSocket.OPEN) {
          reject(new Error(`WebSocket connection failed: ${url}`));
        }
      };
      ws.onclose = () => {
        if (!this.closed) {
          this.closed = true;
          this.onStateChange('disconnected');
        }
      };
    });
  }

  send(bytes) {
    if (this.closed) return;
    const u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    if (u8.byteLength === 0 || u8.byteLength > MAX_WEBTRANSPORT_PACKET_BYTES) return;
    if (this.kind === 'webtransport' && this.wtWriter) {
      this.wtWriter.write(u8).catch(() => {});
    } else if (this.kind === 'websocket' && this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(u8);
    }
  }

  close() {
    if (this.closed) return;
    this.closed = true;
    try {
      this.wtWriter?.releaseLock?.();
    } catch {}
    this.wtWriter = null;
    try {
      this.ws?.close();
    } catch {}
    try {
      this.wt?.close();
    } catch {}
    this.onStateChange('disconnected');
  }
}

export const DEFAULT_ICE_SERVERS = [{ urls: 'stun:stun.l.google.com:19302' }];
export const MAX_WEBRTC_PACKET_BYTES = 4096;
export const MAX_WEBRTC_PEER_SLOTS = 16;

function normalizeSignalBase(url) {
  return String(url || '').trim().replace(/\/+$/, '');
}

async function postJson(url, body) {
  const resp = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body)
  });
  if (!resp.ok) {
    throw new Error(`HTTP ${resp.status} from ${url}`);
  }
  return resp.json();
}

function toBoundedPacket(data) {
  let u8 = null;
  if (data instanceof ArrayBuffer) {
    u8 = new Uint8Array(data);
  } else if (ArrayBuffer.isView(data)) {
    u8 = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  }
  if (!u8 || u8.byteLength === 0 || u8.byteLength > MAX_WEBRTC_PACKET_BYTES) {
    return null;
  }
  return u8;
}

// Browser-hosted listen server hub accepting up to 16 guest peers over unreliable WebRTC DataChannels.
export class WebRtcHostHub {
  constructor({
    signalBaseUrl,
    room,
    map = 'de_dust2',
    maxPlayers = 10,
    iceServers = DEFAULT_ICE_SERVERS,
    onPacket,
    onPeerOpen,
    onPeerClose,
    onStateChange
  }) {
    this.signalBaseUrl = normalizeSignalBase(signalBaseUrl);
    this.room = String(room || '').trim();
    this.map = map;
    this.maxPlayers = Math.max(2, Math.min(MAX_WEBRTC_PEER_SLOTS, Number(maxPlayers) || 10));
    this.iceServers = Array.isArray(iceServers) && iceServers.length > 0 ? iceServers : DEFAULT_ICE_SERVERS;
    this.onPacket = onPacket || (() => {});
    this.onPeerOpen = onPeerOpen || (() => {});
    this.onPeerClose = onPeerClose || (() => {});
    this.onStateChange = onStateChange || (() => {});
    this.peersByLabel = new Map();
    this.peersById = new Map();
    this.lastSignalId = 0;
    this.pollTimer = null;
    this.heartbeatTimer = null;
    this.polling = false;
    this.closed = false;
  }

  async start() {
    if (!this.signalBaseUrl || !this.room) {
      throw new Error('signalBaseUrl and room are required for WebRTC P2P host');
    }
    await this._registerLobby();
    this.onStateChange(`hosting:webrtc (${this.room})`);
    this.pollTimer = setInterval(() => {
      this._pollSignals().catch(() => {});
    }, 450);
    this.heartbeatTimer = setInterval(() => {
      this._registerLobby().catch(() => {});
    }, 45000);
  }

  async _registerLobby() {
    if (this.closed) return;
    const loc = typeof window !== 'undefined' ? window.location : null;
    const host = (loc && loc.hostname) ? loc.hostname : 'browser-p2p';
    const port = (loc && Number(loc.port)) ? Number(loc.port) : 443;
    await postJson(`${this.signalBaseUrl}/lobbies`, {
      room: this.room,
      host,
      port,
      map: this.map,
      players: 1 + this.peersById.size,
      max_players: this.maxPlayers,
      mode: 'webrtc'
    });
  }

  _allocPeerId() {
    const cap = Math.min(MAX_WEBRTC_PEER_SLOTS, this.maxPlayers - 1);
    for (let id = 1; id <= cap; id++) {
      if (!this.peersById.has(id)) return id;
    }
    return null;
  }

  async _pollSignals() {
    if (this.closed || this.polling) return;
    this.polling = true;
    try {
      const url = `${this.signalBaseUrl}/lobbies/${encodeURIComponent(this.room)}/signal?since=${this.lastSignalId}&to=host`;
      const resp = await fetch(url);
      if (!resp.ok) return;
      const list = await resp.json();
      if (!Array.isArray(list)) return;
      for (const sig of list) {
        if (typeof sig.id === 'number' && sig.id > this.lastSignalId) {
          this.lastSignalId = sig.id;
        }
        await this._handleSignal(sig);
      }
    } finally {
      this.polling = false;
    }
  }

  async _handleSignal(sig) {
    if (!sig || !sig.from || sig.from === 'host') return;
    if (sig.kind === 'join') {
      await this._acceptGuest(sig.from);
      return;
    }
    const entry = this.peersByLabel.get(sig.from);
    if (!entry) return;
    if (sig.kind === 'answer') {
      const desc = JSON.parse(sig.payload);
      await entry.pc.setRemoteDescription(desc);
      entry.remoteDescSet = true;
      for (const cand of entry.pendingIce) {
        await entry.pc.addIceCandidate(cand).catch(() => {});
      }
      entry.pendingIce.length = 0;
    } else if (sig.kind === 'ice' && sig.payload) {
      const cand = JSON.parse(sig.payload);
      if (entry.remoteDescSet) {
        await entry.pc.addIceCandidate(cand).catch(() => {});
      } else {
        entry.pendingIce.push(cand);
      }
    }
  }

  async _acceptGuest(guestLabel) {
    if (this.peersByLabel.has(guestLabel)) {
      this._dropPeer(guestLabel);
    }
    const peerId = this._allocPeerId();
    if (peerId === null) return;

    const pc = new RTCPeerConnection({ iceServers: this.iceServers });
    const dc = pc.createDataChannel('proto48', {
      ordered: false,
      maxRetransmits: 0
    });
    dc.binaryType = 'arraybuffer';

    const entry = {
      peerId,
      label: guestLabel,
      pc,
      dc,
      remoteDescSet: false,
      pendingIce: []
    };
    this.peersByLabel.set(guestLabel, entry);
    this.peersById.set(peerId, entry);

    dc.onopen = () => {
      this.onPeerOpen(peerId, guestLabel);
      this._registerLobby().catch(() => {});
    };
    dc.onmessage = (ev) => {
      const pkt = toBoundedPacket(ev.data);
      if (pkt) this.onPacket(peerId, pkt);
    };
    dc.onclose = () => this._dropPeer(guestLabel);
    pc.onconnectionstatechange = () => {
      if (pc.connectionState === 'failed' || pc.connectionState === 'closed') {
        this._dropPeer(guestLabel);
      }
    };
    pc.onicecandidate = (ev) => {
      if (!ev.candidate || this.closed) return;
      postJson(`${this.signalBaseUrl}/lobbies/${encodeURIComponent(this.room)}/signal`, {
        from: 'host',
        to: guestLabel,
        kind: 'ice',
        payload: JSON.stringify(ev.candidate)
      }).catch(() => {});
    };

    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);
    await postJson(`${this.signalBaseUrl}/lobbies/${encodeURIComponent(this.room)}/signal`, {
      from: 'host',
      to: guestLabel,
      kind: 'offer',
      payload: JSON.stringify(pc.localDescription)
    });
  }

  _dropPeer(guestLabel) {
    const entry = this.peersByLabel.get(guestLabel);
    if (!entry) return;
    this.peersByLabel.delete(guestLabel);
    this.peersById.delete(entry.peerId);
    try { entry.dc?.close(); } catch {}
    try { entry.pc?.close(); } catch {}
    this.onPeerClose(entry.peerId, guestLabel);
  }

  send(peerId, bytes) {
    if (this.closed) return;
    const entry = this.peersById.get(peerId);
    if (!entry || !entry.dc || entry.dc.readyState !== 'open') return;
    const u8 = toBoundedPacket(bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes));
    if (u8) {
      entry.dc.send(u8);
    }
  }

  close() {
    if (this.closed) return;
    this.closed = true;
    clearInterval(this.pollTimer);
    clearInterval(this.heartbeatTimer);
    for (const label of Array.from(this.peersByLabel.keys())) {
      this._dropPeer(label);
    }
    if (this.signalBaseUrl && this.room) {
      fetch(`${this.signalBaseUrl}/lobbies/${encodeURIComponent(this.room)}`, {
        method: 'DELETE',
        keepalive: true
      }).catch(() => {});
    }
    this.onStateChange('disconnected');
  }
}

// Guest transport connecting to a browser-hosted listen server over an unreliable WebRTC DataChannel.
export class WebRtcGuestTransport {
  constructor(signalBaseUrl, room, {
    iceServers = DEFAULT_ICE_SERVERS,
    peerLabel,
    onPacket,
    onStateChange
  } = {}) {
    this.signalBaseUrl = normalizeSignalBase(signalBaseUrl);
    this.room = String(room || '').trim();
    this.iceServers = Array.isArray(iceServers) && iceServers.length > 0 ? iceServers : DEFAULT_ICE_SERVERS;
    this.peerLabel = peerLabel || `guest-${Math.random().toString(36).slice(2, 10)}`;
    this.onPacket = onPacket || (() => {});
    this.onStateChange = onStateChange || (() => {});
    this.pc = null;
    this.dc = null;
    this.lastSignalId = 0;
    this.pollTimer = null;
    this.polling = false;
    this.remoteDescSet = false;
    this.pendingIce = [];
    this.closed = false;
  }

  async connect() {
    if (!this.signalBaseUrl || !this.room) {
      throw new Error('signalBaseUrl and room are required');
    }
    this.onStateChange('connecting:webrtc');

    const pc = new RTCPeerConnection({ iceServers: this.iceServers });
    this.pc = pc;

    pc.onicecandidate = (ev) => {
      if (!ev.candidate || this.closed) return;
      postJson(`${this.signalBaseUrl}/lobbies/${encodeURIComponent(this.room)}/signal`, {
        from: this.peerLabel,
        to: 'host',
        kind: 'ice',
        payload: JSON.stringify(ev.candidate)
      }).catch(() => {});
    };

    const channelReady = new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        reject(new Error(`WebRTC DataChannel timeout for room ${this.room}`));
      }, 20000);

      pc.ondatachannel = (ev) => {
        const dc = ev.channel;
        dc.binaryType = 'arraybuffer';
        this.dc = dc;
        dc.onopen = () => {
          clearTimeout(timeout);
          clearInterval(this.pollTimer);
          this.onStateChange('connected:webrtc');
          resolve();
        };
        dc.onmessage = (msgEv) => {
          const pkt = toBoundedPacket(msgEv.data);
          if (pkt) this.onPacket(pkt);
        };
        dc.onclose = () => this.close();
      };

      pc.onconnectionstatechange = () => {
        if (pc.connectionState === 'failed' || pc.connectionState === 'closed') {
          clearTimeout(timeout);
          reject(new Error(`WebRTC peer connection ${pc.connectionState}`));
        }
      };
    });

    await postJson(`${this.signalBaseUrl}/lobbies/${encodeURIComponent(this.room)}/signal`, {
      from: this.peerLabel,
      to: 'host',
      kind: 'join',
      payload: ''
    });

    this.pollTimer = setInterval(() => {
      this._pollSignals().catch(() => {});
    }, 350);
    await this._pollSignals().catch(() => {});
    await channelReady;
  }

  async _pollSignals() {
    if (this.closed || this.polling) return;
    this.polling = true;
    try {
      const url = `${this.signalBaseUrl}/lobbies/${encodeURIComponent(this.room)}/signal?since=${this.lastSignalId}&to=${encodeURIComponent(this.peerLabel)}`;
      const resp = await fetch(url);
      if (!resp.ok) return;
      const list = await resp.json();
      if (!Array.isArray(list)) return;
      for (const sig of list) {
        if (typeof sig.id === 'number' && sig.id > this.lastSignalId) {
          this.lastSignalId = sig.id;
        }
        if (sig.from !== 'host') continue;
        if (sig.kind === 'offer') {
          const desc = JSON.parse(sig.payload);
          await this.pc.setRemoteDescription(desc);
          this.remoteDescSet = true;
          const answer = await this.pc.createAnswer();
          await this.pc.setLocalDescription(answer);
          await postJson(`${this.signalBaseUrl}/lobbies/${encodeURIComponent(this.room)}/signal`, {
            from: this.peerLabel,
            to: 'host',
            kind: 'answer',
            payload: JSON.stringify(this.pc.localDescription)
          });
          for (const cand of this.pendingIce) {
            await this.pc.addIceCandidate(cand).catch(() => {});
          }
          this.pendingIce.length = 0;
        } else if (sig.kind === 'ice' && sig.payload) {
          const cand = JSON.parse(sig.payload);
          if (this.remoteDescSet) {
            await this.pc.addIceCandidate(cand).catch(() => {});
          } else {
            this.pendingIce.push(cand);
          }
        }
      }
    } finally {
      this.polling = false;
    }
  }

  send(bytes) {
    if (this.closed || !this.dc || this.dc.readyState !== 'open') return;
    const u8 = toBoundedPacket(bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes));
    if (u8) {
      this.dc.send(u8);
    }
  }

  close() {
    if (this.closed) return;
    this.closed = true;
    clearInterval(this.pollTimer);
    try { this.dc?.close(); } catch {}
    try { this.pc?.close(); } catch {}
    this.onStateChange('disconnected');
  }
}
