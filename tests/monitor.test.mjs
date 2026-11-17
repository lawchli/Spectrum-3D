import test from 'node:test';
import assert from 'node:assert/strict';
import { pack, niceCeiling } from '../web/src/monitor.js';

// The same cases as the desktop monitor (rust/spectrum-wgpu/src/monitor.rs) and the Android
// demo (android/app/src/test/.../MonitorTest.kt), so the three pack and scale alike.
test('monitor groups take the fewest rows that fit, earlier rows taking the extra one', () => {
  const five = [240, 160, 120, 200, 230];
  assert.deepEqual(pack(five, 1050, 25), [5]);
  assert.deepEqual(pack(five, 1049, 25), [3, 2]);
  assert.deepEqual(pack(five, 570, 25), [3, 2]);
  assert.deepEqual(pack(five, 569, 25), [2, 2, 1]);
  assert.deepEqual(pack(five, 100, 25), [1, 1, 1, 1, 1]);
  assert.deepEqual(pack([], 400, 25), []);
});

test('the memory chart top steps through 1, 1.5, 2, 3, 4, 5, 6, 8 times a power of ten', () => {
  assert.equal(niceCeiling(0), 1);
  assert.equal(niceCeiling(NaN), 1);
  assert.equal(niceCeiling(1.2), 1.5);
  assert.equal(niceCeiling(260), 300);
  assert.equal(niceCeiling(300), 300);
  assert.equal(niceCeiling(810), 1000);
  assert.equal(niceCeiling(1600), 2000);
});
