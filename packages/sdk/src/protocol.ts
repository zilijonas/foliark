// Worker protocol (spec 10.4): runtime guards for every message, request-id correlation, buffer ownership.
// Pure and dependency free so both sides (and vitest) share it. The shapes come from Rust via ./generated;
// the `satisfies Record<...>` tables below stop compiling when a generated union gains or loses a member.
// Messages are never logged and error text never includes document content.
import type { Ctl } from "./generated/Ctl.js";
import type { ErrorCode } from "./generated/ErrorCode.js";
import type { ErrorDetails } from "./generated/ErrorDetails.js";
import type { ErrorWire } from "./generated/ErrorWire.js";
import type { OpName } from "./generated/OpName.js";
import type { PageGeometry } from "./generated/PageGeometry.js";
import type { Phase } from "./generated/Phase.js";
import type { Prog } from "./generated/Prog.js";
import type { RenderedPageHeader } from "./generated/RenderedPageHeader.js";
import type { Req } from "./generated/Req.js";
import type { Res } from "./generated/Res.js";

export const PROTOCOL_VERSION = 1;
/** Default cap for one transferred buffer: spec 9 `maxOutputBytes`. Callers pass their `Limits` value. */
export const DEFAULT_MAX_BUFFER_BYTES = 256 * 1024 * 1024;

// ---- errors -------------------------------------------------------------------------------------

export const ERROR_CODES = {
  INVALID_ARGUMENT: true, MALFORMED_PDF: true, REPAIRED_PDF: true, DECODE_FAILED: true, PASSWORD_REQUIRED: true,
  PASSWORD_INCORRECT: true, ENCRYPTION_UNSUPPORTED: true, LIMIT_EXCEEDED: true, OUT_OF_MEMORY: true,
  ASSET_LOAD_FAILED: true, FONT_UNSUPPORTED: true, EDIT_NO_STYLE: true, EDIT_ROTATED_TEXT: true, EDIT_NO_FONT: true,
  EDIT_NO_FIT: true, EDIT_UNSUPPORTED_SCRIPT: true, EDIT_OVERLAP: true, STALE_REVISION: true, STALE_PLAN: true,
  PLAN_CONSUMED: true, FORM_APPEARANCE_MISSING: true, XFA_UNSUPPORTED: true, OPTIONAL_CONTENT_UNSUPPORTED: true,
  SIGNED_INPUT: true, REDACT_PATTERN_UNSUPPORTED: true, REDACT_VERIFY_FAILED: true, RENDER_INCOMPLETE: true,
  CONSENT_REQUIRED: true, CANCELLED: true, WORKER_RESET: true, DOCUMENT_GONE: true, DOCUMENT_CLOSED: true,
  CLIENT_DISPOSED: true, PROTOCOL_ERROR: true, INTERNAL: true,
} as const satisfies Record<ErrorCode, true>;

/** Spec 12 `FoliarkError`. `recoverable` is decided by the side that raises it. */
export class FoliarkError extends Error {
  readonly code: ErrorCode;
  readonly details?: ErrorDetails;
  docId?: string;
  revision?: number;
  readonly recoverable: boolean;
  constructor(code: ErrorCode, message: string, o: { details?: ErrorDetails; recoverable?: boolean } = {}) {
    super(message);
    this.name = "FoliarkError";
    this.code = code;
    this.details = o.details;
    this.recoverable = o.recoverable ?? false;
  }
  toWire(): ErrorWire {
    const w: ErrorWire = { code: this.code, message: this.message, recoverable: this.recoverable };
    if (this.details !== undefined) w.details = this.details;
    return w;
  }
  static fromWire(w: ErrorWire): FoliarkError {
    return new FoliarkError(w.code, w.message, { details: w.details, recoverable: w.recoverable });
  }
}

/** Anything thrown that is not already a `FoliarkError` is an engine bug (spec 12 INTERNAL). */
export const asFoliarkError = (e: unknown): FoliarkError =>
  e instanceof FoliarkError ? e : new FoliarkError("INTERNAL", e instanceof Error ? e.message : String(e));

const protocolError = (message: string) => new FoliarkError("PROTOCOL_ERROR", message);

// ---- guards -------------------------------------------------------------------------------------

