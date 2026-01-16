# MDEditor: Cross-Platform Markdown Editor

**A Rust-core WYSIWYG markdown editor with native performance across all platforms.**

---

## Vision

Create the definitive markdown editing experience—fast, beautiful, and consistent across web, iOS, Android, and desktop. Built on a shared Rust core that handles document modeling, parsing, and operations, with thin platform-specific rendering layers.

### Core Principles

1. **Markdown-first** — Not a rich text editor that exports markdown. Markdown IS the data model.
2. **Ruthlessly scoped** — Standard markdown only for v1. No MDX, no extensions, no plugins initially.
3. **Native feel** — Each platform should feel native, not a compromised cross-platform wrapper.
4. **Performance** — Sub-millisecond operations, instant rendering, handles large documents gracefully.
5. **Extensibility by design** — Architecture supports future extensions without rewrites.

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                    mdeditor-core (Rust)                     │
│                                                             │
│  ┌───────────────┐  ┌───────────────┐  ┌─────────────────┐ │
│  │   Document    │  │   Markdown    │  │    Command      │ │
│  │    Model      │  │    Parser     │  │    System       │ │
│  │   (Rope)      │  │  (pulldown)   │  │                 │ │
│  └───────────────┘  └───────────────┘  └─────────────────┘ │
│  ┌───────────────┐  ┌───────────────┐  ┌─────────────────┐ │
│  │   Selection   │  │   History     │  │   Serializer    │ │
│  │    Model      │  │  (Undo/Redo)  │  │  (AST ↔ Text)   │ │
│  └───────────────┘  └───────────────┘  └─────────────────┘ │
└─────────────────────────────────────────────────────────────┘
                            │
           ┌────────────────┼────────────────┐
           ▼                ▼                ▼
    ┌─────────────┐  ┌─────────────┐  ┌─────────────┐
    │    WASM     │  │   uniffi    │  │     JNI     │
    │  (wasm-     │  │  (Swift/    │  │  (Kotlin)   │
    │  bindgen)   │  │   ObjC)     │  │             │
    └─────────────┘  └─────────────┘  └─────────────┘
           │                │                │
           ▼                ▼                ▼
    ┌─────────────┐  ┌─────────────┐  ┌─────────────┐
    │  Web View   │  │  iOS/macOS  │  │   Android   │
    │  (React)    │  │    View     │  │    View     │
    └─────────────┘  └─────────────┘  └─────────────┘
```

---

## Part 1: Rust Core (`mdeditor-core`)

### 1.1 Document Model

The document is represented as a tree of nodes, backed by a rope data structure for efficient text manipulation.

```rust
/// Core document types
pub mod document {
    use std::ops::Range;

    /// Unique identifier for nodes
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct NodeId(pub u64);

    /// Position in the document (byte offset)
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    pub struct Position(pub usize);

    /// A span in the document
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Span {
        pub start: Position,
        pub end: Position,
    }

    /// Block-level node types
    #[derive(Clone, Debug, PartialEq)]
    pub enum Block {
        Paragraph(Vec<Inline>),
        Heading { level: u8, content: Vec<Inline> },
        CodeBlock { language: Option<String>, content: String },
        BlockQuote(Vec<Block>),
        List { ordered: bool, start: Option<u32>, items: Vec<ListItem> },
        ThematicBreak,
    }

    /// Inline node types
    #[derive(Clone, Debug, PartialEq)]
    pub enum Inline {
        Text(String),
        Emphasis(Vec<Inline>),
        Strong(Vec<Inline>),
        Code(String),
        Link { url: String, title: Option<String>, content: Vec<Inline> },
        Image { url: String, alt: String, title: Option<String> },
        SoftBreak,
        HardBreak,
    }

    /// List item
    #[derive(Clone, Debug, PartialEq)]
    pub struct ListItem {
        pub checked: Option<bool>, // For task lists (future)
        pub content: Vec<Block>,
    }

    /// The root document
    #[derive(Clone, Debug, Default)]
    pub struct Document {
        pub blocks: Vec<Block>,
    }
}
```

### 1.2 Selection Model

Platform-agnostic selection representation.

```rust
pub mod selection {
    use super::document::Position;

