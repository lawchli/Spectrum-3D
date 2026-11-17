// Game controllers through the browser's Gamepad API, standard mapping (Xbox, PlayStation
// and most other pads): right stick turns the camera and left stick moves it, as in most
// games; triggers zoom, face buttons pick views. Browsers only list a pad after one of its
// buttons is pressed on the page.

const DEAD_ZONE = 0.15;
const ORBIT_PX_PER_S = 420;  // full stick, in the camera's drag pixels (0.3 degree each)
const PAN_PX_PER_S = 520;
const ZOOM_PER_S = 1.6;      // log zoom per second at full trigger

const BUTTON = { a: 0, b: 1, x: 2, y: 3, lt: 6, rt: 7 };

function axis(v) {
  const m = Math.abs(v);
  if (m < DEAD_ZONE) return 0;
  // Rescale past the dead zone and square it, for fine control near the centre.
  const x = (m - DEAD_ZONE) / (1 - DEAD_ZONE);
  return Math.sign(v) * x * x;
}

export class GamepadControl {
  constructor() {
    this.pressed = new Set();
    this.name = null;
  }

  pad() {
    const pads = [...(navigator.getGamepads?.() || [])].filter((p) => p && p.connected);
    return pads.find((p) => p.mapping === 'standard') || pads[0] || null;
  }

  // Reads the pad once per frame. Returns null without one, else stick and trigger motion
  // for this frame plus the buttons newly pressed.
  poll(dt) {
    const pad = this.pad();
    this.name = pad ? pad.id.replace(/\s*\(.*\)\s*$/, '') || pad.id : null;
    if (!pad) return null;
    const value = (i) => pad.buttons[i]?.value ?? 0;
    const down = new Set();
    for (const [name, i] of Object.entries(BUTTON)) {
      const on = pad.buttons[i]?.pressed;
      if (on && !this.pressed.has(name)) down.add(name);
      if (on) this.pressed.add(name); else this.pressed.delete(name);
    }
    const [lx = 0, ly = 0, rx = 0, ry = 0] = pad.axes;
    // Stick right or up moves the camera right or up around the waterfall.
    return {
      orbit: [-axis(rx) * ORBIT_PX_PER_S * dt, -axis(ry) * ORBIT_PX_PER_S * dt],
      pan: [-axis(lx) * PAN_PX_PER_S * dt, -axis(ly) * PAN_PX_PER_S * dt],
      zoom: Math.exp((value(BUTTON.lt) - value(BUTTON.rt)) * ZOOM_PER_S * dt),
      down,
    };
  }
}
