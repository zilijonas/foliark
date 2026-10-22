// Edit previews (ADR 0004). A preview is laid out on one revision and one font registry; the engine keeps no
// prepared plan, so nothing can go stale inside it: every `layoutEdit` and `applyChanges` lays out afresh.
import type { DocumentHandle } from "./document.js";
import type { EditPreview } from "./generated/EditPreview.js";

/** True while `p` was laid out on `doc`'s current revision. A font registered since (`fontRegistryVersion`
 *  moves) also changes the layout: call `layoutEdit` again after `registerFonts`. */
export const isCurrentPreview = (p: EditPreview, doc: DocumentHandle): boolean => p.docId === doc.id && p.revision === doc.revision;