    /// A single cursor or selection range
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Selection {
        /// Where the selection started (anchor)
        pub anchor: Position,
        /// Where the selection ends (focus/cursor)
        pub focus: Position,
    }

    impl Selection {
        pub fn cursor(pos: Position) -> Self {
            Self { anchor: pos, focus: pos }
        }

        pub fn is_collapsed(&self) -> bool {
            self.anchor == self.focus
        }

        pub fn is_forward(&self) -> bool {
            self.anchor <= self.focus
        }

        pub fn start(&self) -> Position {
            if self.is_forward() { self.anchor } else { self.focus }
        }

        pub fn end(&self) -> Position {
            if self.is_forward() { self.focus } else { self.anchor }
        }
    }

    /// Editor state includes selection
    #[derive(Clone, Debug)]
    pub struct EditorState {
        pub selection: Selection,
        // Future: multiple selections for multi-cursor
    }
}
```

### 1.3 Command System

All mutations go through commands. This enables undo/redo, collaborative editing, and debugging.

```rust
pub mod commands {
    use super::document::{Document, Position, Span, Block, Inline};
    use super::selection::Selection;

    /// All possible editor commands
    #[derive(Clone, Debug)]
    pub enum Command {
        // Text manipulation
        InsertText { at: Position, text: String },
        DeleteRange { span: Span },
        ReplaceRange { span: Span, text: String },

        // Formatting (toggles)
        ToggleBold { span: Span },
        ToggleItalic { span: Span },
        ToggleCode { span: Span },
        
        // Block operations
        SetHeading { at: Position, level: u8 }, // 0 = paragraph
        ToggleBlockQuote { at: Position },
        ToggleList { at: Position, ordered: bool },
        InsertCodeBlock { at: Position, language: Option<String> },
        InsertThematicBreak { at: Position },

        // Links & Images
        InsertLink { span: Span, url: String, title: Option<String> },
        RemoveLink { span: Span },

        // Selection
        SetSelection(Selection),

        // Compound (batched for undo)
        Batch(Vec<Command>),
    }

    /// Result of applying a command
    #[derive(Clone, Debug)]
    pub struct CommandResult {
        /// The inverse command (for undo)
        pub inverse: Command,
        /// New selection after command
        pub new_selection: Option<Selection>,
    }
}
```

### 1.4 Editor Core

The main API surface.

```rust
pub mod editor {
    use super::*;

    /// The core editor instance
    pub struct Editor {
        document: Document,
        state: EditorState,
        history: History,
    }

    impl Editor {
        /// Create a new editor with optional initial content
        pub fn new() -> Self;
        pub fn from_markdown(source: &str) -> Result<Self, ParseError>;

        // Document access
        pub fn document(&self) -> &Document;
        pub fn to_markdown(&self) -> String;

        // State access  
        pub fn selection(&self) -> Selection;

        // Command execution
        pub fn execute(&mut self, cmd: Command) -> Result<(), EditorError>;

        // History
        pub fn can_undo(&self) -> bool;
        pub fn can_redo(&self) -> bool;
        pub fn undo(&mut self) -> Result<(), EditorError>;
        pub fn redo(&mut self) -> Result<(), EditorError>;

        // Queries (for rendering)
        pub fn formatting_at(&self, pos: Position) -> FormattingState;
        pub fn block_at(&self, pos: Position) -> Option<&Block>;
    }

    /// Current formatting state (for toolbar)
    #[derive(Clone, Debug, Default)]
    pub struct FormattingState {
        pub bold: bool,
        pub italic: bool,
        pub code: bool,
        pub link: Option<String>,
        pub heading_level: u8, // 0 = not a heading
        pub in_blockquote: bool,
        pub in_list: Option<bool>, // Some(true) = ordered
        pub in_code_block: bool,
    }
}
```

### 1.5 History (Undo/Redo)

```rust
pub mod history {
    use super::commands::Command;

    /// Undo/redo history
    pub struct History {
        undo_stack: Vec<HistoryEntry>,
        redo_stack: Vec<HistoryEntry>,
        max_size: usize,
    }

    struct HistoryEntry {
        command: Command,
        inverse: Command,
        timestamp: u64,
    }

