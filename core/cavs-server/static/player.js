// CAVS-1 web player: JSON control plane + binary CVSP batches processed in
// WASM (BLAKE3 verification + ref resolution) + MSE playback.
'use strict';

const $ = (sel) => document.querySelector(sel);
let wasm = null;

function log(msg) {
  $('#log').textContent += msg + '\n';
}

async function loadWasm() {
  const resp = await fetch('/web/cavs_web.wasm');
  if (!resp.ok) {
    throw new Error(
      'cavs_web.wasm no disponible. Compílalo con:\n' +
      '  cargo build -p cavs-web --target wasm32-unknown-unknown --release'
    );
  }
  const { instance } = await WebAssembly.instantiate(await resp.arrayBuffer(), {});
  wasm = instance.exports;
}

// Copy bytes out of WASM memory (memory.buffer can be detached on growth,
// so views are created fresh and sliced immediately).
function wasmBytes(ptr, len) {
  return new Uint8Array(wasm.memory.buffer, ptr, len).slice();
}

function wasmError() {
  return new TextDecoder().decode(wasmBytes(wasm.cavs_error_ptr(), wasm.cavs_error_len()));
}

// Feed one CVSP batch into WASM; returns reconstructed outputs.
function processBatch(buf) {
  const bytes = new Uint8Array(buf);
  const ptr = wasm.cavs_alloc(bytes.length);
  new Uint8Array(wasm.memory.buffer, ptr, bytes.length).set(bytes);
  if (wasm.cavs_process_batch(ptr, bytes.length) !== 0) {
    throw new Error('WASM: ' + wasmError());
  }
  const outs = [];
  for (let i = 0; i < wasm.cavs_out_count(); i++) {
    outs.push({
      kind: wasm.cavs_out_kind(i), // 0 = init, 1 = segment
      track: wasm.cavs_out_track(i),
      segment: wasm.cavs_out_segment(i),
      bytes: wasmBytes(wasm.cavs_out_ptr(i), wasm.cavs_out_len(i)),
    });
  }
  wasm.cavs_clear_outputs();
  return outs;
}

function haveSet() {
  const len = wasm.cavs_have_build();
  return JSON.parse(new TextDecoder().decode(wasmBytes(wasm.cavs_have_ptr(), len)));
}

function humanBytes(n) {
  const units = ['B', 'KiB', 'MiB', 'GiB'];
  let u = 0;
  while (n >= 1024 && u < units.length - 1) { n /= 1024; u++; }
  return u === 0 ? `${n} B` : `${n.toFixed(2)} ${units[u]}`;
}

function updateStats(known) {
  $('#s-inline').textContent = humanBytes(wasm.cavs_stats_inline_bytes());
  $('#s-refs').textContent = wasm.cavs_stats_refs();
  $('#s-cache').textContent = wasm.cavs_cache_chunks();
  if (known !== undefined) $('#s-known').textContent = known;
}

// Map the packer's codec hint ("h264+aac") to an MSE mime string. The init
// segment drives actual decoding, so a representative RFC 6381 string works.
function mseMime(codec) {
  const hasAudio = /aac|\+/.test(codec);
  const videoCandidates = ['avc1.640028', 'avc1.64001f', 'avc1.4d401f', 'avc1.42e01e'];
  for (const v of videoCandidates) {
    const mime = hasAudio
      ? `video/mp4; codecs="${v}, mp4a.40.2"`
      : `video/mp4; codecs="${v}"`;
    if (MediaSource.isTypeSupported(mime)) return mime;
  }
  return 'video/mp4; codecs="avc1.64001f, mp4a.40.2"';
}

function appendAsync(sb, bytes) {
  return new Promise((resolve, reject) => {
    sb.addEventListener('updateend', resolve, { once: true });
    sb.addEventListener('error', () => reject(new Error('SourceBuffer append failed')), { once: true });
    sb.appendBuffer(bytes);
  });
}

const BATCH_SEGMENTS = 8;

async function play(assetName, warm) {
  const manifest = await (await fetch(`/api/assets/${assetName}/manifest`)).json();
  const track = manifest.tracks.find((t) => t.kind === 'video');
  if (!track) throw new Error('el asset no tiene pista de video');
  const segs = manifest.segments
    .filter((s) => s.track_id === track.track_id)
    .sort((a, b) => a.pts_start - b.pts_start || a.segment_id - b.segment_id);

  const have = warm ? haveSet() : [];
  const session = await (await fetch(`/api/assets/${assetName}/sessions`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ have }),
  })).json();
  log(`sesión ${warm ? 'caliente' : 'fría'} ${session.session_id.slice(0, 8)}… ` +
      `(servidor reconoce ${session.known_chunks} chunks)`);

  const video = $('#video');
  const ms = new MediaSource();
  video.src = URL.createObjectURL(ms);
  await new Promise((r) => ms.addEventListener('sourceopen', r, { once: true }));
  const sb = ms.addSourceBuffer(mseMime(track.codec));

  let first = true;
  for (let i = 0; i < segs.length; i += BATCH_SEGMENTS) {
    const group = segs.slice(i, i + BATCH_SEGMENTS).map((s) => s.segment_id);
    const req = { track_inits: first ? [track.track_id] : [], segment_ids: group };
    first = false;
    const resp = await fetch(`/api/sessions/${session.session_id}/batch`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(req),
    });
    if (!resp.ok) throw new Error(`batch: HTTP ${resp.status}`);
    for (const out of processBatch(await resp.arrayBuffer())) {
      await appendAsync(sb, out.bytes);
    }
    updateStats(session.known_chunks);
  }
  if (ms.readyState === 'open') ms.endOfStream();
  video.play().catch(() => {}); // autoplay may need a user gesture
  $('#replay').disabled = false;
}

async function main() {
  await loadWasm();
  const assets = await (await fetch('/api/assets')).json();
  const select = $('#asset');
  for (const a of assets) {
    const opt = document.createElement('option');
    opt.value = a.name;
    opt.textContent = `${a.name} (${a.segments} segmentos)`;
    select.appendChild(opt);
  }
  const run = (warm) => {
    $('#play').disabled = true;
    play(select.value, warm)
      .catch((e) => log('ERROR: ' + e.message))
      .finally(() => { $('#play').disabled = false; });
  };
  $('#play').addEventListener('click', () => run(false));
  $('#replay').addEventListener('click', () => run(true));
}

main().catch((e) => log('ERROR: ' + e.message));