type Rec = Record<string, unknown>;
const isRec = (x: unknown): x is Rec => typeof x === "object" && x !== null && !Array.isArray(x);
const isStr = (x: unknown): x is string => typeof x === "string";
const isNum = (x: unknown): x is number => typeof x === "number" && Number.isFinite(x);
const isU32 = (x: unknown): x is number => Number.isInteger(x) && (x as number) >= 0 && (x as number) <= 0xffff_ffff;
const opt = (g: (x: unknown) => boolean) => (x: unknown) => x === undefined || g(x);
const arrayOf = (g: (x: unknown) => boolean) => (x: unknown) => Array.isArray(x) && x.every(g);
const tuple = (n: number) => (x: unknown) => Array.isArray(x) && x.length === n && x.every(isNum);
/** Every listed field must satisfy its guard (use `opt` for optional ones). Extra fields are ignored (v1 is additive). */
const shape = (fields: Record<string, (x: unknown) => boolean>) => (x: unknown): boolean =>
  isRec(x) && Object.entries(fields).every(([k, g]) => g(x[k]));
const oneOf = <T extends string>(table: Record<T, unknown>) => (x: unknown): x is T =>
  isStr(x) && Object.hasOwn(table, x);

const PHASES = { open: true, extract: true, render: true, layout: true, export: true, verify: true } as const satisfies Record<Phase, true>;

/** `needsDoc`: whether the op addresses an open document through `Req.doc`. */
export const OPS = {
  init: false, open: false, info: true, pages: true, capabilities: true, extractText: true, hitTest: true, select: true,
  quadsFor: true, search: true, renderPage: true, prepareParagraph: true, applyChanges: true, prepareExport: true,
  export: true, registerFonts: false, close: true,
} as const satisfies Record<OpName, boolean>;

export const isErrorCode = oneOf(ERROR_CODES);
export const isOpName = oneOf(OPS);
const isPhase = oneOf(PHASES);

const isErrorDetails = shape({
  limit: opt(isStr), actual: opt(isNum), max: opt(isNum), feature: opt(isStr), pages: opt(arrayOf(isU32)),
  fields: opt(arrayOf(isStr)), editId: opt(isStr), chars: opt(isStr), neededLines: opt(isU32), maxLines: opt(isU32),
  objRef: opt(isStr),
});
export const isErrorWire = shape({
  code: isErrorCode, message: isStr, details: opt(isErrorDetails), recoverable: (x) => typeof x === "boolean",
}) as (x: unknown) => x is ErrorWire;

const isRect = shape({ x: isNum, y: isNum, width: isNum, height: isNum });
export const isPageGeometry = shape({
  index: isU32, width: isNum, height: isNum, rotation: (x) => x === 0 || x === 90 || x === 180 || x === 270,
  userUnit: (x) => isNum(x) && x > 0, mediaBox: tuple(4), cropBox: tuple(4), userToDisplay: tuple(6), displayToUser: tuple(6),
}) as (x: unknown) => x is PageGeometry;
const isRenderedHeader = shape({
  width: isU32, height: isU32, scale: (x) => isNum(x) && x > 0, clip: isRect, complete: (x) => typeof x === "boolean",
  warnings: arrayOf(shape({ code: isStr, detail: isStr })),
}) as (x: unknown) => x is RenderedPageHeader;

// ---- buffers ------------------------------------------------------------------------------------

const isDetached = (b: ArrayBuffer): boolean => (b as { detached?: boolean }).detached === true;

/**
 * A transferable byte buffer: a real, attached `ArrayBuffer` (never a view, never shared) with
 * `min <= byteLength <= max`. `min: 1` rejects the empty or detached stand-in for a missing buffer.
 */
export function isBuffer(x: unknown, max = DEFAULT_MAX_BUFFER_BYTES, min = 0): x is ArrayBuffer {
  return (
    Object.prototype.toString.call(x) === "[object ArrayBuffer]" &&
    !isDetached(x as ArrayBuffer) &&
    (x as ArrayBuffer).byteLength >= min &&
    (x as ArrayBuffer).byteLength <= max
  );
}