    impl History {
        pub fn new(max_size: usize) -> Self;
        pub fn push(&mut self, command: Command, inverse: Command);
        pub fn undo(&mut self) -> Option<Command>;
        pub fn redo(&mut self) -> Option<Command>;
        pub fn clear(&mut self);
    }
}
```

---

## Part 2: Platform Bindings

### 2.1 WASM Bindings (Web)

Using `wasm-bindgen` for JavaScript interop.

```rust
// mdeditor-wasm/src/lib.rs
use wasm_bindgen::prelude::*;
use mdeditor_core::*;

#[wasm_bindgen]
pub struct WasmEditor {
    inner: Editor,
}

#[wasm_bindgen]
impl WasmEditor {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self;

    #[wasm_bindgen]
    pub fn from_markdown(source: &str) -> Result<WasmEditor, JsError>;

    #[wasm_bindgen]
    pub fn to_markdown(&self) -> String;

    #[wasm_bindgen]
    pub fn to_html(&self) -> String; // For rendering

    // Commands exposed as methods
    #[wasm_bindgen]
    pub fn insert_text(&mut self, position: usize, text: &str);
    
    #[wasm_bindgen]
    pub fn delete_range(&mut self, start: usize, end: usize);

    #[wasm_bindgen]
    pub fn toggle_bold(&mut self);
    
    #[wasm_bindgen]
    pub fn toggle_italic(&mut self);

    // ... etc

    #[wasm_bindgen]
    pub fn undo(&mut self) -> bool;
    
    #[wasm_bindgen]
    pub fn redo(&mut self) -> bool;

    // Selection
    #[wasm_bindgen]
    pub fn set_selection(&mut self, anchor: usize, focus: usize);

    #[wasm_bindgen]
    pub fn get_formatting_state(&self) -> JsValue; // Returns FormattingState as JSON
}
```

### 2.2 Swift Bindings (iOS/macOS)

Using Mozilla's `uniffi` for Swift generation.

```rust
// mdeditor-swift/src/lib.rs
uniffi::include_scaffolding!("mdeditor");

// uniffi.toml defines the interface
// Generates Swift classes automatically
```

```udl
// mdeditor.udl
namespace mdeditor {
    // Factory functions
};

interface Editor {
    constructor();
    [Name=from_markdown]
    constructor(string source);
    
    string to_markdown();
    string to_html();
    
    void insert_text(u64 position, string text);
    void delete_range(u64 start, u64 end);
    void toggle_bold();
    void toggle_italic();
    
    boolean undo();
    boolean redo();
    
    void set_selection(u64 anchor, u64 focus);
    FormattingState get_formatting_state();
};

dictionary FormattingState {
    boolean bold;
    boolean italic;
    boolean code;
    string? link;
    u8 heading_level;
    boolean in_blockquote;
    boolean? in_list;
    boolean in_code_block;
};
```

### 2.3 Kotlin Bindings (Android)

Also via `uniffi` with Kotlin output.

```kotlin
// Generated by uniffi
class Editor {
    constructor()
    constructor(source: String)
    
    fun toMarkdown(): String
    fun toHtml(): String
    
    fun insertText(position: ULong, text: String)
    fun deleteRange(start: ULong, end: ULong)
    fun toggleBold()
    fun toggleItalic()
    
    fun undo(): Boolean
    fun redo(): Boolean
    
    fun setSelection(anchor: ULong, focus: ULong)
    fun getFormattingState(): FormattingState
}
```

---

## Part 3: Platform Views

### 3.1 Web (React Component)

Phase 1 uses DOM-based rendering (not Canvas). Simpler, accessible, good enough for v1.

```typescript
// packages/mdeditor-react/src/Editor.tsx
import { useEffect, useRef, useState, useCallback } from 'react';
import type { WasmEditor } from 'mdeditor-wasm';

interface MDEditorProps {
  initialValue?: string;
  onChange?: (markdown: string) => void;
  placeholder?: string;
  className?: string;
}

