// What the client owns on behalf of its handles (spec 1, 10.2): one registry, so `close()`, a worker reset and
// `dispose()` release the same things the same way. A handle registers a hook that drops its JS-side caches and
// marks it gone; the client releases every hook of a dead worker generation at reset and all of them at dispose.
// This frees JS memory only. Wasm memory never shrinks (a new worker returns it) and nothing here is a wipe.
export class Lifecycle {
  private readonly owned = new Set<() => void>();

  /** Own a resource until it is released. Returns the early release (`close()`): it runs the hook once. */
  own(release: () => void): () => void {
    this.owned.add(release);
    return () => {
      if (this.owned.delete(release)) release();
    };
  }

  /** Release everything owned; a hook that throws does not stop the others. */
  releaseAll(): void {
    const all = [...this.owned];
    this.owned.clear();
    for (const release of all) {
      try {
        release();
      } catch {
        // releasing is best effort per resource; the rest must still go
      }
    }
  }
}
