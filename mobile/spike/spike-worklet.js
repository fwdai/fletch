class Tap extends AudioWorkletProcessor {
  process(inputs) {
    const ch = inputs[0]?.[0];
    if (ch) this.port.postMessage(Float32Array.from(ch));
    return true;
  }
}
registerProcessor("tap", Tap);