export function MDEditor({ 
  initialValue = '', 
  onChange,
  placeholder,
  className 
}: MDEditorProps) {
  const editorRef = useRef<WasmEditor | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const [html, setHtml] = useState('');
  const [formatting, setFormatting] = useState<FormattingState>(defaultFormatting);

  // Initialize WASM editor
  useEffect(() => {
    initWasm().then(() => {
      editorRef.current = initialValue 
        ? WasmEditor.from_markdown(initialValue)
        : new WasmEditor();
      setHtml(editorRef.current.to_html());
    });
  }, []);

  // Handle input
  const handleInput = useCallback((e: InputEvent) => {
    const editor = editorRef.current;
    if (!editor) return;

    // Map DOM input to editor commands
    // ... input handling logic
    
    setHtml(editor.to_html());
    setFormatting(editor.get_formatting_state());
    onChange?.(editor.to_markdown());
  }, [onChange]);

  // Handle selection change
  const handleSelectionChange = useCallback(() => {
    const editor = editorRef.current;
    if (!editor) return;
    
    const sel = window.getSelection();
    if (!sel) return;
    
    // Map DOM selection to editor positions
    // ... selection sync logic
    
    setFormatting(editor.get_formatting_state());
  }, []);

  return (
    <div className={className}>
      <Toolbar 
        formatting={formatting}
        onBold={() => editorRef.current?.toggle_bold()}
        onItalic={() => editorRef.current?.toggle_italic()}
        // ... other toolbar actions
      />
      <div
        ref={containerRef}
        contentEditable
        onInput={handleInput}
        onSelect={handleSelectionChange}
        dangerouslySetInnerHTML={{ __html: html }}
        data-placeholder={placeholder}
      />
    </div>
  );
}
```

### 3.2 iOS (SwiftUI)

```swift
// packages/mdeditor-ios/Sources/MDEditor/MDEditorView.swift
import SwiftUI
import MDEditorCore // uniffi-generated

public struct MDEditorView: View {
    @StateObject private var viewModel: MDEditorViewModel
    
    public init(
        initialValue: String = "",
        onChange: ((String) -> Void)? = nil
    ) {
        _viewModel = StateObject(wrappedValue: MDEditorViewModel(
            initialValue: initialValue,
            onChange: onChange
        ))
    }
    
    public var body: some View {
        VStack(spacing: 0) {
            ToolbarView(
                formatting: viewModel.formatting,
                onBold: viewModel.toggleBold,
                onItalic: viewModel.toggleItalic
                // ...
            )
            
            EditorTextView(
                attributedText: $viewModel.attributedText,
                onTextChange: viewModel.handleTextChange,
                onSelectionChange: viewModel.handleSelectionChange
            )
        }
    }
}

// UIViewRepresentable wrapper for UITextView
struct EditorTextView: UIViewRepresentable {
    @Binding var attributedText: NSAttributedString
    var onTextChange: (String, NSRange) -> Void
    var onSelectionChange: (NSRange) -> Void
    
    func makeUIView(context: Context) -> UITextView {
        let textView = UITextView()
        textView.delegate = context.coordinator
        textView.font = .preferredFont(forTextStyle: .body)
        return textView
    }
    
    func updateUIView(_ uiView: UITextView, context: Context) {
        uiView.attributedText = attributedText
    }
    
    func makeCoordinator() -> Coordinator {
        Coordinator(self)
    }
}
```

### 3.3 Android (Jetpack Compose)

```kotlin
// packages/mdeditor-android/src/main/kotlin/MDEditor.kt
package com.mdeditor

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import uniffi.mdeditor.Editor
import uniffi.mdeditor.FormattingState

