// Orbit camera with spring-smoothed parameters and three presets. Distance is always
// derived from the field of view so the box stays framed: a preset that drops the fov
// toward 1 degree becomes a dolly zoom into a flat 2D view instead of a jump cut.
// Only the framed part (size.depth) is fitted; the tail behind it runs out of frame.

import { perspective, lookAt, multiply, invert } from './mat4.js';

const DEG = Math.PI / 180;
const KEYS = ['yaw', 'pitch', 'logFov', 'logZoom', 'px', 'py', 'pz'];

// Critically damped spring step (the usual SmoothDamp form).
function smoothDamp(current, target, state, key, smoothTime, dt) {
  const omega = 2 / Math.max(smoothTime, 1e-4);
  const x = omega * dt;
  const exp = 1 / (1 + x + 0.48 * x * x + 0.235 * x * x * x);
  const change = current - target;
  const temp = (state[key] + omega * change) * dt;
  state[key] = (state[key] - omega * temp) * exp;
  return target + (change + temp) * exp;
}

export class OrbitCamera {
  constructor(look) {
    this.look = look;
    this.current = this.fromView(look.views.perspective);
    this.target = { ...this.current };
    this.velocity = Object.fromEntries(KEYS.map((k) => [k, 0]));
    this.smoothTime = look.motion.presetSeconds;
    this.view = 'perspective';
  }

  fromView(v) {
    return { yaw: v.yaw, pitch: v.pitch, logFov: Math.log(v.fov), logZoom: Math.log(v.zoom), px: v.pan[0], py: v.pan[1], pz: v.pan[2] };
  }

  setView(name) {
    const v = this.fromView(this.look.views[name]);
    v.yaw += 360 * Math.round((this.current.yaw - v.yaw) / 360);  // shortest way round
    this.target = v;
    this.smoothTime = this.look.motion.presetSeconds;
    this.view = name;
  }

  // Where the camera is heading, to come back to later (after the spectrum-only view).
  snapshot() { return { view: this.view, target: { ...this.target } }; }

  restore(snap) {
    this.target = { ...snap.target };
    this.target.yaw += 360 * Math.round((this.current.yaw - this.target.yaw) / 360);
    this.smoothTime = this.look.motion.presetSeconds;
    this.view = snap.view;
  }

  get settled() {
    return KEYS.every((k) => Math.abs(this.current[k] - this.target[k]) < 1e-3 && Math.abs(this.velocity[k]) < 1e-3);
  }

  // Drag: one finger or mouse orbits; leaving a flat view widens the lens again.
  orbit(dxPx, dyPx) {
    const { motion } = this.look;
    if (Math.exp(this.target.logFov) < 10) this.target.logFov = Math.log(this.look.freeFov);
    this.target.yaw -= dxPx * 0.3;
    this.target.pitch = Math.min(motion.pitchMax, Math.max(motion.pitchMin, this.target.pitch + dyPx * 0.3));
    this.smoothTime = motion.dragSeconds;
    this.view = 'free';
  }

  zoomBy(factor) {
    this.target.logZoom = Math.min(Math.log(3), Math.max(Math.log(0.25), this.target.logZoom + Math.log(factor)));
    this.smoothTime = this.look.motion.dragSeconds * 2;
  }

  panBy(dxPx, dyPx, viewportH) {
    const { right, up, dist } = this.basis(this.current);
    const s = (2 * dist * Math.tan(Math.exp(this.current.logFov) * DEG / 2)) / viewportH;
    this.target.px -= (right[0] * dxPx - up[0] * dyPx) * s;
    this.target.py -= (right[1] * dxPx - up[1] * dyPx) * s;
    this.target.pz -= (right[2] * dxPx - up[2] * dyPx) * s;
    this.smoothTime = this.look.motion.dragSeconds;
  }

  update(dt) {
    for (const k of KEYS) {
      this.current[k] = smoothDamp(this.current[k], this.target[k], this.velocity, k, this.smoothTime, dt);
    }
  }

  basis(c, aspect = 1) {
    const yaw = c.yaw * DEG, pitch = c.pitch * DEG;
    const back = [Math.cos(pitch) * Math.sin(yaw), Math.sin(pitch), Math.cos(pitch) * Math.cos(yaw)];
    const right = [Math.cos(yaw), 0, -Math.sin(yaw)];
    const up = [
      back[1] * right[2] - back[2] * right[1],
      back[2] * right[0] - back[0] * right[2],
      back[0] * right[1] - back[1] * right[0],
    ];
    return { back, right, up, dist: this.fitDistance(c, back, right, up, aspect) };
  }

  // Smallest distance that keeps all eight box corners inside the frame for this lens,
  // orientation and aspect, then scaled by zoom.
  fitDistance(c, back, right, up, aspect) {
    const { width, height, depth } = this.look.size;
    const tanV = Math.tan(Math.exp(c.logFov) * DEG / 2);
    const tanH = tanV * aspect;
    let d = 0;
    for (const x of [-width / 2, width / 2]) for (const y of [0, height]) for (const z of [-depth / 2, depth / 2]) {
      const p = [x, y - height / 2, z];
      const px = p[0] * right[0] + p[1] * right[1] + p[2] * right[2];
      const py = p[0] * up[0] + p[1] * up[1] + p[2] * up[2];
      const toward = p[0] * back[0] + p[1] * back[1] + p[2] * back[2];
      d = Math.max(d, Math.abs(px) / tanH + toward, Math.abs(py) / tanV + toward);
    }
    return d * 1.06 * Math.exp(c.logZoom);
  }

  matrices(aspect) {
    const c = this.current;
    const { back, dist } = this.basis(c, aspect);
    const { width, height, depth, tail } = this.look.size;
    const center = [c.px, height / 2 + c.py, c.pz];
    const eye = center.map((v, i) => v + back[i] * dist);
    const radius = Math.hypot(width, height, depth) / 2;
    const near = Math.max(dist * 0.01, dist - radius * 2.5);
    const far = dist + radius * 2.5 + depth * tail;  // the tail runs on behind the framed box
    const up = c.pitch > 89 ? [-Math.sin(c.yaw * DEG), 0, -Math.cos(c.yaw * DEG)] : [0, 1, 0];
    const view = lookAt(eye, center, up);
    const proj = perspective(Math.exp(c.logFov) * DEG, aspect, near, far);
    const viewProj = multiply(proj, view);
    return { viewProj, inverse: invert(viewProj), eye, back };
  }
}
