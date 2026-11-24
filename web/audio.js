const ROOM_WET = [
  0.0, 0.06, 0.14, 0.18, 0.24, 0.16, 0.22, 0.28, 0.34, 0.12,
  0.22, 0.32, 0.42, 0.48, 0.25, 0.32, 0.40, 0.15, 0.26, 0.35,
  0.20, 0.30, 0.38, 0.46, 0.22, 0.30, 0.40, 0.28, 0.36,
];

const ROOM_DECAY_SEC = [
  0.0, 0.25, 0.55, 0.80, 1.15, 0.45, 0.85, 1.20, 1.55, 0.35,
  0.70, 1.15, 1.65, 2.10, 0.80, 1.10, 1.45, 0.55, 0.95, 1.35,
  0.75, 1.15, 1.55, 2.00, 0.85, 1.25, 1.65, 1.10, 1.45,
];

export class WebAudioMixer {
  constructor(readFileFn) {
    this.readFileFn = readFileFn;
    this.ctx = null;
    this.bufferCache = new Map();
    this.activeChannels = new Map();
    this.voiceNextAt = new Map();
    this.unlocked = false;

    this.dryGain = null;
    this.wetGain = null;
    this.convolver = null;
    this.underwaterFilter = null;
    this.masterGain = null;

    this.musicEntry = null;
    this.roomType = 0;
  }

  unlock() {
    if (!this.ctx && (window.AudioContext || window.webkitAudioContext)) {
      const Ctor = window.AudioContext || window.webkitAudioContext;
      this.ctx = new Ctor();
      this._initGraph();
    }
    if (this.ctx && this.ctx.state === 'suspended') {
      this.ctx.resume().catch(() => {});
    }
    this.unlocked = true;
  }

  _initGraph() {
    if (!this.ctx || this.masterGain) return;
    this.dryGain = this.ctx.createGain();
    this.dryGain.gain.value = 1.0;

    this.wetGain = this.ctx.createGain();
    this.wetGain.gain.value = 0.0;

    this.convolver = this.ctx.createConvolver();
    this.underwaterFilter = this.ctx.createBiquadFilter();
    this.underwaterFilter.type = 'lowpass';
    this.underwaterFilter.frequency.value = 22000;
    this.underwaterFilter.Q.value = 0.7;

    this.masterGain = this.ctx.createGain();
    this.masterGain.gain.value = 1.0;

    this.dryGain.connect(this.underwaterFilter);
    this.convolver.connect(this.wetGain);
    this.wetGain.connect(this.underwaterFilter);
    this.underwaterFilter.connect(this.masterGain);
    this.masterGain.connect(this.ctx.destination);
  }

  async processEvents(events) {
    if (!this.unlocked || !this.ctx || !Array.isArray(events)) return;
    this._initGraph();
    for (const ev of events) {
      switch (ev.kind) {
        case 'stop_all':
          this._stopAllSfx();
          break;
        case 'stop_channel':
          this._stopChannel(ev.entity, ev.channel);
          break;
        case 'update_channel':
          this._updateChannel(ev);
          break;
        case 'set_room_type':
          this._setRoomType(ev.room_type | 0);
          break;
        case 'set_underwater':
          if (this.underwaterFilter) {
            this.underwaterFilter.frequency.setTargetAtTime(
              ev.underwater ? 550 : 22000,
              this.ctx.currentTime,
              0.04,
            );
          }
          break;
        case 'set_fade':
          if (this.masterGain) {
            this.masterGain.gain.setTargetAtTime(
              Math.min(Math.max(ev.gain ?? 1.0, 0), 1),
              this.ctx.currentTime,
              0.03,
            );
          }
          break;
        case 'play_music':
          this._playMusic(ev).catch(() => {});
          break;
        case 'set_music_volume':
          if (this.musicEntry?.gain) {
            this.musicEntry.gain.gain.value = Math.min(Math.max(ev.volume ?? 0, 0), 1);
          }
          break;
        case 'stop_music':
          this._stopMusic();
          break;
        case 'voice_pcm':
          this._playVoicePcm(ev);
          break;
        case 'play_2d':
        case 'play_3d':
          this._play(ev).catch(() => {});
          break;
        default:
          break;
      }
    }
  }

  _stopChannel(entity, channel) {
    const key = `${entity}:${channel}`;
    const entry = this.activeChannels.get(key);
    if (entry) {
      try { entry.src.stop(); } catch {}
      this.activeChannels.delete(key);
    }
  }

  _stopAllSfx() {
    for (const entry of this.activeChannels.values()) {
      try { entry.src.stop(); } catch {}
    }
    this.activeChannels.clear();
  }

  _updateChannel(ev) {
    const key = `${ev.entity}:${ev.channel}`;
    const entry = this.activeChannels.get(key);
    if (!entry) return;
    if (ev.volume != null && entry.gain) {
      entry.gain.gain.value = Math.min(Math.max(ev.volume, 0), 1.5);
    }
    if (ev.pitch != null && entry.src) {
      entry.src.playbackRate.value = Math.min(Math.max(ev.pitch, 50), 255) / 100.0;
    }
  }

