/**
 * View-side syntax highlighting for fenced code blocks.
 *
 * Architecture note (plan.md §5.7 / xi lesson): highlighting is DISPOSABLE
 * per-view derived state — it never touches the core or the protocol. The
 * core tells us which lines are `code-fence`/`code-block`; the fence line's
 * own source text carries the language ("```ts"); we parse each visible
 * region with the matching Lezer parser (lazily loaded via
 * @codemirror/language-data) and emit ordinary CM6 mark decorations with
 * `tok-*` classes (@lezer/highlight's classHighlighter). Colors live in
 * theme.ts.
 *
 * Languages load asynchronously the first time they appear; `onLoad` lets
 * the caller schedule a decoration rebuild when one arrives. Unknown
 * languages are remembered as misses and skipped thereafter; a FAILED load
 * of a known language is retried on the next rebuild instead (a transient
 * network error must not disable the language page-wide until reload).
 */

import type { EditorState } from "@codemirror/state";
import type { Range } from "@codemirror/state";
import { Decoration } from "@codemirror/view";
import { LanguageDescription, type LanguageSupport } from "@codemirror/language";
import { languages as languageRegistry } from "@codemirror/language-data";
import { classHighlighter, highlightTree } from "@lezer/highlight";
import { Tree, TreeFragment } from "@lezer/common";
import type { Decoration as CoreDecoration } from "./protocol.js";

export interface FenceRegion {
  /** Body range (first body line start .. last body line end), current doc. */
  from: number;
  to: number;
  /** Info-string language name, e.g. "ts" — never empty. */
  lang: string;
}

const FENCE_INFO_RE = /^\s{0,3}(?:`{3,}|~{3,})\s*([^\s`]*)/;

/**
 * Assemble fenced regions from the core's line decorations: an opening
 * `code-fence` (with a language in its source line) starts a region; the
 * following `code-block` lines extend it; the closing fence ends it.
 * Viewport-scoped by construction (the core only emits for the viewport).
 */
export function collectFenceRegions(
  decos: readonly CoreDecoration[],
  state: EditorState,
): FenceRegion[] {
  const lineDecos = decos
    .filter(
      (d): d is Extract<CoreDecoration, { kind: "line" }> =>
        d.kind === "line" && (d.style === "code-fence" || d.style === "code-block"),
    )
    .sort((a, b) => a.at - b.at);

  const regions: FenceRegion[] = [];
  let current: FenceRegion | null = null;
  for (const d of lineDecos) {
    const line = state.doc.lineAt(Math.min(d.at, state.doc.length));
    if (d.style === "code-fence") {
      if (current) {
        // Closing fence.
        if (current.to > current.from) regions.push(current);
        current = null;
      } else {
        const lang = FENCE_INFO_RE.exec(line.text)?.[1] ?? "";
        const bodyStart = Math.min(line.to + 1, state.doc.length);
        current = lang ? { lang, from: bodyStart, to: bodyStart } : null;
      }
    } else if (current) {
      current.to = Math.max(current.to, line.to);
    }
  }
  // Unterminated fence (e.g. mid-stream): highlight what's there.
  if (current && current.to > current.from) regions.push(current);
  return regions;
}

// -- language loading (lazy, cached, misses remembered) ----------------------
// Deliberately MODULE-GLOBAL (unlike the parse caches below): a language only
// needs to load once per page, however many editor instances want it.

/** lang (lowercased) -> loaded support, or null for a known miss. */
const supports = new Map<string, LanguageSupport | null>();
/**
 * In-flight loads: key -> EVERY requester's onLoad. All of them are notified
 * on resolution — a second editor (or plugin instance) requesting a language
 * while another's load is still in flight must get repainted too, not just
 * the instance that initiated the load.
 */
const loading = new Map<string, Set<() => void>>();

