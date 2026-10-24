// Public document API (spec 1): `DocumentHandle` and the option types of its calls. A handle is a thin,
// validated view over one document inside the worker; `Link` is how it reaches the client.
import type { Capabilities } from "./generated/Capabilities.js";
import type { ChangeResult } from "./generated/ChangeResult.js";
import type { ChangeSet } from "./generated/ChangeSet.js";
import type { DocumentInfo } from "./generated/DocumentInfo.js";
import type { ExportMode } from "./generated/ExportMode.js";
import type { ExportPlan } from "./generated/ExportPlan.js";
import type { ExportPolicy } from "./generated/ExportPolicy.js";
import type { EditPreview } from "./generated/EditPreview.js";
import type { ExtractOptions } from "./generated/ExtractOptions.js";
import type { Hit } from "./generated/Hit.js";
import type { OpName } from "./generated/OpName.js";
import type { PageGeometry } from "./generated/PageGeometry.js";
import type { ParagraphInfo } from "./generated/ParagraphInfo.js";
import type { Point } from "./generated/Point.js";
import type { Rect } from "./generated/Rect.js";
import type { RenderWarning } from "./generated/RenderWarning.js";
import type { SearchHit } from "./generated/SearchHit.js";
import type { SearchOptions as SearchWire } from "./generated/SearchOptions.js";
import type { TextPage } from "./generated/TextPage.js";
import type { TextRange } from "./generated/TextRange.js";
import { FoliarkError } from "./protocol.js";

export interface Signal {
  signal?: AbortSignal;
}
export type ExtractTextOptions = ExtractOptions & Signal;
export type SearchOptions = SearchWire & Signal;
export interface RenderOptions extends Signal {
  /** Pixels per point (1). */
  scale?: number;
  /** Display-space rect to render (the whole page). */
  clip?: Rect;
  /** Draw annotation appearances (true). */
  annotations?: boolean;
  /** Straight-alpha sRGB, each 0..1 (opaque white). */
  background?: [number, number, number, number];
}
export interface RenderedPage {
  width: number;
  height: number;
  /** RGBA8 straight alpha sRGB, ImageData-compatible. */
  data: Uint8ClampedArray;
  scale: number;
  clip: Rect;
  /** False iff `warnings` is not empty. */
  complete: boolean;
  warnings: RenderWarning[];
}
export interface ExportOptions extends Signal {
  mode: ExportMode;
  policy?: Partial<ExportPolicy>;
}

export interface DocumentHandle {
  readonly id: string;
  readonly generation: number;
  /** Starts at 0; +1 per accepted `applyChanges`. */
  readonly revision: number;
  info(): Promise<DocumentInfo>;
  pages(): Promise<PageGeometry[]>;
  capabilities(o?: { deep?: boolean } & Signal): Promise<Capabilities>;
  extractText(page: number, o?: ExtractTextOptions): Promise<TextPage>;
  hitTest(page: number, p: Point, o?: { tolerance?: number }): Promise<Hit | null>;
  select(page: number, from: Point, to: Point): Promise<TextRange | null>;
  quadsFor(page: number, start: number, end: number): Promise<TextRange>;
  search(query: string, o?: SearchOptions): Promise<SearchHit[]>;
  renderPage(page: number, o?: RenderOptions): Promise<RenderedPage>;
  prepareParagraph(page: number, rects: Rect[], o?: { font?: string; editorFace?: boolean }): Promise<ParagraphInfo>;
  /**
   * ADR 0004: the layout `applyChanges` would commit for one edit, nothing committed. Cheap enough for every
   * keystroke. Rejects as `applyChanges` would (EDIT_*), except that overflow is `fits: false`.
   */
  layoutEdit(page: number, rects: Rect[], text: string, o?: { font?: string; glyphs?: boolean }): Promise<EditPreview>;
  applyChanges(cs: ChangeSet, o?: Signal): Promise<ChangeResult>;
  prepareExport(o: ExportOptions): Promise<ExportPlan>;
  /** Single-use byte handoff: the plan is freed. */
  export(plan: ExportPlan): Promise<Uint8Array>;
  /** Idempotent; frees the document, its plans and caches. */
  close(): Promise<void>;
}

/** What a handle needs from the client. */
export interface Link {
  readonly generation: number;
  readonly disposed: boolean;
  /** Register what the handle holds so reset and dispose release it (see lifecycle.ts); returns the early release. */
  own(release: () => void): () => void;
  call<R>(op: OpName, args: object, o?: { doc?: string; transfer?: ArrayBuffer[]; signal?: AbortSignal }): Promise<R>;
}

