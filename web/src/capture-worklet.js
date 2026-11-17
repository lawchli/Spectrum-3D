// AudioWorklet that hands the input to the main thread, mixed to mono, one render quantum
// (128 frames) per message. audio.js keeps it in a ring buffer and runs its own FFTs on it.
// With nothing connected it still sends silence, so the audio clock keeps running.

class Capture extends AudioWorkletProcessor {
  process(inputs) {
    const input = inputs[0];
    const out = new Float32Array(input?.[0]?.length || 128);
    if (input?.length) {
      const g = 1 / input.length;
      for (const ch of input) for (let i = 0; i < out.length; i++) out[i] += ch[i] * g;
    }
    this.port.postMessage(out, [out.buffer]);
    return true;
  }
}

registerProcessor('capture', Capture);