function supportFor(lang: string, onLoad: () => void): LanguageSupport | null | undefined {
  const key = lang.toLowerCase();
  if (supports.has(key)) return supports.get(key);
  const pending = loading.get(key);
  if (pending) {
    pending.add(onLoad);
    return undefined; // still loading; notified with the initiator
  }
  const desc = LanguageDescription.matchLanguageName(languageRegistry, key, true);
  if (!desc) {
    supports.set(key, null);
    return null;
  }
  const callbacks = new Set<() => void>([onLoad]);
  loading.set(key, callbacks);
  desc.load().then(
    (s) => {
      supports.set(key, s);
      loading.delete(key);
      for (const cb of callbacks) cb();
    },
    () => {
      // A FAILED load (network hiccup, CDN outage) is NOT a permanent miss —
      // only an unknown language is (the `!desc` branch above). Leave
      // `supports` unset so the next decoration rebuild retries the load.
      // There is nothing to repaint right now, so the pending callbacks are
      // dropped, not invoked — which is also what prevents a retry loop:
      // a failure schedules nothing itself, so the retry only happens when
      // an ordinary trigger (edit/selection/viewport change) rebuilds
      // decorations anyway.
      loading.delete(key);
    },
  );
  return undefined; // still loading
}

// -- highlighting (parse cache + mark cache) ---------------------------------

interface CachedSpan {
  from: number;
  to: number;
  cls: string;
}

const PARSE_CACHE_MAX = 64;

interface TreeCacheEntry {
  text: string;
  tree: Tree;
}
const TREE_CACHE_MAX = 64;

// markCache stays module-global: Decoration.mark values are immutable and
// keyed purely by class name, so sharing them across instances is safe (and
// lets CM6 see identical decorations across editors).
const markCache = new Map<string, Decoration>();
function markFor(cls: string): Decoration {
  let m = markCache.get(cls);
  if (!m) {
    m = Decoration.mark({ class: cls });
    markCache.set(cls, m);
  }
  return m;
}

/** Common prefix/suffix lengths between two strings (never overlapping). */
function commonPrefixSuffix(a: string, b: string): { prefix: number; suffix: number } {
  const maxLen = Math.min(a.length, b.length);
  let prefix = 0;
  while (prefix < maxLen && a.charCodeAt(prefix) === b.charCodeAt(prefix)) prefix++;
  let suffix = 0;
  const maxSuffix = maxLen - prefix;
  while (
    suffix < maxSuffix &&
    a.charCodeAt(a.length - 1 - suffix) === b.charCodeAt(b.length - 1 - suffix)
  ) {
    suffix++;
  }
  return { prefix, suffix };
}

/**
 * Per-view-plugin-instance highlighter: the parse/tree caches live here (one
 * FenceHighlighter per plugin instance, created with it and released with
 * it) rather than at module scope — module-global caches would outlive every
 * editor AND collide across concurrent instances, whose `${regionIndex}:
 * ${lang}` keys would steal each other's slots (correct output, but the
 * incremental reuse constantly thrashes). The language-load registry above
 * stays global on purpose (a language loads once per page).
 */
export class FenceHighlighter {
  /** `${lang} ${text}` -> spans relative to the region text (exact full-text hits). */
  private readonly parseCache = new Map<string, CachedSpan[]>();

  /**
   * Incremental-parse support: the last Tree (+ the text it was parsed from)
   * per fence, so a keystroke inside a large fence reuses Lezer's own
   * incremental machinery (TreeFragment.applyChanges) instead of a
   * from-scratch parse of the whole fence body on every call. Keyed by
   * `${index in this call's region list}:${lang}` — a cheap, not-perfectly-
   * stable "which fence is this" identity (it can point at the wrong prior
   * entry when fences are added/removed/reordered above it in the same edit).
   * That's fine for CORRECTNESS: the diff below (common prefix/suffix between
   * the cached old text and the current text) is a mathematically valid
   * description of an unchanged-prefix/unchanged-suffix edit for WHATEVER two
   * strings are compared — a wrong-identity mismatch just yields a smaller
   * reusable region (more re-parsing), never a wrong tree. Only efficiency is
   * at stake, matching this module's existing "bounded cache, cheap eviction"
   * discipline.
   */
  private readonly treeCache = new Map<string, TreeCacheEntry>();

