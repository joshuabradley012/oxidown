export type {
  Splice,
  EditOrigin,
  Decoration,
  DecorationMark,
  DecorationConceal,
  DecorationLine,
  SelectionRange,
  OxidownCore,
} from "./protocol.js";

export { MockCore, applySplices, diffSplices, parseDoc } from "./mock-core.js";
export type { MockCoreOptions } from "./mock-core.js";

export { oxidown, changesToSplices, endOfLastSplice } from "./extension.js";
export type { OxidownOptions } from "./extension.js";

export { oxidownTheme } from "./theme.js";

export { loadWasmCore } from "./wasm-core.js";
