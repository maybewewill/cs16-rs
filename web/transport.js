// Browser transport adapter: tries WebTransport over HTTPS only if a user supplies an
// external HTTP/3 proxy, and otherwise connects directly to `cs-server`'s RFC 6455 WebSocket
// endpoint (`cs-server` itself serves WebSocket only; see `docs/parity/web.toml`).
export class WebGameTransport {
  constructor(url, { onPacket, onStateChange } = {}) {
    this.url = url;
    this.onPacket = onPacket || (() => {});
    this.onStateChange = onStateChange || (() => {});
    this.kind = null;
    this.ws = null;
    this.wt = null;
    this.wtWriter = null;
    this.closed = false;
  }

  async connect() {
    if (this.url.startsWith('https://') && typeof WebTransport !== 'undefined') {
      try {
        await this._connectWebTransport(this.url);
        return;
      } catch (err) {
        console.warn('WebTransport failed, falling back to WebSocket:', err);
      }
    }
    const wsUrl = this.url
      .replace(/^https:\/\//i, 'wss://')
      .replace(/^http:\/\//i, 'ws://');
    await this._connectWebSocket(wsUrl);
  }

  async _connectWebTransport(url) {
    const wt = new WebTransport(url);
    await wt.ready;
    this.wt = wt;
    this.kind = 'webtransport';
    this.wtWriter = wt.datagrams.writable.getWriter();
    this.onStateChange('connected:webtransport');

    const reader = wt.datagrams.readable.getReader();
    (async () => {
      try {
        while (!this.closed) {
          const { value, done } = await reader.read();
          if (done) break;
          if (value && value.byteLength > 0) {
            this.onPacket(value instanceof Uint8Array ? value : new Uint8Array(value));
          }
        }
      } catch {
        // Ignore read closure when disconnecting cleanly.
      } finally {
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
      this.ws?.close();
    } catch {}
    try {
      this.wt?.close();
    } catch {}
    this.onStateChange('disconnected');
  }
}
