export class WebInputController {
  constructor(canvas, cs16Web, { onGesture } = {}) {
    this.canvas = canvas;
    this.cs16Web = cs16Web;
    this.onGesture = onGesture || (() => {});
    this.touchMove = { lx: 0, ly: 0 };
    this.touchLookDelta = { dx: 0, dy: 0 };
    this.touchButtons = 0;
    this._bindEvents();
  }

  async lockPointer() {
    this.onGesture();
    if (!this.canvas || typeof this.canvas.requestPointerLock !== 'function') return;
    try {
      const maybePromise = this.canvas.requestPointerLock({ unadjustedMovement: true });
      if (maybePromise && typeof maybePromise.catch === 'function') {
        await maybePromise.catch(() => this.canvas.requestPointerLock());
      }
    } catch {
      this.canvas.requestPointerLock();
    }
  }

  toggleFullscreen() {
    this.onGesture();
    if (!document.fullscreenElement) {
      document.documentElement.requestFullscreen?.().catch(() => {});
    } else {
      document.exitFullscreen?.().catch(() => {});
    }
  }

  _bindEvents() {
    this.canvas.addEventListener('click', () => {
      this.onGesture();
      if (document.pointerLockElement !== this.canvas) {
        this.lockPointer();
      }
    });

    window.addEventListener('keydown', (e) => {
      if (document.activeElement && ['INPUT', 'SELECT', 'TEXTAREA'].includes(document.activeElement.tagName)) {
        return;
      }
      this.onGesture();
      if (e.repeat) return;
      if (e.code === 'F11') {
        e.preventDefault();
        this.toggleFullscreen();
        return;
      }
      this.cs16Web.onKey?.(e.code, true);
    });

    window.addEventListener('keyup', (e) => {
      this.cs16Web.onKey?.(e.code, false);
    });

    window.addEventListener('mousemove', (e) => {
      if (document.pointerLockElement === this.canvas) {
        this.cs16Web.onMouseMove?.(e.movementX || 0, e.movementY || 0);
      }
    });

    this.canvas.addEventListener('mousedown', (e) => {
      this.onGesture();
      if (document.pointerLockElement === this.canvas) {
        e.preventDefault();
        this.cs16Web.onMouseButton?.(e.button, true);
      }
    });

    window.addEventListener('mouseup', (e) => {
      this.cs16Web.onMouseButton?.(e.button, false);
    });

    this.canvas.addEventListener('wheel', (e) => {
      if (document.pointerLockElement === this.canvas) {
        e.preventDefault();
        this.cs16Web.onMouseWheel?.(e.deltaY, e.deltaMode);
      }
    }, { passive: false });
  }

  pollGamepadAndTouch() {
    if (typeof navigator.getGamepads === 'function') {
      const pads = navigator.getGamepads();
      const pad = pads && [...pads].find((p) => p && p.connected);
      if (pad) {
        const ax = (i) => (i < pad.axes.length ? pad.axes[i] : 0);
        const btn = (i) => (i < pad.buttons.length && pad.buttons[i].pressed ? 1 : 0);
        const mask =
          (btn(7) ? 1 : 0) |           // RT: fire
          (btn(6) ? (1 << 1) : 0) |    // LT: secondary attack
          (btn(1) ? (1 << 2) : 0) |    // B: duck
          (btn(2) ? (1 << 3) : 0) |    // X: reload
          (btn(3) ? (1 << 4) : 0) |    // Y: use
          (btn(0) ? (1 << 5) : 0);     // A: jump
        this.cs16Web.onGamepad?.(ax(0), ax(1), ax(2), ax(3), mask);
      }
    }

    if (
      this.touchMove.lx !== 0 ||
      this.touchMove.ly !== 0 ||
      this.touchLookDelta.dx !== 0 ||
      this.touchLookDelta.dy !== 0 ||
      this.touchButtons !== 0
    ) {
      this.cs16Web.onTouch?.(
        this.touchMove.lx,
        this.touchMove.ly,
        this.touchLookDelta.dx,
        this.touchLookDelta.dy,
        this.touchButtons
      );
      this.touchLookDelta.dx = 0;
      this.touchLookDelta.dy = 0;
    }
  }
}