/** Wrapper that makes `postMessage` ownership explicit: listed buffers must be valid, unique, and detached afterwards. */
export function postTransfer(
  port: { postMessage(message: unknown, options: { transfer: Transferable[] }): void },
  message: unknown,
  buffers: ArrayBuffer[],
  maxBytes = DEFAULT_MAX_BUFFER_BYTES,
): void {
  if (new Set(buffers).size !== buffers.length) throw protocolError("buffer listed twice for transfer");
  if (!buffers.every((b) => isBuffer(b, maxBytes))) throw protocolError("transfer list holds a missing, detached, shared or oversized buffer");
  port.postMessage(message, { transfer: buffers });
  // Still attached means the host did not move it: the sender would keep a live alias of bytes the receiver owns.
  if (buffers.some((b) => b.byteLength !== 0 && !isDetached(b))) throw new FoliarkError("INTERNAL", "buffer was not detached by transfer");
}

// ---- per-op payloads (spec 10.4). Deep ChangeSet/ExportReport/ParagraphInfo checks belong to the ops that build them ----

type Guard = (x: unknown) => boolean;
const isNull: Guard = (x) => x === null;
const isBool: Guard = (x) => typeof x === "boolean";
const isEmptyArgs: Guard = isRec;
const isPoint = shape({ x: isNum, y: isNum });
const isQuad: Guard = (x) => Array.isArray(x) && x.length === 4 && x.every(isPoint);
const isRgb = tuple(3);

const isDocumentInfo = shape({
  pageCount: isU32, pdfVersion: isStr, encrypted: isBool, repaired: isBool, requiredAssets: arrayOf(isStr),
  features: shape({
    forms: isBool, xfa: isBool, signatures: isBool, optionalContent: isBool, javascript: isBool, embeddedFiles: isBool,
    annotations: isU32, structTree: isBool, outlines: isBool, thumbnails: isBool, metadata: isBool,
  }),
});
const isWord = shape({
  text: isStr, start: isU32, end: isU32, box: isRect, blockIndex: isU32, lineIndex: isU32, wordIndex: isU32,
  styleIndex: isU32, invisible: isBool, ocgHidden: opt((x) => x === true), source: (x) => x === "native",
});
const isStyleRun = shape({
  fontName: isStr, fontRef: isStr, size: isNum, color: isRgb, renderMode: isU32,
  flags: shape({ bold: isBool, italic: isBool, serif: isBool, mono: isBool }),
});
const isCluster = shape({ start: isU32, end: isU32, quad: isQuad, styleIndex: isU32, glyphs: isU32, flags: isU32 });
const isTextPage = shape({
  docId: isStr, revision: isU32, pageIndex: isU32, text: isStr, words: arrayOf(isWord), styles: arrayOf(isStyleRun),
  clusters: opt(arrayOf(isCluster)),
  stats: shape({ visibleClusters: isU32, invisibleClusters: isU32, unmappedGlyphs: isU32, imageCoverage: isNum }),
});
const isHit = shape({ start: isU32, end: isU32, wordIndex: isU32, quad: isQuad });
const isTextRange = shape({ start: isU32, end: isU32, text: isStr, quads: arrayOf(isQuad), partialCluster: isBool });
const isSearchHit = (x: unknown) => isTextRange(x) && shape({ pageIndex: isU32, revision: isU32 })(x);
const isCapability = shape({
  status: (x) => x === "supported" || x === "unsupported" || x === "unknown",
  reasons: arrayOf(shape({ code: isErrorCode, pages: opt(arrayOf(isU32)) })),
});
const isCapabilities = shape({
  extract: isCapability, render: isCapability, edit: isCapability, redact: isCapability, exportNative: isCapability,
  exportRaster: isCapability,
});
const isRegisteredFont = shape({
  id: isStr, family: isStr, weight: isU32, style: (x) => x === "normal" || x === "italic", postscriptName: isStr, glyphCount: isU32,
});
const isMode = (x: unknown) => x === "native" || x === "raster";

