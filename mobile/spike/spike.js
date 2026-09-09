// Mic capture spike for the Tauri iOS webview. Reports into the page so a
// simulator screenshot is the whole log. Not shipped.
const out = document.getElementById("out");
const lines = [];
const log = (s) => {
  lines.push(s);
  out.textContent = lines.join("\n");
};

function stats(chunks) {
  let n = 0;
  let sq = 0;
  let max = 0;
  let nonzero = 0;
  for (const c of chunks) {
    for (let i = 0; i < c.length; i++) {
      const v = c[i];
      n++;
      sq += v * v;
      const a = Math.abs(v);
      if (a > max) max = a;
      if (v !== 0) nonzero++;
    }
  }
  return { n, rms: n ? Math.sqrt(sq / n) : 0, max, nonzero };
}

async function captureWorklet(ctx, stream, seconds) {
  await ctx.audioWorklet.addModule("/spike-worklet.js");
  const src = ctx.createMediaStreamSource(stream);
  const node = new AudioWorkletNode(ctx, "tap", { numberOfInputs: 1, numberOfOutputs: 0 });
  const chunks = [];
  node.port.onmessage = (e) => chunks.push(e.data);
  src.connect(node);
  await new Promise((r) => setTimeout(r, seconds * 1000));
  src.disconnect();
  node.port.onmessage = null;
  return chunks;
}

async function captureScriptProcessor(ctx, stream, seconds) {
  const src = ctx.createMediaStreamSource(stream);
  const node = ctx.createScriptProcessor(4096, 1, 1);
  const chunks = [];
  node.onaudioprocess = (e) => chunks.push(Float32Array.from(e.inputBuffer.getChannelData(0)));
  src.connect(node);
  node.connect(ctx.destination);
  await new Promise((r) => setTimeout(r, seconds * 1000));
  src.disconnect();
  node.disconnect();
  return chunks;
}

async function run() {
  lines.length = 0;
  log(`url ${location.href}`);
  log(`secure ${window.isSecureContext}`);
  log(`mediaDevices ${!!navigator.mediaDevices} gUM ${!!navigator.mediaDevices?.getUserMedia}`);
  log(`MediaRecorder ${typeof MediaRecorder}`);
  if (typeof MediaRecorder !== "undefined") {
    for (const t of ["audio/mp4", "audio/mp4;codecs=pcm", "audio/webm;codecs=opus", "audio/wav"]) {
      log(`  ${t}: ${MediaRecorder.isTypeSupported(t)}`);
    }
  }
  log(`AudioWorklet ${typeof AudioWorkletNode}`);
  let stream;
  try {
    const t0 = performance.now();
    stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    const track = stream.getAudioTracks()[0];
    log(
      `gUM ok in ${Math.round(performance.now() - t0)}ms track=${track?.label} state=${track?.readyState} muted=${track?.muted}`,
    );
    log(`settings ${JSON.stringify(track?.getSettings?.() ?? {})}`);
  } catch (e) {
    log(`gUM FAILED ${e?.name}: ${e?.message}`);
    return;
  }
  for (const rate of [undefined, 16000]) {
    let ctx;
    try {
      ctx = rate ? new AudioContext({ sampleRate: rate }) : new AudioContext();
      await ctx.resume();
      log(`ctx(${rate ?? "default"}) rate=${ctx.sampleRate} state=${ctx.state}`);
    } catch (e) {
      log(`ctx(${rate ?? "default"}) FAILED ${e?.name}: ${e?.message}`);
      continue;
    }
    try {
      const chunks = await captureWorklet(ctx, stream, 3);
      const s = stats(chunks);
      log(
        `  worklet: chunks=${chunks.length} frames=${s.n} (${(s.n / ctx.sampleRate).toFixed(2)}s) rms=${s.rms.toFixed(5)} max=${s.max.toFixed(4)} nonzero=${s.nonzero}`,
      );
    } catch (e) {
      log(`  worklet FAILED ${e?.name}: ${e?.message}`);
      try {
        const chunks = await captureScriptProcessor(ctx, stream, 3);
        const s = stats(chunks);
        log(
          `  scriptproc: frames=${s.n} rms=${s.rms.toFixed(5)} max=${s.max.toFixed(4)} nonzero=${s.nonzero}`,
        );
      } catch (e2) {
        log(`  scriptproc FAILED ${e2?.name}: ${e2?.message}`);
      }
    }
    await ctx.close();
  }
  if (typeof MediaRecorder !== "undefined") {
    try {
      const rec = new MediaRecorder(stream);
      const blobs = [];
      rec.ondataavailable = (e) => blobs.push(e.data);
      rec.start();
      await new Promise((r) => setTimeout(r, 2000));
      await new Promise((r) => {
        rec.onstop = r;
        rec.stop();
      });
      const size = blobs.reduce((a, b) => a + b.size, 0);
      log(`recorder mime=${rec.mimeType} blobs=${blobs.length} bytes=${size}`);
    } catch (e) {
      log(`recorder FAILED ${e?.name}: ${e?.message}`);
    }
  }
  for (const t of stream.getTracks()) t.stop();
  log("done");
}

document
  .getElementById("go")
  .addEventListener("click", () => run().catch((e) => log(`run FAILED ${e}`)));
// Auto-start: the simulator has no tap command.
setTimeout(() => run().catch((e) => log(`run FAILED ${e}`)), 1500);
