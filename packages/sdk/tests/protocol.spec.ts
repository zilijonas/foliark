import { describe, expect, it } from "vitest";
import {
  Correlator, ERROR_CODES, FoliarkError, checkRequest, errRes, isErrorCode, isErrorWire, okRes, postTransfer,
  type Inbound,
} from "../src/protocol.js";
import type { ErrorCode } from "../src/generated/ErrorCode.js";

const geometry = {
  index: 0, width: 200, height: 100, rotation: 0, userUnit: 1, mediaBox: [0, 0, 200, 100], cropBox: [0, 0, 200, 100],
  userToDisplay: [1, 0, 0, -1, 0, 100], displayToUser: [1, 0, 0, -1, 0, 100],
};
const header = (w: number, h: number) => ({ width: w, height: h, scale: 1, clip: { x: 0, y: 0, width: w, height: h }, complete: true, warnings: [] });
const pending = () => {
  const c = new Correlator<string>();
  return { c, id: c.register("renderPage", "ctx") };
};
const kind = (i: Inbound<string>) => (i.kind === "dropped" ? `dropped:${i.reason}` : i.kind);
const failure = (i: Inbound<string>) => {
  expect(i.kind).toBe("failure");
  return (i as Extract<Inbound<string>, { kind: "failure" }>).error;
};
// A port that really transfers, like Worker.postMessage.
const realPort = { postMessage: (m: unknown, o: { transfer: Transferable[] }) => void structuredClone(m, o) };

describe("malformed messages", () => {
  it("drops what carries no readable id and fails a pending request whose answer is unusable", () => {
    for (const bad of [null, "x", 7, [], {}, { v: 1 }]) {
      expect(kind(new Correlator<string>().receive(bad))).toBe("dropped:malformed");
    }
    for (const bad of [
      { v: 2, id: 1, ok: true, result: null }, // wrong version
      { v: 1, id: 1, ok: true }, // no result
      { v: 1, id: 1, ok: false, error: { code: "NOPE", message: "m", recoverable: false } }, // unknown code
      { v: 1, id: 1, ok: false, error: { code: "INTERNAL", message: "m" } }, // no recoverable
    ]) {
      const { c } = pending();
      expect(failure(c.receive(bad)).code).toBe("PROTOCOL_ERROR");
      expect(kind(c.receive(okRes(1, null)))).toBe("dropped:duplicate"); // it was settled, not left hanging
    }
  });

  it("rejects bad requests with PROTOCOL_ERROR, keeping the id so the worker can answer", () => {
    const ok = { v: 1, id: 4, gen: 1, op: "pages", doc: "7", args: {} };
    expect(checkRequest(ok, 1).ok).toBe(true);
    for (const bad of [
      { ...ok, v: 2 }, { ...ok, op: "nope" }, { ...ok, doc: undefined }, { ...ok, args: undefined },
      { ...ok, op: "open", doc: undefined, args: { bytes: new Uint8Array(4) } }, // a view, not an ArrayBuffer
      { ...ok, op: "renderPage", args: { page: -1 } },
    ]) {
      const r = checkRequest(bad, 1);
      expect(r.ok).toBe(false);
      if (!r.ok) expect([r.id, r.error.code]).toEqual([4, "PROTOCOL_ERROR"]);
    }
    expect(checkRequest("junk", 1)).toMatchObject({ ok: false, id: undefined });
  });
});

describe("generation and correlation", () => {
  it("rejects a request from a previous worker generation as WORKER_RESET", () => {
    const r = checkRequest({ v: 1, id: 2, gen: 1, op: "pages", doc: "7", args: {} }, 2);
    expect(r).toMatchObject({ ok: false, id: 2, error: { code: "WORKER_RESET", recoverable: true } });
  });

  it("drops responses and events from before a reset, and hands back the orphaned requests", () => {
    const { c, id } = pending();
    expect(c.reset()).toEqual(["ctx"]);
    expect(kind(c.receive(okRes(id, null)))).toBe("dropped:stale-generation");
    const fresh = c.register("close", "new");
    expect(c.receive(okRes(fresh, null))).toMatchObject({ kind: "result", ctx: "new" });
    expect(kind(c.receive({ v: 1, event: "fatal", gen: 1 }))).toBe("dropped:stale-event");
    expect(c.receive({ v: 1, event: "fatal", gen: 2, reason: "boom" })).toMatchObject({ kind: "event", event: "fatal" });
  });

  it("drops a duplicate response and an unknown request id", () => {
    const c = new Correlator<string>();
    const id = c.register("close", "a");
    expect(c.receive(okRes(id, null)).kind).toBe("result");
    expect(kind(c.receive(okRes(id, null)))).toBe("dropped:duplicate");
    expect(kind(c.receive(okRes(id + 5, null)))).toBe("dropped:unknown-id");
  });

  it("validates a result against its op, so a wrong shape cannot reach the caller", () => {
    const { c, id } = pending();
    expect(failure(c.receive(okRes(id, { docId: 7, pageCount: 1 }))).code).toBe("PROTOCOL_ERROR");
    const c2 = new Correlator<string>();
    const id2 = c2.register("pages", "p");
    expect(failure(c2.receive(okRes(id2, [{ ...geometry, rotation: 45 }]))).code).toBe("PROTOCOL_ERROR");
    const id3 = c2.register("pages", "p");
    expect(c2.receive(okRes(id3, [geometry])).kind).toBe("result");
  });
});

