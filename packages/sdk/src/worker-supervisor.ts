// Hang supervision and reset classification (spec 10.2). The worker cannot report a hang itself: a
// synchronous wasm call blocks its event loop, so only the main thread can notice that it went silent.
import type { ResetReason } from "./client.js";

/** Spec 10.2 default `hangTimeoutMs`. */
export const DEFAULT_HANG_TIMEOUT_MS = 10_000;

/**
 * Fires `onHang` when it is armed and nothing calls `kick` for `ms`. The client arms it while an op is in
 * flight and kicks it on every message from the worker (results and progress both count), so a long op that
 * keeps reporting is alive and one stuck in a single call is terminated.
 */
export class Watchdog {
  private timer: ReturnType<typeof setTimeout> | undefined;
  constructor(private readonly ms: number, private readonly onHang: () => void) {}

  /** Start counting unless already counting: queueing more requests behind a hung op must not extend its deadline. */
  arm(): void {
    this.timer ??= setTimeout(this.fire, this.ms);
  }

  /** The worker spoke: count from zero. */
  kick(): void {
    this.stop();
    this.arm();
  }

  stop(): void {
    clearTimeout(this.timer);
    this.timer = undefined;
  }

  private fire = () => {
    this.timer = undefined;
    this.onHang();
  };
}

/** A JS-side allocation failure (typed array, wasm memory growth); a wasm-side one traps and is a crash. */
const OOM = /out of memory|could not allocate|allocation (failed|size overflow)|memory allocation|maximum memory size|memory\.grow|invalid (typed )?array length|array buffer allocation/i;

export function isOutOfMemory(e: unknown): boolean {
  const text = e instanceof Error ? e.message : typeof e === "string" ? e : "";
  return OOM.test(text) || (e instanceof RangeError && /memory|alloc/i.test(text));
}

/** Why a worker that failed on its own (not hung, not restarted) must be replaced. */
export const failureReason = (e: unknown): Exclude<ResetReason, "hang" | "restart"> => (isOutOfMemory(e) ? "oom" : "crash");
