# High-Performance Text Streaming Architecture

A comprehensive exploration of streaming AI/LLM text to clients, covering protocols, rendering strategies, and advanced virtualization patterns.

---

## Table of Contents

1. [Protocol Selection: SSE vs WebSocket](#protocol-selection)
2. [HTTP/3 and WebTransport](#http3-and-webtransport)
3. [Structured Events for Tool Calls](#structured-events-for-tool-calls)
4. [Stream Lifecycle in Chat Applications](#stream-lifecycle)
5. [Client-Side Performance Optimization](#client-side-performance)
6. [Virtualization Strategies](#virtualization-strategies)
7. [Server-Backed Virtual Window](#server-backed-virtual-window)
8. [WASM Rendering Architecture](#wasm-rendering-architecture)

---

## Protocol Selection

### Server-Sent Events (SSE) — Recommended for AI Streaming

**Why SSE is typically the better choice:**

- **Unidirectional by design** — AI streaming is inherently server→client
- **Simple implementation** — Works with standard HTTP, integrates with Next.js API routes and edge functions
- **Auto-reconnection** — Browser's `EventSource` API handles reconnection automatically
- **No infrastructure changes** — Works over standard HTTP ports, no proxy/firewall issues
- **Industry standard** — OpenAI, Anthropic, and most AI providers use SSE for streaming
- **HTTP/2 friendly** — Multiplexes well with other requests

**Basic SSE Implementation (Next.js):**

```typescript
// API route
export async function POST(request: NextRequest) {
  const encoder = new TextEncoder();
  const stream = new ReadableStream({
    async start(controller) {
      for await (const chunk of aiStream) {
        controller.enqueue(encoder.encode(`data: ${JSON.stringify(chunk)}\n\n`));
      }
      controller.close();
    },
  });

  return new Response(stream, {
    headers: {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      "Connection": "keep-alive",
    },
  });
}
```

### WebSockets — When to Consider

**Use WebSockets when:**

- **Bidirectional needs** — User can send messages *while* receiving (interrupt generation, send context mid-stream)
- **High-frequency messaging** — Lower per-message overhead once connected
- **Binary data** — Streaming non-text content
- **Existing infrastructure** — WebSocket servers already running

**Drawbacks for AI streaming:**

- More complex setup (requires separate WebSocket server or Socket.io)
- Manual reconnection logic
- Potential proxy/firewall issues
- Overkill if you only need server→client streaming

---

## HTTP/3 and WebTransport

### HTTP/3 Improvements

HTTP/3 **improves SSE** without changing the recommendation:

- **Faster connection setup** — 0-RTT handshakes
- **No head-of-line blocking** — Packet loss on one stream doesn't stall others
- **Better on lossy networks** — QUIC handles packet loss more gracefully

SSE over HTTP/3 is strictly better than HTTP/2, with the same API.

### WebTransport — The Future

WebTransport is potentially the **ideal protocol** for AI streaming:

**Advantages over SSE:**

| Feature | Benefit |
|---------|---------|
| Unidirectional streams | Purpose-built for server→client, lower overhead |
| Bidirectional streams | Available if needed, without switching protocols |
| Datagrams | Unreliable low-latency option |
| No HTTP header overhead | Each chunk is just bytes, not `data: ...\n\n` |
| Native binary | Stream tokens as binary if needed |
| Multiplexing | Independent streams without blocking |

**Current Limitations (as of 2026):**

| Factor | Status |
|--------|--------|
| Browser support | Chrome, Edge (Firefox behind flag, Safari missing) |
| Next.js support | None — requires separate QUIC server |
| Node.js support | Experimental |
| CDN/proxy support | Limited |
| Vercel/edge support | No |

**Recommendation:** Use SSE now, structure abstractions for future transport swap:

```typescript
interface StreamTransport {
  send(chunk: string): void;
  close(): void;
}

// Today: SSE implementation
// Tomorrow: WebTransport implementation (same interface)
```

---

## Structured Events for Tool Calls

For AI agents that need to update client UI (e.g., populate forms), use typed JSON events:

```typescript
// Event types
type StreamEvent =
  | { type: "text"; content: string }
  | { type: "tool_call"; id: string; name: string; args: Record<string, unknown> }
  | { type: "tool_result"; id: string; result: unknown }
  | { type: "done" };
```

**Server side:**

```typescript
export async function POST(request: NextRequest) {
  const encoder = new TextEncoder();
  
  const stream = new ReadableStream({
    async start(controller) {
      const send = (event: StreamEvent) => {
        controller.enqueue(encoder.encode(`data: ${JSON.stringify(event)}\n\n`));
      };

      for await (const chunk of agentStream) {
        if (chunk.type === "text") {
          send({ type: "text", content: chunk.content });
        } else if (chunk.type === "tool_call") {
          send({
            type: "tool_call",
            id: chunk.id,
            name: "fill_form",
            args: { email: "user@example.com", name: "John" },
          });
        }
      }
      
      send({ type: "done" });
      controller.close();
    },
  });

  return new Response(stream, {
    headers: { "Content-Type": "text/event-stream" },
  });
}
```

**Client side:**

```typescript
const processStream = async (response: Response) => {
  const reader = response.body?.getReader();
  const decoder = new TextDecoder();

  while (reader) {
    const { done, value } = await reader.read();
    if (done) break;

    const chunk = decoder.decode(value);
    const lines = chunk.split("\n");

    for (const line of lines) {
      if (!line.startsWith("data: ")) continue;
      const event: StreamEvent = JSON.parse(line.slice(6));

      switch (event.type) {
        case "text":
          setText((prev) => prev + event.content);
          break;
        case "tool_call":
          if (event.name === "fill_form") {
            setFormData((prev) => ({ ...prev, ...event.args }));
          }
          break;
        case "done":
          break;
      }
    }
  }
};
```

### When WebSockets Become Necessary

1. **Client needs to send data mid-stream** — Approve/reject tool calls before execution
2. **Tool execution happens client-side** — Agent calls tool → client runs it → sends result back → agent continues
3. **Persistent conversation state** — Agent needs to "see" user typing in real-time

**Multi-turn SSE (simpler alternative):**

```
Stream 1: Agent → tool_call event → stream ends
Client: Executes tool, gets result
Stream 2: Client sends result → Agent continues → new stream
```

---

## Stream Lifecycle

### One Stream Per Response

Each user message opens a new stream:

```
User sends message → POST /api/chat → Stream opens → AI tokens flow → Stream closes
User sends another → POST /api/chat → New stream opens → ...
```

Conversation history is sent with each request:

```typescript
const response = await fetch("/api/chat", {
  method: "POST",
  body: JSON.stringify({
    messages: [
      { role: "user", content: "Hello" },
      { role: "assistant", content: "Hi there!" },
      { role: "user", content: "Tell me about streams" },
    ],
  }),
});
```

### Interrupting Streams

**1. Client-side abort (primary method):**

```typescript
const abortController = useRef<AbortController | null>(null);

const sendMessage = async (content: string) => {
  abortController.current?.abort();
  abortController.current = new AbortController();

  try {
    const response = await fetch("/api/chat", {
      method: "POST",
      body: JSON.stringify({ messages }),
      signal: abortController.current.signal,
    });
    // Process stream...
  } catch (error) {
    if (error instanceof Error && error.name === "AbortError") {
      return; // User cancelled
    }
    throw error;
  }
};

const stopGenerating = () => abortController.current?.abort();
```

**2. Server-side detection:**

```typescript
const stream = new ReadableStream({
  async start(controller) {
    try {
      for await (const chunk of aiStream) {
        controller.enqueue(encoder.encode(`data: ${JSON.stringify(chunk)}\n\n`));
      }
    } catch (error) {
      await cancelAIGeneration();
    }
    controller.close();
  },
  cancel() {
    cancelAIGeneration();
  },
});
```

**3. Separate HTTP call (belt and suspenders):**

```typescript
const stopGenerating = async () => {
  abortController.current?.abort();
  await fetch("/api/chat/cancel", {
    method: "POST",
    body: JSON.stringify({ streamId: currentStreamId }),
  });
};
```

---

## Client-Side Performance

### 1. Batched State Updates (First optimization)

Don't update React state on every chunk — batch to animation frames:

```typescript
const pendingText = useRef("");
const rafId = useRef<number>();

const processChunk = (text: string) => {
  pendingText.current += text;
  
  if (!rafId.current) {
    rafId.current = requestAnimationFrame(() => {
      setMessages((prev) => {
        const updated = [...prev];
        updated[updated.length - 1].content += pendingText.current;
        return updated;
      });
      pendingText.current = "";
      rafId.current = undefined;
    });
  }
};
```

### 2. Chunked Rendering (For long messages)

Render as array of chunks — React only updates the new chunk:

```typescript
interface Message {
  role: "user" | "assistant";
  chunks: string[];
}

function MessageBubble({ message }: { message: Message }) {
  return (
    <div>
      {message.chunks.map((chunk, i) => (
        <span key={i}>{chunk}</span>
      ))}
    </div>
  );
}

const CHUNK_SIZE = 500;

const processChunk = (text: string) => {
  setMessages((prev) => {
    const updated = [...prev];
    const last = updated[updated.length - 1];
    const lastChunk = last.chunks[last.chunks.length - 1] || "";
    
    if (lastChunk.length + text.length > CHUNK_SIZE) {
      last.chunks.push(text);
    } else {
      last.chunks[last.chunks.length - 1] = lastChunk + text;
    }
    return updated;
  });
};
```

### 3. Virtualized Message List (For many messages)

```typescript
import { useVirtualizer } from "@tanstack/react-virtual";

function MessageList({ messages }: { messages: Message[] }) {
  const parentRef = useRef<HTMLDivElement>(null);
  
  const virtualizer = useVirtualizer({
    count: messages.length,
    getScrollElement: () => parentRef.current,
    estimateSize: (index) => estimateMessageHeight(messages[index]),
    overscan: 5,
  });

  return (
    <div ref={parentRef} style={{ height: "100%", overflow: "auto" }}>
      <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {virtualizer.getVirtualItems().map((virtualRow) => (
          <div
            key={virtualRow.key}
            style={{
              position: "absolute",
              top: virtualRow.start,
              width: "100%",
            }}
          >
            <MessageBubble message={messages[virtualRow.index]} />
          </div>
        ))}
      </div>
    </div>
  );
}
```

### 4. Direct DOM for Active Stream (Nuclear option)

Bypass React entirely for the streaming message:

```typescript
function StreamingMessage() {
  const contentRef = useRef<HTMLDivElement>(null);

  useImperativeHandle(ref, () => ({
    appendText: (text: string) => {
      if (contentRef.current) {
        contentRef.current.textContent += text;
      }
    },
  }));

  return <div ref={contentRef} />;
}

// During streaming — no setState
streamingRef.current?.appendText(chunk);

// Sync to React when complete
onStreamEnd(() => {
  setMessages((prev) => {
    const updated = [...prev];
    updated[updated.length - 1].content = contentRef.current?.textContent || "";
    return updated;
  });
});
```

### 5. Markdown Rendering Optimization

Often the real bottleneck:

```typescript
// Bad: Re-parse entire document every chunk
<ReactMarkdown>{message.content}</ReactMarkdown>

// Better: Only parse completed blocks during streaming
function SmartMarkdown({ content, isStreaming }: Props) {
  if (!isStreaming) {
    return <ReactMarkdown>{content}</ReactMarkdown>;
  }
  
  const lastBreak = content.lastIndexOf("\n\n");
  const complete = content.slice(0, lastBreak);
  const pending = content.slice(lastBreak);
  
  return (
    <>
      <ReactMarkdown>{complete}</ReactMarkdown>
      <span>{pending}</span>
    </>
  );
}
```

### Optimization Decision Matrix

| Message count | Message length | Solution |
|---------------|----------------|----------|
| < 50 | < 5K chars | RAF batching only |
| < 50 | > 5K chars | Chunked rendering + RAF |
| > 50 | Any | Virtualized list + above |
| Code/Markdown heavy | Any | Lazy markdown parsing |

---

## Virtualization Strategies

### Direct DOM with Rich Content

**Option 1: Stream Plain, Render Rich on Complete**

```typescript
function StreamingMessage({ isComplete, finalContent }: Props) {
  const rawRef = useRef<HTMLDivElement>(null);

  const append = (text: string) => {
    if (rawRef.current) {
      rawRef.current.textContent += text;
    }
  };

  if (isComplete) {
    return <ReactMarkdown>{finalContent}</ReactMarkdown>;
  }

  return <div ref={rawRef} className="whitespace-pre-wrap font-mono" />;
}
```

**Option 2: Incremental innerHTML**

```typescript
import { marked } from "marked";
import DOMPurify from "dompurify";

function StreamingMarkdown() {
  const containerRef = useRef<HTMLDivElement>(null);
  const buffer = useRef("");

  const append = (text: string) => {
    buffer.current += text;
    
    const lastSafeBreak = findLastCompleteBlock(buffer.current);
    const safeContent = buffer.current.slice(0, lastSafeBreak);
    const pending = buffer.current.slice(lastSafeBreak);
    
    if (containerRef.current) {
      containerRef.current.innerHTML = 
        DOMPurify.sanitize(marked.parse(safeContent)) +
        `<span class="pending">${escapeHtml(pending)}</span>`;
    }
  };

  return <div ref={containerRef} />;
}

function findLastCompleteBlock(text: string): number {
  const lastParagraph = text.lastIndexOf("\n\n");
  const lastCodeFence = text.lastIndexOf("\n```\n");
  return Math.max(lastParagraph, lastCodeFence, 0);
}
```

**Option 3: Web Worker Parsing**

```typescript
// markdown.worker.ts
import { marked } from "marked";

self.onmessage = (e) => {
  const html = marked.parse(e.data);
  self.postMessage(html);
};

// Component
function WorkerMarkdown() {
  const containerRef = useRef<HTMLDivElement>(null);
  const workerRef = useRef<Worker>();
  const buffer = useRef("");
  const pendingParse = useRef(false);

  useEffect(() => {
    workerRef.current = new Worker(
      new URL("./markdown.worker.ts", import.meta.url)
    );
    workerRef.current.onmessage = (e) => {
      if (containerRef.current) {
        containerRef.current.innerHTML = DOMPurify.sanitize(e.data);
      }
      pendingParse.current = false;
    };
    return () => workerRef.current?.terminate();
  }, []);

  const append = (text: string) => {
    buffer.current += text;
    if (!pendingParse.current) {
      pendingParse.current = true;
      workerRef.current?.postMessage(buffer.current);
    }
  };

  return <div ref={containerRef} />;
}
```

### Approach Comparison

| Approach | Performance | Rich Rendering | Complexity |
|----------|-------------|----------------|------------|
| Plain DOM → Rich on complete | Fastest | Only at end | Low |
| Incremental innerHTML | Fast | Real-time | Medium (XSS risk) |
| Web Worker | Fast | Real-time | High |
| Chunked React | Good enough | Partial during stream | Low |

---

## Server-Backed Virtual Window

The key insight: **the server becomes your "swap file."** Client memory is bounded regardless of stream length.

### Architecture

```
Server: [block0][block1][block2][block3][block4][block5][block6][block7]...
                              ↑                    ↑
Client window:            [block3][block4][block5]
                          (visible + overscan)
```

### Protocol

**During streaming:**
```
Client ←──── SSE stream ────── Server
             │
             ├─ { type: "block", index: 0, content: "..." }
             ├─ { type: "block", index: 1, content: "..." }
             └─ ...
```

**After stream completes (or during, if user scrolls back):**
```
Client ────── GET /stream/{id}/blocks?start=0&end=5 ─────→ Server
       ←───── [block0, block1, block2, block3, block4] ────
```

### Server Implementation (Go + HTTP/3)

```go
type StreamStore struct {
    mu     sync.RWMutex
    blocks []Block
}

type Block struct {
    Index   int    `json:"index"`
    Content string `json:"content"`
    Height  int    `json:"height,omitempty"`
}

// Range endpoint for fetching arbitrary windows
func (s *Server) GetBlockRange(w http.ResponseWriter, r *http.Request) {
    streamID := chi.URLParam(r, "streamID")
    start, _ := strconv.Atoi(r.URL.Query().Get("start"))
    end, _ := strconv.Atoi(r.URL.Query().Get("end"))
    
    store := s.streams[streamID]
    store.mu.RLock()
    defer store.mu.RUnlock()
    
    end = min(end, len(store.blocks))
    
    json.NewEncoder(w).Encode(store.blocks[start:end])
}

// Metadata endpoint
func (s *Server) GetStreamMeta(w http.ResponseWriter, r *http.Request) {
    streamID := chi.URLParam(r, "streamID")
    store := s.streams[streamID]
    
    json.NewEncoder(w).Encode(map[string]any{
        "totalBlocks":  len(store.blocks),
        "totalHeight":  store.calculateTotalHeight(),
        "isComplete":   store.complete,
    })
}
```

### Client Implementation: Sliding Window

```typescript
interface VirtualWindowState {
  streamId: string;
  totalBlocks: number;
  totalHeight: number;
  window: Map<number, Block>;
  windowStart: number;
  windowEnd: number;
}

function useVirtualStream(streamId: string) {
  const [state, setState] = useState<VirtualWindowState>({
    streamId,
    totalBlocks: 0,
    totalHeight: 0,
    window: new Map(),
    windowStart: 0,
    windowEnd: 0,
  });

  const loadRange = useCallback(async (start: number, end: number) => {
    const response = await fetch(
      `/stream/${streamId}/blocks?start=${start}&end=${end}`
    );
    const blocks: Block[] = await response.json();
    
    setState((prev) => {
      const newWindow = new Map<number, Block>();
      
      blocks.forEach((block) => newWindow.set(block.index, block));
      
      // Evict blocks outside window (memory management)
      const keepStart = Math.max(0, start - 20);
      const keepEnd = end + 20;
      
      prev.window.forEach((block, index) => {
        if (index >= keepStart && index < keepEnd) {
          newWindow.set(index, block);
        }
      });
      
      return { ...prev, window: newWindow, windowStart: start, windowEnd: end };
    });
  }, [streamId]);

  const onViewportChange = useCallback((visibleStart: number, visibleEnd: number) => {
    const OVERSCAN = 10;
    const needStart = Math.max(0, visibleStart - OVERSCAN);
    const needEnd = Math.min(state.totalBlocks, visibleEnd + OVERSCAN);
    
    const missingBlocks = [];
    for (let i = needStart; i < needEnd; i++) {
      if (!state.window.has(i)) {
        missingBlocks.push(i);
      }
    }
    
    if (missingBlocks.length > 0) {
      loadRange(Math.min(...missingBlocks), Math.max(...missingBlocks) + 1);
    }
  }, [state.totalBlocks, state.window, loadRange]);

  return { state, onViewportChange, loadRange };
}
```

### Rendering with Sparse Data

```typescript
function VirtualizedStream({ streamId }: { streamId: string }) {
  const { state, onViewportChange } = useVirtualStream(streamId);
  const [scrollTop, setScrollTop] = useState(0);
  
  const VIEWPORT_HEIGHT = 800;
  const ESTIMATED_BLOCK_HEIGHT = 100;

  const visibleRange = useMemo(() => {
    const start = Math.floor(scrollTop / ESTIMATED_BLOCK_HEIGHT);
    const count = Math.ceil(VIEWPORT_HEIGHT / ESTIMATED_BLOCK_HEIGHT);
    return { start, end: start + count };
  }, [scrollTop]);

  useEffect(() => {
    onViewportChange(visibleRange.start, visibleRange.end);
  }, [visibleRange, onViewportChange]);

  return (
    <div
      style={{ height: VIEWPORT_HEIGHT, overflow: "auto" }}
      onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
    >
      <div style={{ height: state.totalHeight, position: "relative" }}>
        {Array.from({ length: state.totalBlocks }, (_, i) => {
          const block = state.window.get(i);
          const top = i * ESTIMATED_BLOCK_HEIGHT;
          
          if (i < visibleRange.start - 10 || i > visibleRange.end + 10) {
            return null;
          }
          
          return (
            <div
              key={i}
              style={{ position: "absolute", top, left: 0, right: 0 }}
            >
              {block ? (
                <BlockRenderer content={block.content} />
              ) : (
                <BlockSkeleton />
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
```

### Memory Characteristics

```
Client memory = (window_size × avg_block_size) + overhead
             ≈ (50 blocks × 2KB) + 10KB
             ≈ 110KB constant
```

No matter how long the stream, client memory stays flat. Tradeoff: network round-trips when scrolling through history.

---

## WASM Rendering Architecture

For maximum performance, bypass JavaScript entirely:

```
┌─────────────────────────────────────────┐
│  Server (Go + HTTP/3)                   │
│  - Stores all blocks                    │
│  - Pre-parses markdown → AST            │
│  - Calculates heights                   │
│  - Serves binary protocol               │
└─────────────────┬───────────────────────┘
                  │ HTTP/3 + Binary protocol
                  ▼
┌─────────────────────────────────────────┐
│  WASM Module (Rust/Go)                  │
│  - Receives binary block data           │
│  - Manages virtual window (ring buffer) │
│  - Renders directly to canvas/DOM       │
│  - Zero JS object allocation            │
│  - Native scroll physics                │
└─────────────────────────────────────────┘
```

### WASM Module Responsibilities

- **Ring buffer** for window — fixed memory allocation
- **Canvas rendering** — skip DOM entirely for text
- **Native scroll handling** — smooth physics without JS overhead
- **Binary protocol** — efficient data transfer
- **JS communication** — only for input events

### When WASM Makes Sense

| Scenario | Recommendation |
|----------|----------------|
| Standard chat UI | Overkill — use React |
| 10K+ token responses | Consider WASM |
| Code editor with streaming | Strong candidate |
| Mobile/low-power devices | Worth exploring |
| Real-time collaborative editing | Good fit |

---

## Summary

### Protocol Selection

| Need | Solution |
|------|----------|
| AI text streaming | SSE |
| Bidirectional mid-stream | WebSocket |
| Future-proof architecture | Abstract transport layer |

### Performance Optimization Path

1. **Start with RAF batching** — highest impact, lowest complexity
2. **Add chunked rendering** — if single messages are long
3. **Add message virtualization** — if many messages
4. **Consider direct DOM** — only if profiling shows React as bottleneck
5. **Server-backed window** — for truly massive streams
6. **WASM rendering** — for specialized high-performance needs

### Key Insights

- SSE handles 90% of AI streaming use cases
- Virtualize at the **message level**, not text-block level
- The server can be your "swap file" for bounded client memory
- Profile before optimizing — React is usually fast enough
- WebTransport is the future, but not production-ready yet

---

## Future Exploration

- [ ] Build HTTP/3 + Go streaming server
- [ ] Implement server-backed virtual window
- [ ] Explore WASM text rendering (Rust + wgpu or canvas)
- [ ] Benchmark: SSE vs WebTransport when available
- [ ] Ring buffer implementation for fixed-memory streaming
- [ ] Binary protocol design for block transfer
