export class WebAudioMixer {
  constructor(readFileFn) {
    this.readFileFn = readFileFn;
    this.ctx = null;
    this.bufferCache = new Map();
    this.activeChannels = new Map();
    this.unlocked = false;
  }

  unlock() {
    if (!this.ctx && (window.AudioContext || window.webkitAudioContext)) {
      const Ctor = window.AudioContext || window.webkitAudioContext;
      this.ctx = new Ctor();
    }
    if (this.ctx && this.ctx.state === 'suspended') {
      this.ctx.resume().catch(() => {});
    }
    this.unlocked = true;
  }

  async processEvents(events) {
    if (!this.unlocked || !this.ctx || !Array.isArray(events)) return;
    for (const ev of events) {
      if (ev.kind === 'stop_all') {
        for (const src of this.activeChannels.values()) {
          try { src.stop(); } catch {}
        }
        this.activeChannels.clear();
      } else if (ev.kind === 'stop_channel') {
        const key = `${ev.entity}:${ev.channel}`;
        const src = this.activeChannels.get(key);
        if (src) {
          try { src.stop(); } catch {}
          this.activeChannels.delete(key);
        }
      } else if (ev.kind === 'play_2d' || ev.kind === 'play_3d') {
        this._play(ev).catch(() => {});
      }
    }
  }

  async _loadBuffer(soundPath) {
    const norm = soundPath.toLowerCase().startsWith('sound/')
      ? soundPath.toLowerCase()
      : `sound/${soundPath.toLowerCase()}`;
    if (this.bufferCache.has(norm)) return this.bufferCache.get(norm);
    const bytes = this.readFileFn?.(norm);
    if (!bytes) {
      this.bufferCache.set(norm, null);
      return null;
    }
    try {
      const copy = bytes.slice().buffer;
      const decoded = await this.ctx.decodeAudioData(copy);
      this.bufferCache.set(norm, decoded);
      return decoded;
    } catch {
      this.bufferCache.set(norm, null);
      return null;
    }
  }

  async _play(ev) {
    const buffer = await this._loadBuffer(ev.path);
    if (!buffer || !this.ctx) return;

    let vol = ev.volume ?? 1.0;
    if (ev.kind === 'play_3d' && ev.pos && ev.listener) {
      const dx = ev.pos[0] - ev.listener[0];
      const dy = ev.pos[1] - ev.listener[1];
      const dz = ev.pos[2] - ev.listener[2];
      const dist = Math.hypot(dx, dy, dz);
      if (dist >= 1800) return;
      vol *= Math.pow(1 - dist / 1800, 1.35);
    }
    if (vol <= 0.005) return;

    const src = this.ctx.createBufferSource();
    src.buffer = buffer;
    const gain = this.ctx.createGain();
    gain.gain.value = Math.min(Math.max(vol, 0), 1.5);
    src.connect(gain);
    gain.connect(this.ctx.destination);

    if (ev.entity != null && ev.channel != null && ev.channel !== 0) {
      const key = `${ev.entity}:${ev.channel}`;
      const prev = this.activeChannels.get(key);
      if (prev) {
        try { prev.stop(); } catch {}
      }
      this.activeChannels.set(key, src);
      src.onended = () => {
        if (this.activeChannels.get(key) === src) {
          this.activeChannels.delete(key);
        }
      };
    }
    src.start();
  }
}