export const argsGuards = (maxBytes = DEFAULT_MAX_BUFFER_BYTES): Partial<Record<OpName, Guard>> => ({
  init: shape({ wasmUrl: isStr, cmapsUrl: opt(isStr), limits: opt(isRec), fontMatch: opt(arrayOf(shape({ pattern: isStr, family: isStr }))) }),
  open: shape({ bytes: (x) => isBuffer(x, maxBytes, 1), password: opt(isStr), allowRepaired: opt(isBool) }),
  info: isEmptyArgs,
  pages: isEmptyArgs,
  capabilities: shape({ deep: opt(isBool) }),
  extractText: shape({ page: isU32, chars: opt(isBool), includeOcgHidden: opt(isBool) }),
  hitTest: shape({ page: isU32, point: isPoint, tolerance: opt((x) => isNum(x) && x >= 0) }),
  select: shape({ page: isU32, from: isPoint, to: isPoint }),
  quadsFor: shape({ page: isU32, start: isU32, end: isU32 }),
  search: shape({
    query: isStr, pages: opt(arrayOf(isU32)), caseSensitive: opt(isBool), diacriticSensitive: opt(isBool), wholeWord: opt(isBool),
    maxHits: opt(isU32),
  }),
  renderPage: shape({
    page: isU32, scale: opt((x) => isNum(x) && x > 0), clip: opt(isRect), annotations: opt(isBool), background: opt(tuple(4)),
  }),
  prepareParagraph: shape({ page: isU32, rects: arrayOf(isRect), font: opt(isStr), editorFace: opt(isBool) }),
  applyChanges: shape({ changes: shape({ baseRevision: isU32 }) }),
  prepareExport: shape({ mode: isMode, policy: opt(isRec) }),
  export: shape({ planId: isStr }),
  registerFonts: shape({
    fonts: arrayOf(shape({
      bytes: (x) => isBuffer(x, maxBytes, 1), family: isStr, weight: (x) => x === 400 || x === 700,
      style: (x) => x === "normal" || x === "italic",
    })),
  }),
  close: isEmptyArgs,
});

export const resultGuards = (maxBytes = DEFAULT_MAX_BUFFER_BYTES): Partial<Record<OpName, Guard>> => ({
  init: isNull,
  open: shape({ docId: isStr, info: isDocumentInfo }),
  info: isDocumentInfo,
  pages: arrayOf(isPageGeometry),
  capabilities: isCapabilities,
  extractText: isTextPage,
  hitTest: (x) => x === null || isHit(x),
  select: (x) => x === null || isTextRange(x),
  quadsFor: isTextRange,
  search: arrayOf(isSearchHit),
  renderPage: (x) =>
    isRenderedHeader(x) &&
    isBuffer((x as Rec).data, maxBytes, 1) &&
    ((x as Rec).data as ArrayBuffer).byteLength === x.width * x.height * 4,
  prepareParagraph: shape({ docId: isStr, revision: isU32, pageIndex: isU32, bounds: isRect, font: isRec }),
  applyChanges: shape({
    accepted: isBool, revision: isU32, rejected: arrayOf(isRec), changedPages: arrayOf(isU32), layouts: arrayOf(isRec),
  }),
  prepareExport: shape({
    planId: isStr, docId: isStr, generation: isU32, revision: isU32, mode: isMode, policyHash: isStr,
    fontRegistryVersion: isU32, byteLength: isU32, report: isRec,
  }),
  export: (x) => isBuffer(x, maxBytes, 1),
  registerFonts: arrayOf(isRegisteredFont),
  close: isNull,
});

// ---- messages -----------------------------------------------------------------------------------

export const isReq = (x: unknown): x is Req<unknown> =>
  isRec(x) && x.v === PROTOCOL_VERSION && isU32(x.id) && isU32(x.gen) && isOpName(x.op) && opt(isStr)(x.doc) && "args" in x &&
  (x.doc !== undefined) === OPS[x.op as OpName];

export const isProg = shape({
  v: (x) => x === PROTOCOL_VERSION, id: isU32,
  progress: shape({ phase: isPhase, done: isU32, total: isU32 }),
}) as (x: unknown) => x is Prog;

export const isCtl = (x: unknown): x is Ctl =>
  isRec(x) && x.v === PROTOCOL_VERSION &&
  (isU32(x.cancel) || ((x.event === "ready" || x.event === "fatal") && isU32(x.gen) && opt(isStr)(x.reason)));

const isResEnvelope = (x: unknown): x is Res<unknown> =>
  isRec(x) && x.v === PROTOCOL_VERSION && isU32(x.id) && ((x.ok === true && "result" in x) || (x.ok === false && isErrorWire(x.error)));