@Composable
fun MDEditor(
    initialValue: String = "",
    onChange: ((String) -> Unit)? = null,
    modifier: Modifier = Modifier
) {
    var editor by remember { 
        mutableStateOf(
            if (initialValue.isNotEmpty()) Editor(initialValue) 
            else Editor()
        )
    }
    var formatting by remember { mutableStateOf(editor.getFormattingState()) }
    var html by remember { mutableStateOf(editor.toHtml()) }
    
    Column(modifier = modifier) {
        EditorToolbar(
            formatting = formatting,
            onBold = { 
                editor.toggleBold()
                formatting = editor.getFormattingState()
                html = editor.toHtml()
                onChange?.invoke(editor.toMarkdown())
            },
            onItalic = {
                editor.toggleItalic()
                formatting = editor.getFormattingState()
                html = editor.toHtml()
                onChange?.invoke(editor.toMarkdown())
            }
            // ...
        )
        
        // Custom text input with HTML rendering
        EditorTextField(
            html = html,
            onTextChange = { text, range ->
                // Map to editor commands
                // ...
            },
            onSelectionChange = { range ->
                // Sync selection
                // ...
            }
        )
    }
}
```

---

## Part 4: Supported Markdown (v1 Scope)

### Included in v1

| Element | Syntax | Notes |
|---------|--------|-------|
| Paragraphs | Plain text | Separated by blank lines |
| Headings | `# H1` through `###### H6` | ATX style only |
| Bold | `**bold**` | |
| Italic | `*italic*` | |
| Bold + Italic | `***both***` | |
| Inline code | `` `code` `` | |
| Code blocks | ```` ``` ```` | With optional language |
| Links | `[text](url)` | Optional title |
| Images | `![alt](url)` | Display as placeholder in v1 |
| Blockquotes | `> quote` | Nested supported |
| Unordered lists | `- item` or `* item` | Nested supported |
| Ordered lists | `1. item` | Nested supported |
| Horizontal rules | `---` or `***` | |
| Hard breaks | Two trailing spaces | |

### Explicitly NOT in v1

- Tables
- Task lists (checkboxes)
- Footnotes
- Definition lists
- Strikethrough
- Subscript/superscript
- HTML passthrough
- Custom containers
- MDX/JSX

These can be added as extensions in v2+.

---

## Part 5: Implementation Phases

### Phase 1: Core Library (Weeks 1-8)

**Goal:** Rust core that can parse, manipulate, and serialize markdown.

| Week | Deliverable |
|------|-------------|
| 1-2 | Project setup, document model types, basic tests |
| 3-4 | Markdown parser integration (pulldown-cmark), AST ↔ text |
| 5-6 | Command system, text operations, formatting toggles |
| 7-8 | History (undo/redo), selection model, API polish |

**Exit criteria:**
- `cargo test` passes with >90% coverage on core operations
- Can round-trip markdown (parse → AST → serialize) without loss
- All basic commands work correctly

### Phase 2: Web Binding & Component (Weeks 9-14)

**Goal:** Usable React component powered by WASM core.

| Week | Deliverable |
|------|-------------|
| 9-10 | WASM bindings, wasm-pack setup, JS interop tests |
| 11-12 | React component, DOM rendering, toolbar |
| 13-14 | Input handling, selection sync, keyboard shortcuts |

**Exit criteria:**
- npm package `@mdeditor/react` installable
- Works in Chrome, Firefox, Safari
- Basic editing workflow functional

### Phase 3: Polish & Edge Cases (Weeks 15-20)

**Goal:** Production-ready web editor.

| Week | Deliverable |
|------|-------------|
| 15-16 | IME handling (CJK input), paste handling |
| 17-18 | Accessibility (ARIA, screen readers) |
| 19-20 | Performance optimization, large document testing |

**Exit criteria:**
- Handles 10,000+ line documents smoothly
- Passes basic accessibility audit
- IME works for Chinese/Japanese/Korean

### Phase 4: First Native Platform (Weeks 21-28)

**Goal:** iOS app using shared core.

| Week | Deliverable |
|------|-------------|
| 21-22 | uniffi setup, Swift bindings generation |
| 23-24 | SwiftUI wrapper, basic text view integration |
| 25-26 | Toolbar, formatting, selection sync |
| 27-28 | Polish, keyboard handling, iOS idioms |

**Exit criteria:**
- SPM package `MDEditorIOS` installable  
- Native feel on iOS
- Feature parity with web

### Phase 5: Android (Weeks 29-34)

**Goal:** Android library using shared core.

| Week | Deliverable |
|------|-------------|
| 29-30 | JNI/uniffi Kotlin bindings |
| 31-32 | Compose component |
| 33-34 | Polish, Android idioms |

### Future Phases

- **v1.1:** Image uploads (URL + upload API)
- **v1.2:** Tables support
- **v1.3:** Task lists
- **v1.4:** Collaborative editing (CRDT integration)
- **v2.0:** Custom rendering (Canvas on web for ultimate control)

---

## Part 6: Project Structure

```
mdeditor/
├── Cargo.toml                    # Workspace root
├── README.md
├── plan.md                       # This document
│
├── crates/
│   ├── mdeditor-core/            # Core Rust library
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── document.rs
│   │       ├── selection.rs
│   │       ├── commands.rs
│   │       ├── editor.rs
│   │       ├── history.rs
│   │       ├── parser.rs         # pulldown-cmark integration
│   │       └── serializer.rs     # AST → markdown
│   │
│   ├── mdeditor-wasm/            # WASM bindings
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   │
│   └── mdeditor-ffi/             # uniffi bindings (iOS/Android)
│       ├── Cargo.toml
│       ├── src/lib.rs
│       └── uniffi.toml
│
├── packages/
│   ├── mdeditor-react/           # React component
│   │   ├── package.json
│   │   ├── src/
│   │   │   ├── index.ts
│   │   │   ├── Editor.tsx
│   │   │   ├── Toolbar.tsx
│   │   │   └── styles.css
│   │   └── tsconfig.json
│   │
│   ├── mdeditor-ios/             # iOS SwiftUI package
│   │   ├── Package.swift
│   │   └── Sources/MDEditor/
│   │       ├── MDEditorView.swift
│   │       ├── MDEditorViewModel.swift
│   │       └── ToolbarView.swift
│   │
│   └── mdeditor-android/         # Android Compose library
│       ├── build.gradle.kts
│       └── src/main/kotlin/
│           └── com/mdeditor/
│               ├── MDEditor.kt
│               └── EditorToolbar.kt
│
├── examples/
│   ├── web-demo/                 # Standalone web demo
│   ├── ios-demo/                 # iOS sample app
│   └── android-demo/             # Android sample app
│
└── docs/
    ├── architecture.md
    ├── contributing.md
    └── api-reference.md