describe("buffer ownership", () => {
  it("accepts a render result only when its pixel buffer is exactly width*height*4", () => {
    for (const [data, expected] of [
      [new ArrayBuffer(4 * 2 * 3 * 4), "result"],
      [new ArrayBuffer(4 * 2 * 3 * 4 - 4), "failure"], // short
      [new ArrayBuffer(4 * 2 * 3 * 4 + 4), "failure"], // long
      [new ArrayBuffer(0), "failure"], // missing
      [new Uint8ClampedArray(96), "failure"], // a view, not a buffer
      [undefined, "failure"],
    ] as const) {
      const { c, id } = pending();
      expect(c.receive(okRes(id, { ...header(4, 2 * 3), data })).kind).toBe(expected);
    }
  });

  it("refuses an oversized buffer on both the result and the request side", () => {
    const c = new Correlator<string>(16);
    const id = c.register("renderPage", "x");
    expect(failure(c.receive(okRes(id, { ...header(2, 3), data: new ArrayBuffer(24) }))).code).toBe("PROTOCOL_ERROR");
    const open = { v: 1, id: 1, gen: 1, op: "open", args: { bytes: new ArrayBuffer(17) } };
    expect(checkRequest(open, 1, 16).ok).toBe(false);
    expect(checkRequest({ ...open, args: { bytes: new ArrayBuffer(16) } }, 1, 16).ok).toBe(true);
  });

  it("detaches transferred buffers and refuses to send one twice", () => {
    const bytes = new ArrayBuffer(8);
    const msg = { v: 1, id: 1, gen: 1, op: "open", args: { bytes } };
    postTransfer(realPort, msg, [bytes]);
    expect(bytes.byteLength).toBe(0);
    expect(() => postTransfer(realPort, msg, [bytes])).toThrowError(expect.objectContaining({ code: "PROTOCOL_ERROR" }));
  });

  it("fails loudly when the host did not take ownership, and on duplicate or shared buffers", () => {
    const bytes = new ArrayBuffer(8);
    const copyingPort = { postMessage: (m: unknown) => void structuredClone(m) };
    expect(() => postTransfer(copyingPort, {}, [bytes])).toThrowError(expect.objectContaining({ code: "INTERNAL" }));
    expect(() => postTransfer(realPort, {}, [bytes, bytes])).toThrowError(expect.objectContaining({ code: "PROTOCOL_ERROR" }));
    expect(() => postTransfer(realPort, {}, [new SharedArrayBuffer(8) as unknown as ArrayBuffer])).toThrowError(
      expect.objectContaining({ code: "PROTOCOL_ERROR" }),
    );
  });
});

describe("error codes over the wire", () => {
  const codes = Object.keys(ERROR_CODES) as ErrorCode[];
  it("round trips every code with its details through a structured clone", () => {
    expect(codes).toHaveLength(35);
    const details = { limit: "maxPages", actual: 31, max: 30, pages: [1, 2], fields: ["a"], editId: "e1", chars: "x", neededLines: 3, maxLines: 2, objRef: "12 0", feature: "f" };
    for (const code of codes) {
      const c = new Correlator<string>();
      const id = c.register("close", "x");
      const sent = structuredClone(errRes(id, new FoliarkError(code, "msg", { details, recoverable: code === "WORKER_RESET" })));
      const e = failure(c.receive(sent));
      expect(e).toBeInstanceOf(FoliarkError);
      expect({ code: e.code, message: e.message, details: e.details, recoverable: e.recoverable }).toEqual({
        code, message: "msg", details, recoverable: code === "WORKER_RESET",
      });
    }
  });

  it("rejects codes and details that are not in the contract", () => {
    expect(isErrorCode("INTERNAL")).toBe(true);
    expect(isErrorCode("toString")).toBe(false); // not an inherited property
    expect(isErrorWire({ code: "INTERNAL", message: "m", recoverable: false, details: { pages: ["1"] } })).toBe(false);
    expect(isErrorWire({ code: "INTERNAL", message: "m", recoverable: false })).toBe(true);
  });
});