/**
 * Worker side: validate one raw request and check it belongs to this worker generation.
 * Failure carries the id (when readable) so the worker can answer, else the worker must report `fatal`.
 */
export function checkRequest(
  raw: unknown,
  generation: number | undefined,
  maxBytes = DEFAULT_MAX_BUFFER_BYTES,
): { ok: true; req: Req<unknown> } | { ok: false; id?: number; error: FoliarkError } {
  const id = isRec(raw) && isU32(raw.id) ? raw.id : undefined;
  if (!isReq(raw)) return { ok: false, id, error: protocolError("malformed request") };
  if (generation !== undefined && raw.gen !== generation) {
    return { ok: false, id, error: new FoliarkError("WORKER_RESET", "request from a previous worker generation", { recoverable: true }) };
  }
  if (!(argsGuards(maxBytes)[raw.op] ?? (() => true))(raw.args)) return { ok: false, id, error: protocolError(`malformed ${raw.op} arguments`) };
  return { ok: true, req: raw };
}

export const okRes = (id: number, result: unknown): Res<unknown> => ({ v: 1, id, ok: true, result });
export const errRes = (id: number, e: FoliarkError): Res<unknown> => ({ v: 1, id, ok: false, error: e.toWire() });

// ---- request correlation (main-thread side) -------------------------------------------------------

export type DropReason = "malformed" | "unknown-id" | "duplicate" | "stale-generation" | "stale-event";
export type Inbound<T> =
  | { kind: "result"; ctx: T; value: unknown }
  | { kind: "failure"; ctx: T; error: FoliarkError }
  | { kind: "progress"; ctx: T; progress: Prog["progress"] }
  | { kind: "event"; event: "ready" | "fatal"; reason?: string }
  | { kind: "dropped"; reason: DropReason };

/**
 * Request ids are monotonic across worker generations, so a response id classifies by position alone:
 * below this generation's first id = stale, pending = answer, otherwise issued-and-settled = duplicate, else unknown.
 * Every `dropped` outcome is a protocol violation the caller must report (reason only, never content).
 */
export class Correlator<T> {
  generation = 1;
  private last = 0;
  private firstOfGeneration = 1;
  private readonly pending = new Map<number, { op: OpName; ctx: T }>();
  constructor(private readonly maxBytes = DEFAULT_MAX_BUFFER_BYTES) {}

  register(op: OpName, ctx: T): number {
    this.pending.set(++this.last, { op, ctx });
    return this.last;
  }

  /** Start a new generation: returns the contexts still waiting so the caller rejects them `WORKER_RESET`. */
  reset(): T[] {
    const orphans = [...this.pending.values()].map((p) => p.ctx);
    this.pending.clear();
    this.generation++;
    this.firstOfGeneration = this.last + 1;
    return orphans;
  }

  receive(raw: unknown): Inbound<T> {
    if (isCtl(raw)) {
      if ("event" in raw && raw.gen !== this.generation) return { kind: "dropped", reason: "stale-event" };
      return "event" in raw ? { kind: "event", event: raw.event, reason: raw.reason } : { kind: "dropped", reason: "malformed" };
    }
    const id = isRec(raw) && isU32(raw.id) ? raw.id : undefined;
    if (id === undefined) return { kind: "dropped", reason: "malformed" };
    const entry = this.pending.get(id);
    if (!entry) return { kind: "dropped", reason: id < this.firstOfGeneration ? "stale-generation" : id <= this.last ? "duplicate" : "unknown-id" };
    if (isProg(raw)) return { kind: "progress", ctx: entry.ctx, progress: raw.progress };
    // From here the request is answered one way or another; a bad answer still settles it, as PROTOCOL_ERROR.
    this.pending.delete(id);
    if (!isResEnvelope(raw)) return { kind: "failure", ctx: entry.ctx, error: protocolError("malformed response") };
    if (!raw.ok) return { kind: "failure", ctx: entry.ctx, error: FoliarkError.fromWire(raw.error) };
    const guard = resultGuards(this.maxBytes)[entry.op];
    if (guard && !guard(raw.result)) return { kind: "failure", ctx: entry.ctx, error: protocolError(`malformed ${entry.op} result`) };
    return { kind: "result", ctx: entry.ctx, value: raw.result };
  }
}
