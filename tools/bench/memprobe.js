// Worker-side probe: records every WebAssembly.Memory created or exported here and answers a "q" on the
// BroadcastChannel "bench-mem" with their summed byteLength (the wasm linear memory, not process RSS).
const mems = new Set();
const Mem = WebAssembly.Memory;
WebAssembly.Memory = new Proxy(Mem, { construct: (t, a) => { const m = new t(...a); mems.add(m); return m; } });
for (const k of ["instantiate", "instantiateStreaming"]) {
  const f = WebAssembly[k];
  WebAssembly[k] = async (...a) => {
    const r = await f.apply(WebAssembly, a);
    for (const v of Object.values((r.instance ?? r).exports)) if (v instanceof Mem) mems.add(v);
    return r;
  };
}
const ch = new BroadcastChannel("bench-mem");
ch.onmessage = () => ch.postMessage({ wasmMemoryBytes: [...mems].reduce((s, m) => s + m.buffer.byteLength, 0), memories: mems.size });