```

---

## Part 7: Technical Decisions

### Why Rust?

1. **Memory safety** without garbage collection pauses
2. **Excellent WASM support** via wasm-bindgen
3. **uniffi** provides seamless Swift/Kotlin bindings
4. **Performance** for large documents
5. **Rich parsing ecosystem** (pulldown-cmark, pest, nom)

### Why pulldown-cmark?

- CommonMark compliant
- Pure Rust, no dependencies
- Fast (used by docs.rs, rustdoc)
- Event-based API allows streaming

### Why DOM rendering for web v1?

- Accessibility "free" (screen readers work)
- Faster to ship
- Selection API works
- Can upgrade to Canvas in v2 if needed

### Why NOT ContentEditable alternatives?

Options like Slate.js abstract ContentEditable but still sit on top of it. By owning the document model in Rust, we:
- Have single source of truth
- Can debug document state
- Get consistent behavior across platforms

We still USE ContentEditable for input, but the Rust core is authoritative.

---

## Part 8: Success Metrics

### v1 Release Criteria

- [ ] Parse any valid CommonMark document
- [ ] Round-trip without data loss
- [ ] All basic formatting operations work
- [ ] Undo/redo for all operations
- [ ] < 100ms initial load (WASM)
- [ ] < 10ms per keystroke
- [ ] Works offline (no network required)
- [ ] Accessible (keyboard navigation, screen reader compatible)
- [ ] MIT licensed

### Adoption Goals (Year 1)

- 1,000+ GitHub stars
- 10+ production deployments
- Active contributor community
- At least one major integration (VS Code extension, CMS plugin, etc.)

---

## Part 9: Open Questions

Things to decide as development progresses:

1. **Collaboration model** — CRDT vs OT for future real-time editing?
2. **Extension API** — How to allow custom node types in v2+?
3. **Theming** — CSS variables? JS theme objects? Both?
4. **Mobile keyboards** — Custom toolbar vs system keyboard integration?
5. **Offline-first** — Local storage strategy? IndexedDB? SQLite?

---

## Getting Started

```bash
# Clone the repo
git clone https://github.com/yourname/mdeditor
cd mdeditor

# Build the core
cargo build -p mdeditor-core
cargo test -p mdeditor-core

# Build WASM
cd crates/mdeditor-wasm
wasm-pack build --target web

# Run web demo
cd ../../examples/web-demo
npm install
npm run dev
```

---

*This is a living document. Update as decisions are made and scope evolves.*
