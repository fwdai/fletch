// The mic tap, on the audio rendering thread. Plain JS because an
// AudioWorklet module is loaded by URL, not bundled with the app; it is
// referenced from capture.ts as `new URL("./worklet.js", import.meta.url)`.
//
// Nothing happens here but a copy: the render quantum's first channel is
// posted to the main thread, which batches, converts and sends. Averaging
// channels down would belong here too, but `getUserMedia` audio is mono.
class PcmTap extends AudioWorkletProcessor {
  process(inputs) {
    const channel = inputs[0]?.[0];
    if (channel) this.port.postMessage(Float32Array.from(channel));
    return true;
  }
}

registerProcessor("fletch-pcm-tap", PcmTap);