const invalid = (message: string) => new FoliarkError("INVALID_ARGUMENT", message);
const isIndex = (n: unknown): n is number => Number.isInteger(n) && (n as number) >= 0 && (n as number) <= 0xffff_ffff;
const isPoint = (p: Point | undefined) => !!p && Number.isFinite(p.x) && Number.isFinite(p.y);

export function makeHandle(link: Link, id: string, info: DocumentInfo): DocumentHandle {
  const generation = link.generation;
  let revision = 0;
  let closed = false;
  let pages: PageGeometry[] | undefined;
  /** Gone: closed, or its worker was replaced or disposed. Drops what the handle caches. */
  const release = link.own(() => {
    closed = true;
    pages = undefined;
  });

  /** One validated call. A closed document, or one from before a reset, is gone. */
  const call = async <R>(op: OpName, args: object, signal?: AbortSignal, transfer?: ArrayBuffer[]): Promise<R> => {
    try {
      if (link.disposed) throw new FoliarkError("CLIENT_DISPOSED", "client disposed");
      if (closed || link.generation !== generation) throw new FoliarkError("DOCUMENT_GONE", "document is closed or from a previous worker");
      return await link.call<R>(op, args, { doc: id, signal, transfer });
    } catch (e) {
      if (e instanceof FoliarkError) {
        e.docId = id;
        e.revision = revision;
      }
      throw e;
    }
  };
  const page = (n: number) => {
    if (!isIndex(n)) throw invalid("page must be a non-negative integer");
    return n;
  };

  return {
    id,
    generation,
    get revision() {
      return revision;
    },
    info: async () => info,
    async pages() {
      return (pages ??= await call<PageGeometry[]>("pages", {}));
    },
    capabilities: (o) => call("capabilities", { deep: o?.deep }, o?.signal),
    async extractText(p, o) {
      return call("extractText", { page: page(p), chars: o?.chars, includeOcgHidden: o?.includeOcgHidden }, o?.signal);
    },
    async hitTest(p, point, o) {
      if (!isPoint(point) || (o?.tolerance !== undefined && !(o.tolerance >= 0))) throw invalid("point must be finite and tolerance >= 0");
      return call("hitTest", { page: page(p), point, tolerance: o?.tolerance });
    },
    async select(p, from, to) {
      if (!isPoint(from) || !isPoint(to)) throw invalid("points must be finite");
      return call("select", { page: page(p), from, to });
    },
    async quadsFor(p, start, end) {
      if (!isIndex(start) || !isIndex(end) || start > end) throw invalid("start and end must be offsets with start <= end");
      return call("quadsFor", { page: page(p), start, end });
    },
    async search(query, o) {
      if (typeof query !== "string") throw invalid("query must be a string");
      const { signal, ...options } = o ?? {};
      return call("search", { query, ...options }, signal);
    },
    async renderPage(p, o) {
      const { signal, ...options } = o ?? {};
      const r = await call<Omit<RenderedPage, "data"> & { data: ArrayBuffer }>("renderPage", { page: page(p), ...options }, signal);
      return { ...r, data: new Uint8ClampedArray(r.data) };
    },
    async prepareParagraph(p, rects, o) {
      const r = await call<ParagraphInfo>("prepareParagraph", { page: page(p), rects, font: o?.font, editorFace: o?.editorFace });
      // The faces cross the worker boundary as transferred ArrayBuffers.
      for (const x of [r, ...r.segments]) if (x.editorFace) x.editorFace.bytes = new Uint8Array(x.editorFace.bytes as unknown as ArrayBuffer);
      return r;
    },
    async layoutEdit(p, rects, text, o) {
      if (typeof text !== "string") throw invalid("text must be a string");
      return call("layoutEdit", { page: page(p), rects, text, font: o?.font, glyphs: o?.glyphs });
    },
    async applyChanges(changes, o) {
      const r = await call<ChangeResult>("applyChanges", { changes }, o?.signal);
      if (r.accepted) revision = r.revision;
      return r;
    },
    prepareExport: (o) => call("prepareExport", { mode: o.mode, policy: o.policy }, o.signal),
    async export(plan) {
      return new Uint8Array(await call<ArrayBuffer>("export", { planId: plan.planId }));
    },
    async close() {
      if (closed) return;
      if (link.disposed || link.generation !== generation) return release(); // its worker is gone, nothing left to free
      await call<null>("close", {});
      release();
    },
  };
}