  /**
   * Parse `text` with `support`'s Lezer parser, reusing the previous parse for
   * this `cacheKey` (via TreeFragment.applyChanges) when one exists — unchanged
   * parts of the fence (everything outside the common prefix/suffix diff
   * against the previously cached text) are reused rather than re-tokenized.
   * Falls back to a full from-scratch parse when there's no previous tree for
   * this key, or the computed change range is degenerate.
   */
  private parseIncremental(support: LanguageSupport, text: string, cacheKey: string): Tree {
    const parser = support.language.parser;
    const prev = this.treeCache.get(cacheKey);
    let tree: Tree;
    if (!prev) {
      tree = parser.parse(text);
    } else if (prev.text === text) {
      tree = prev.tree;
    } else {
      const { prefix, suffix } = commonPrefixSuffix(prev.text, text);
      const fromA = prefix;
      const toA = prev.text.length - suffix;
      const fromB = prefix;
      const toB = text.length - suffix;
      // Guard against a degenerate/overlapping range (shouldn't happen given
      // prefix+suffix <= min(len), but never feed Lezer a malformed change).
      if (fromA <= toA && fromB <= toB) {
        const fragments = TreeFragment.applyChanges(TreeFragment.addTree(prev.tree), [
          { fromA, toA, fromB, toB },
        ]);
        tree = parser.parse(text, fragments);
      } else {
        tree = parser.parse(text);
      }
    }
    if (!this.treeCache.has(cacheKey) && this.treeCache.size >= TREE_CACHE_MAX) {
      // Drop the oldest entry (Map preserves insertion order).
      const oldest = this.treeCache.keys().next().value;
      if (oldest !== undefined) this.treeCache.delete(oldest);
    }
    this.treeCache.set(cacheKey, { text, tree });
    return tree;
  }

  private highlightText(
    support: LanguageSupport,
    lang: string,
    text: string,
    cacheKey: string,
  ): CachedSpan[] {
    const key = `${lang} ${text}`;
    const hit = this.parseCache.get(key);
    if (hit) return hit;
    const spans: CachedSpan[] = [];
    const tree = this.parseIncremental(support, text, cacheKey);
    highlightTree(tree, classHighlighter, (from, to, cls) => {
      spans.push({ from, to, cls });
    });
    if (this.parseCache.size >= PARSE_CACHE_MAX) {
      // Drop the oldest entry (Map preserves insertion order).
      const oldest = this.parseCache.keys().next().value;
      if (oldest !== undefined) this.parseCache.delete(oldest);
    }
    this.parseCache.set(key, spans);
    return spans;
  }

  /**
   * Produce highlight mark ranges for every region whose language is already
   * loaded, and kick off loads (reporting via `onLoad`) for those that aren't.
   */
  highlightRegions(
    state: EditorState,
    regions: readonly FenceRegion[],
    onLoad: () => void,
  ): Range<Decoration>[] {
    const out: Range<Decoration>[] = [];
    for (let i = 0; i < regions.length; i++) {
      const region = regions[i];
      const support = supportFor(region.lang, onLoad);
      if (!support) continue; // miss or still loading
      const text = state.doc.sliceString(region.from, region.to);
      // See treeCache's doc comment: this key need not be a perfectly stable
      // fence identity for correctness, only for how much gets reused.
      const cacheKey = `${i}:${region.lang}`;
      for (const span of this.highlightText(support, region.lang, text, cacheKey)) {
        const from = region.from + span.from;
        const to = region.from + span.to;
        if (to > from && to <= region.to) out.push(markFor(span.cls).range(from, to));
      }
    }
    return out;
  }
}
