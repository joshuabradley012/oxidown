export type {
  Splice,
  EditOrigin,
  Decoration,
  DecorationMark,
  DecorationConceal,
  DecorationLine,
  DecorationWidget,
  SelectionRange,
  OxidownCore,
  CoreChange,
  RangeCommandName,
} from "./protocol.js";

// BREAKING (pre-1.0): MockCore, MockCoreOptions, diffSplices, and parseDoc
// are gone — the hand-written TypeScript reference core is retired (the wasm
// core is the contract's only implementation; see docs/boundary-v0.md's
// testing-strategy note). `applySplices` survives as a plain helper, now
// exported from splices.ts.
export { applySplices } from "./splices.js";

export {
  oxidown,
  oxidownCommands,
  applyCoreChange,
  changesToSplices,
  endOfLastSplice,
  sanitizeSurrogates,
} from "./extension.js";
export type { OxidownOptions } from "./extension.js";

export { oxidownTheme } from "./theme.js";

export { loadWasmCore } from "./wasm-core.js";