  _setRoomType(roomType) {
    const idx = Math.max(0, Math.min(ROOM_WET.length - 1, roomType));
    if (idx === this.roomType || !this.ctx || !this.convolver || !this.wetGain) return;
    this.roomType = idx;
    const wet = ROOM_WET[idx] ?? 0;
    if (idx === 0 || wet <= 0.001) {
      this.wetGain.gain.value = 0.0;
      return;
    }
    const decay = ROOM_DECAY_SEC[idx] ?? 0.6;
    const rate = this.ctx.sampleRate;
    const length = Math.max(256, Math.floor(rate * decay));
    const ir = this.ctx.createBuffer(2, length, rate);
    for (let ch = 0; ch < 2; ch++) {
      const data = ir.getChannelData(ch);
      for (let i = 0; i < length; i++) {
        const env = Math.exp(-3.5 * (i / length));
        data[i] = (Math.random() * 2 - 1) * env;
      }
    }
    this.convolver.buffer = ir;
    this.wetGain.gain.value = wet;
  }

  _normalizeAudioPath(rawPath, defaultPrefix) {
    const lower = String(rawPath || '').replace(/\\/g, '/').toLowerCase();
    if (lower.startsWith('sound/') || lower.startsWith('media/')) {
      return lower;
    }
    return `${defaultPrefix}/${lower}`;
  }

  async _loadBuffer(rawPath, defaultPrefix = 'sound') {
    const norm = this._normalizeAudioPath(rawPath, defaultPrefix);
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

  async _playMusic(ev) {
    this._stopMusic();
    const buffer = await this._loadBuffer(ev.path, 'media');
    if (!buffer || !this.ctx) return;
    const src = this.ctx.createBufferSource();
    src.buffer = buffer;
    src.loop = Boolean(ev.looping);
    const gain = this.ctx.createGain();
    gain.gain.value = Math.min(Math.max(ev.volume ?? 0.5, 0), 1);
    src.connect(gain);
    gain.connect(this.ctx.destination);
    src.start();
    this.musicEntry = { src, gain };
  }

  _stopMusic() {
    if (this.musicEntry) {
      try { this.musicEntry.src.stop(); } catch {}
      this.musicEntry = null;
    }
  }

  _playVoicePcm(ev) {
    if (!this.ctx || !ev.samples || ev.samples.length === 0) return;
    const rate = Math.max(8000, Math.min(48000, ev.sample_rate || 24000));
    const buf = this.ctx.createBuffer(1, ev.samples.length, rate);
    buf.getChannelData(0).set(ev.samples);
    const src = this.ctx.createBufferSource();
    src.buffer = buf;
    const gain = this.ctx.createGain();
    gain.gain.value = Math.min(Math.max(ev.volume ?? 1.0, 0), 4.0);
    src.connect(gain);
    gain.connect(this.ctx.destination);

    const now = this.ctx.currentTime;
    const prevEnd = this.voiceNextAt.get(ev.speaker) ?? now;
    const startAt = Math.max(now, prevEnd);
    src.start(startAt);
    this.voiceNextAt.set(ev.speaker, startAt + buf.duration);
  }

  async _play(ev) {
    const buffer = await this._loadBuffer(ev.path, 'sound');
    if (!buffer || !this.ctx || !this.dryGain) return;

    let vol = ev.volume ?? 1.0;
    let pan = 0.0;
    if (ev.kind === 'play_3d' && ev.pos && ev.listener) {
      const dx = ev.pos[0] - ev.listener[0];
      const dy = ev.pos[1] - ev.listener[1];
      const dz = ev.pos[2] - ev.listener[2];
      const dist = Math.hypot(dx, dy, dz);
      const atten = ev.attenuation ?? 0.8;
      if (atten > 0) {
        const gain = Math.max(0, 1.0 - (dist * atten) / 1000.0);
        if (gain <= 0.005) return;
        vol *= gain;
      }
      if (ev.right && dist > 1e-3) {
        const dot = (dx * ev.right[0] + dy * ev.right[1] + dz * ev.right[2]) / dist;
        pan = Math.min(Math.max(dot * 0.85, -1), 1);
      }
      if (ev.occluded) {
        vol *= 0.65;
      }
    }
    if (vol <= 0.002) return;

    const src = this.ctx.createBufferSource();
    src.buffer = buffer;
    const pitch = ev.pitch ?? 100;
    src.playbackRate.value = Math.min(Math.max(pitch, 50), 255) / 100.0;

    if (ev.loop_start != null && ev.loop_end != null && ev.loop_end > ev.loop_start) {
      src.loop = true;
      src.loopStart = ev.loop_start / buffer.sampleRate;
      src.loopEnd = ev.loop_end / buffer.sampleRate;
    }

    const gain = this.ctx.createGain();
    gain.gain.value = Math.min(Math.max(vol, 0), 1.5);

    let tail = src;
    if (ev.occluded) {
      const lpf = this.ctx.createBiquadFilter();
      lpf.type = 'lowpass';
      lpf.frequency.value = 850;
      tail.connect(lpf);
      tail = lpf;
    }
    if (Math.abs(pan) > 0.01 && typeof this.ctx.createStereoPanner === 'function') {
      const panner = this.ctx.createStereoPanner();
      panner.pan.value = pan;
      tail.connect(panner);
      tail = panner;
    }
    tail.connect(gain);
    gain.connect(this.dryGain);
    if (this.convolver && this.roomType > 0) {
      gain.connect(this.convolver);
    }

    if (ev.entity != null && ev.channel != null && ev.channel !== 0) {
      const key = `${ev.entity}:${ev.channel}`;
      const prev = this.activeChannels.get(key);
      if (prev) {
        try { prev.src.stop(); } catch {}
      }
      const entry = { src, gain };
      this.activeChannels.set(key, entry);
      src.onended = () => {
        if (this.activeChannels.get(key) === entry) {
          this.activeChannels.delete(key);
        }
      };
    }

    const delay = Math.max(0, ev.delay_sec ?? 0);
    src.start(this.ctx.currentTime + delay);
  }
}
