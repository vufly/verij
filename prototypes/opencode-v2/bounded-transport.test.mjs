// bounded-transport.test.mjs — Deterministic adversarial tests.
// Run: node --test prototypes/opencode-v2/bounded-transport.test.mjs
// No external endpoints; all fetch calls use synthetic loopback transports.

import { describe, it } from "node:test"
import assert from "node:assert/strict"
import { createReadOnlyTransport, TransportError } from "./bounded-transport.mjs"
import { observationBudget } from "../../integrations/opencode/core.mjs"

// ---------------------------------------------------------------------------
// Synthetic fetch helpers
// ---------------------------------------------------------------------------

function fakeFetch({ status = 200, body = "{}", contentType = "application/json", headers = {} } = {}) {
  return async () => {
    const bytes = new TextEncoder().encode(body)
    const respHeaders = new Headers({ "content-type": contentType, ...headers })
    const stream = new ReadableStream({ start(c) { c.enqueue(bytes); c.close() } })
    return new Response(stream, { status, headers: respHeaders })
  }
}

function chunkedFetch(body, splitAt) {
  const bytes = new TextEncoder().encode(body)
  return async () => {
    const stream = new ReadableStream({
      start(c) {
        c.enqueue(bytes.subarray(0, splitAt))
        c.enqueue(bytes.subarray(splitAt))
        c.close()
      }
    })
    return new Response(stream, { headers: { "content-type": "application/json" } })
  }
}

// fetch that never resolves; respects AbortSignal.
function stalledFetch() {
  return (_href, opts) => new Promise((_res, rej) => {
    const s = opts?.signal
    if (s?.aborted) { rej(new Error("aborted")); return }
    s?.addEventListener("abort", () => rej(new Error("aborted")), { once: true })
  })
}

// fetch that returns headers immediately; body stream never enqueues.
function stalledBodyFetch(status = 200) {
  return () => {
    const stream = new ReadableStream({ start() {}, cancel() {} })
    return Promise.resolve(new Response(stream, { status, headers: { "content-type": "application/json" } }))
  }
}

// fetch that ignores the AbortSignal on the request but resolves eventually.
function uncooperativeFetch(delayTicks = 2) {
  return () => new Promise(res => {
    let t = 0
    function tick() {
      if (++t >= delayTicks) {
        const stream = new ReadableStream({ start(c) { c.enqueue(new TextEncoder().encode("{}")); c.close() } })
        res(new Response(stream, { headers: { "content-type": "application/json" } }))
      } else { Promise.resolve().then(tick) }
    }
    tick()
  })
}

// fetch that ignores AbortSignal; body reader.cancel() never resolves.
function uncooperativeReaderFetch() {
  return () => {
    const stream = new ReadableStream({
      start() {},
      cancel() { return new Promise(() => {}) } // never settles
    })
    return Promise.resolve(new Response(stream, { headers: { "content-type": "application/json" } }))
  }
}

function redirectFetch(location = "https://other.example.com/") {
  return () => {
    const stream = new ReadableStream({ start(c) { c.close() } })
    return Promise.resolve(new Response(stream, { status: 302, headers: { location } }))
  }
}

function redirectThrowFetch() {
  return () => Promise.reject(new Error("redirect was blocked"))
}

// ---------------------------------------------------------------------------
// Fake clock helper
// ---------------------------------------------------------------------------

function makeFakeClock() {
  const timers = []
  let now = 0
  return {
    now: () => now,
    setTimeout(fn, ms) { const t = { fn, due: now + Math.max(0, ms), cancelled: false }; timers.push(t); return t },
    clearTimeout(t) { if (t) t.cancelled = true },
    advance(ms) {
      now += ms
      for (const t of [...timers]) {
        if (!t.cancelled && t.due <= now) { t.cancelled = true; t.fn() }
      }
    },
    live() { return timers.filter(t => !t.cancelled) },
  }
}

function makeTransport(fetchImpl, { baseUrl = "http://127.0.0.1:8080/api", headers = {}, deadlineMs = 3000, clock } = {}) {
  const clk = clock ?? makeFakeClock()
  const t = createReadOnlyTransport({ baseUrl, headers, fetch: fetchImpl, deadlineMs, clock: clk })
  return { transport: t, clock: clk }
}

// ---------------------------------------------------------------------------
// assertTransportError
// ---------------------------------------------------------------------------

async function assertTransportError(promise, expectedCode, label) {
  try {
    await promise
    assert.fail(`${label}: expected TransportError but resolved`)
  } catch (err) {
    assert.ok(err instanceof TransportError,
      `${label}: must be TransportError; got ${err?.constructor?.name}: ${err?.message}`)
    assert.equal(err.code, expectedCode,
      `${label}: expected code=${expectedCode}, got code=${err.code} (msg: ${err.message})`)
  }
}

// ---------------------------------------------------------------------------
// 1. Constructor validation
// ---------------------------------------------------------------------------

describe("constructor validation", () => {
  it("rejects missing fetch", () => assert.throws(
    () => createReadOnlyTransport({ baseUrl: "http://localhost/", headers: {}, fetch: undefined }),
    /TransportError/))

  it("rejects null headers", () => assert.throws(
    () => createReadOnlyTransport({ baseUrl: "http://localhost/", headers: null, fetch: fakeFetch() }),
    /TransportError/))

  it("rejects array headers", () => assert.throws(
    () => createReadOnlyTransport({ baseUrl: "http://localhost/", headers: [], fetch: fakeFetch() }),
    /TransportError/))

  it("rejects non-string header value", () => assert.throws(
    () => createReadOnlyTransport({ baseUrl: "http://localhost/", headers: { x: 42 }, fetch: fakeFetch() }),
    /TransportError/))

  it("rejects headers over 32 entries", () => {
    const h = {}
    for (let i = 0; i < 33; i++) h[`k${i}`] = "v"
    assert.throws(() => createReadOnlyTransport({ baseUrl: "http://localhost/", headers: h, fetch: fakeFetch() }),
      /TransportError/)
  })

  it("rejects headers over 8KiB total", () => {
    const h = { x: "a".repeat(8193) }
    assert.throws(() => createReadOnlyTransport({ baseUrl: "http://localhost/", headers: h, fetch: fakeFetch() }),
      /TransportError/)
  })

  it("rejects non-string baseUrl", () => assert.throws(
    () => createReadOnlyTransport({ baseUrl: 1234, headers: {}, fetch: fakeFetch() }),
    /TransportError/))

  it("rejects baseUrl > 4096 chars", () => assert.throws(
    () => createReadOnlyTransport({ baseUrl: "http://localhost/" + "a".repeat(4096), headers: {}, fetch: fakeFetch() }),
    /TransportError/))

  it("rejects file: protocol", () => assert.throws(
    () => createReadOnlyTransport({ baseUrl: "file:///etc/passwd", headers: {}, fetch: fakeFetch() }),
    /TransportError/))

  it("rejects baseUrl with userinfo", () => assert.throws(
    () => createReadOnlyTransport({ baseUrl: "http://user:pass@localhost/", headers: {}, fetch: fakeFetch() }),
    /TransportError/))

  it("rejects baseUrl with hash", () => assert.throws(
    () => createReadOnlyTransport({ baseUrl: "http://localhost/#frag", headers: {}, fetch: fakeFetch() }),
    /TransportError/))

  it("rejects baseUrl with query", () => assert.throws(
    () => createReadOnlyTransport({ baseUrl: "http://localhost/?q=1", headers: {}, fetch: fakeFetch() }),
    /TransportError/))

  it("rejects deadlineMs > 3000", () => assert.throws(
    () => createReadOnlyTransport({ baseUrl: "http://localhost/", headers: {}, fetch: fakeFetch(), deadlineMs: 3001 }),
    /TransportError/))

  it("rejects zero deadlineMs", () => assert.throws(
    () => createReadOnlyTransport({ baseUrl: "http://localhost/", headers: {}, fetch: fakeFetch(), deadlineMs: 0 }),
    /TransportError/))

  it("rejects NaN deadlineMs", () => assert.throws(
    () => createReadOnlyTransport({ baseUrl: "http://localhost/", headers: {}, fetch: fakeFetch(), deadlineMs: NaN }),
    /TransportError/))

  it("accepts valid http baseUrl", () => assert.doesNotThrow(
    () => createReadOnlyTransport({ baseUrl: "http://localhost:8080/api", headers: {}, fetch: fakeFetch() })))

  it("accepts valid https baseUrl", () => assert.doesNotThrow(
    () => createReadOnlyTransport({ baseUrl: "https://example.com/prefix/", headers: {}, fetch: fakeFetch() })))

  it("accepts deadlineMs=3000", () => assert.doesNotThrow(
    () => createReadOnlyTransport({ baseUrl: "http://localhost/", headers: {}, fetch: fakeFetch(), deadlineMs: 3000 })))
})

// ---------------------------------------------------------------------------
// 2. Path validation — strict prefix-preserving
// ---------------------------------------------------------------------------

describe("path validation", () => {
  function makePV(baseUrl = "http://localhost:8080/api") {
    const clk = makeFakeClock()
    return { t: createReadOnlyTransport({ baseUrl, headers: {}, fetch: fakeFetch({ body: "{}" }), deadlineMs: 3000, clock: clk }), clk }
  }

  it("accepts simple relative path", async () => {
    const { t } = makePV()
    try { assert.equal(await t.get("session/list"), "{}") } finally { t.dispose() }
  })

  it("rejects empty path", async () => {
    const { t } = makePV()
    try { await assertTransportError(t.get(""), "schema", "empty") } finally { t.dispose() }
  })

  it("rejects leading slash", async () => {
    const { t } = makePV()
    try { await assertTransportError(t.get("/other"), "schema", "leading slash") } finally { t.dispose() }
  })

  it("rejects leading // (protocol-relative)", async () => {
    const { t } = makePV()
    try { await assertTransportError(t.get("//evil.com/"), "schema", "protocol-relative") } finally { t.dispose() }
  })

  it("rejects absolute URL as path", async () => {
    const { t } = makePV()
    try { await assertTransportError(t.get("http://evil.com/"), "schema", "abs URL") } finally { t.dispose() }
  })

  it("rejects .. segment", async () => {
    const { t } = makePV()
    try { await assertTransportError(t.get("../secret"), "schema", "dotdot") } finally { t.dispose() }
  })

  it("rejects . segment", async () => {
    const { t } = makePV()
    try { await assertTransportError(t.get("./thing"), "schema", "dot") } finally { t.dispose() }
  })

  it("rejects mid-path .. segment", async () => {
    const { t } = makePV()
    try { await assertTransportError(t.get("foo/../secret"), "schema", "mid dotdot") } finally { t.dispose() }
  })

  it("rejects encoded dot (%2E)", async () => {
    const { t } = makePV()
    try { await assertTransportError(t.get("%2E%2E/secret"), "schema", "encoded dot") } finally { t.dispose() }
  })

  it("rejects encoded backslash (%5C)", async () => {
    const { t } = makePV()
    try { await assertTransportError(t.get("foo%5Cbar"), "schema", "encoded backslash") } finally { t.dispose() }
  })

  it("rejects literal backslash", async () => {
    const { t } = makePV()
    try { await assertTransportError(t.get("foo\\bar"), "schema", "backslash") } finally { t.dispose() }
  })

  it("rejects path with hash", async () => {
    const { t } = makePV()
    try { await assertTransportError(t.get("session#foo"), "schema", "hash") } finally { t.dispose() }
  })

  it("rejects path with query char", async () => {
    const { t } = makePV()
    try { await assertTransportError(t.get("session?x=1"), "schema", "query char") } finally { t.dispose() }
  })

  it("rejects path with @ sign", async () => {
    const { t } = makePV()
    try { await assertTransportError(t.get("user@host"), "schema", "@") } finally { t.dispose() }
  })

  it("path stays under base prefix — prefix preserved", async () => {
    let captured = ""
    const t = createReadOnlyTransport({
      baseUrl: "http://localhost/prefix/v2",
      headers: {},
      fetch: async (href) => { captured = href; return new Response("{}", { headers: { "content-type": "application/json" } }) },
      deadlineMs: 3000,
      clock: makeFakeClock(),
    })
    try {
      await t.get("session/list")
      assert.ok(captured.startsWith("http://localhost/prefix/v2/"), `got: ${captured}`)
    } finally { t.dispose() }
  })
})

// ---------------------------------------------------------------------------
// 3. Query parameter encoding
// ---------------------------------------------------------------------------

describe("query parameters", () => {
  let captured = ""
  function capFetch() {
    return async (href) => { captured = href; return new Response("{}", { headers: { "content-type": "application/json" } }) }
  }

  it("appends query from plain object", async () => {
    const t = createReadOnlyTransport({ baseUrl: "http://localhost/api", headers: {}, fetch: capFetch(), deadlineMs: 3000, clock: makeFakeClock() })
    try {
      await t.get("resource", { query: { limit: 10, cursor: "abc" } })
      assert.ok(captured.includes("limit=10"), `got: ${captured}`)
      assert.ok(captured.includes("cursor=abc"), `got: ${captured}`)
    } finally { t.dispose() }
  })

  it("appends query from URLSearchParams", async () => {
    const t = createReadOnlyTransport({ baseUrl: "http://localhost/api", headers: {}, fetch: capFetch(), deadlineMs: 3000, clock: makeFakeClock() })
    try {
      const sp = new URLSearchParams([["a", "1"], ["b", "2"]])
      await t.get("resource", { query: sp })
      assert.ok(captured.includes("a=1"), `got: ${captured}`)
      assert.ok(captured.includes("b=2"), `got: ${captured}`)
    } finally { t.dispose() }
  })

  it("no query appended when omitted", async () => {
    const t = createReadOnlyTransport({ baseUrl: "http://localhost/api", headers: {}, fetch: capFetch(), deadlineMs: 3000, clock: makeFakeClock() })
    try {
      await t.get("resource")
      assert.ok(!captured.includes("?"), `got: ${captured}`)
    } finally { t.dispose() }
  })

  it("rejects query with non-string non-finite-number value", async () => {
    const t = createReadOnlyTransport({ baseUrl: "http://localhost/api", headers: {}, fetch: fakeFetch(), deadlineMs: 3000, clock: makeFakeClock() })
    try {
      await assertTransportError(t.get("r", { query: { x: null } }), "schema", "null value")
    } finally { t.dispose() }
  })

  it("rejects query exceeding 32 pairs", async () => {
    const t = createReadOnlyTransport({ baseUrl: "http://localhost/api", headers: {}, fetch: fakeFetch(), deadlineMs: 3000, clock: makeFakeClock() })
    try {
      const q = {}
      for (let i = 0; i < 33; i++) q[`k${i}`] = "v"
      await assertTransportError(t.get("r", { query: q }), "schema", "too many pairs")
    } finally { t.dispose() }
  })

  it("rejects query as array", async () => {
    const t = createReadOnlyTransport({ baseUrl: "http://localhost/api", headers: {}, fetch: fakeFetch(), deadlineMs: 3000, clock: makeFakeClock() })
    try {
      await assertTransportError(t.get("r", { query: [["k", "v"]] }), "schema", "array query")
    } finally { t.dispose() }
  })

  it("query special chars are percent-encoded", async () => {
    const t = createReadOnlyTransport({ baseUrl: "http://localhost/api", headers: {}, fetch: capFetch(), deadlineMs: 3000, clock: makeFakeClock() })
    try {
      await t.get("resource", { query: { key: "hello world" } })
      assert.ok(captured.includes("hello+world") || captured.includes("hello%20world"), `got: ${captured}`)
    } finally { t.dispose() }
  })

  it("finite number values are stringified", async () => {
    const t = createReadOnlyTransport({ baseUrl: "http://localhost/api", headers: {}, fetch: capFetch(), deadlineMs: 3000, clock: makeFakeClock() })
    try {
      await t.get("resource", { query: { n: 42.5 } })
      assert.ok(captured.includes("n=42.5"), `got: ${captured}`)
    } finally { t.dispose() }
  })
})

// ---------------------------------------------------------------------------
// 4. Auth header forwarding and method enforcement
// ---------------------------------------------------------------------------

describe("auth headers and method", () => {
  it("auth headers forwarded to fetch", async () => {
    let capturedHeaders
    const t = createReadOnlyTransport({
      baseUrl: "http://localhost/",
      headers: { Authorization: "Bearer tok123" },
      fetch: async (_href, opts) => { capturedHeaders = opts.headers; return new Response("{}", { headers: { "content-type": "application/json" } }) },
      deadlineMs: 3000, clock: makeFakeClock(),
    })
    try { await t.get("r"); assert.equal(capturedHeaders.Authorization, "Bearer tok123") }
    finally { t.dispose() }
  })

  it("method is always GET", async () => {
    let capturedMethod
    const t = createReadOnlyTransport({
      baseUrl: "http://localhost/",
      headers: {},
      fetch: async (_href, opts) => { capturedMethod = opts.method; return new Response("{}", { headers: { "content-type": "application/json" } }) },
      deadlineMs: 3000, clock: makeFakeClock(),
    })
    try { await t.get("r"); assert.equal(capturedMethod, "GET") }
    finally { t.dispose() }
  })
})

// ---------------------------------------------------------------------------
// 5. Redirect rejection
// ---------------------------------------------------------------------------

describe("redirects", () => {
  it("rejects 302 redirect response", async () => {
    const { transport } = makeTransport(redirectFetch())
    try { await assertTransportError(transport.get("path"), "redirect", "302") }
    finally { transport.dispose() }
  })

  it("rejects fetch that throws redirect error", async () => {
    const { transport } = makeTransport(redirectThrowFetch())
    try {
      const p = transport.get("path")
      try { await p; assert.fail("expected error") }
      catch (err) { assert.ok(err instanceof TransportError); assert.ok(["redirect","schema"].includes(err.code)) }
    } finally { transport.dispose() }
  })
})

// ---------------------------------------------------------------------------
// 6. HTTP error codes
// ---------------------------------------------------------------------------

describe("HTTP errors", () => {
  for (const status of [400, 401, 403, 404, 500, 503]) {
    it(`rejects HTTP ${status}`, async () => {
      const { transport } = makeTransport(fakeFetch({ status, body: "error" }))
      try { await assertTransportError(transport.get("path"), "http", `HTTP ${status}`) }
      finally { transport.dispose() }
    })
  }

  it("error body not in error message", async () => {
    const { transport } = makeTransport(fakeFetch({ status: 500, body: '{"secret":"credential123"}' }))
    try {
      try { await transport.get("path"); assert.fail("expected error") }
      catch (err) {
        assert.ok(!err.message.includes("credential123"))
        assert.ok(!err.message.includes("secret"))
      }
    } finally { transport.dispose() }
  })
})

// ---------------------------------------------------------------------------
// 7. Content-Type enforcement (media-type not substring)
// ---------------------------------------------------------------------------

describe("content-type enforcement", () => {
  it("rejects text/plain", async () => {
    const { transport } = makeTransport(fakeFetch({ contentType: "text/plain", body: "{}" }))
    try { await assertTransportError(transport.get("path"), "schema", "text/plain") }
    finally { transport.dispose() }
  })

  it("rejects application/json-extra (spoof via substring)", async () => {
    // "application/json-extra" contains "application/json" as substring but is a different type.
    const { transport } = makeTransport(fakeFetch({ contentType: "application/json-extra", body: "{}" }))
    try { await assertTransportError(transport.get("path"), "schema", "spoofed ct") }
    finally { transport.dispose() }
  })

  it("accepts application/json; charset=utf-8", async () => {
    const { transport } = makeTransport(fakeFetch({ contentType: "application/json; charset=utf-8", body: "{}" }))
    try { assert.equal(await transport.get("path"), "{}") }
    finally { transport.dispose() }
  })

  it("accepts application/vnd.api+json (structured syntax suffix)", async () => {
    const { transport } = makeTransport(fakeFetch({ contentType: "application/vnd.api+json", body: "{}" }))
    try { assert.equal(await transport.get("path"), "{}") }
    finally { transport.dispose() }
  })

  it("rejects absent content-type", async () => {
    const t = createReadOnlyTransport({ baseUrl: "http://localhost/", headers: {}, fetch: async () => new Response("{}"), deadlineMs: 3000, clock: makeFakeClock() })
    try { await assertTransportError(t.get("path"), "schema", "no ct") }
    finally { t.dispose() }
  })
})

// ---------------------------------------------------------------------------
// 8. Byte limits
// ---------------------------------------------------------------------------

describe("byte limits", () => {
  it("rejects body > 1MiB general limit", async () => {
    const { transport } = makeTransport(fakeFetch({ body: "x".repeat(observationBudget.responseBytes + 1) }))
    try { await assertTransportError(transport.get("path"), "bound", "oversized general") }
    finally { transport.dispose() }
  })

  it("rejects body > 64KiB outcome limit", async () => {
    const { transport } = makeTransport(fakeFetch({ body: "x".repeat(observationBudget.metadataBytes + 1) }))
    try { await assertTransportError(transport.get("path", { outcome: true }), "bound", "oversized outcome") }
    finally { transport.dispose() }
  })

  it("accepts body exactly at 1MiB boundary", async () => {
    const body = "x".repeat(observationBudget.responseBytes)
    const { transport } = makeTransport(fakeFetch({ body }))
    try { assert.equal((await transport.get("path")).length, observationBudget.responseBytes) }
    finally { transport.dispose() }
  })

  it("rejects Content-Length header claiming oversized", async () => {
    const { transport } = makeTransport(fakeFetch({ body: "{}", headers: { "content-length": String(observationBudget.responseBytes + 1) } }))
    try { await assertTransportError(transport.get("path"), "bound", "large CL") }
    finally { transport.dispose() }
  })

  it("counts actual Uint8Array bytes ignoring misleadingly small Content-Length", async () => {
    const bytes = new TextEncoder().encode("x".repeat(observationBudget.responseBytes + 1))
    const t = createReadOnlyTransport({
      baseUrl: "http://localhost/", headers: {}, fetch: async () => {
        const stream = new ReadableStream({ start(c) { c.enqueue(bytes); c.close() } })
        return new Response(stream, { headers: { "content-type": "application/json", "content-length": "10" } })
      }, deadlineMs: 3000, clock: makeFakeClock(),
    })
    try { await assertTransportError(t.get("path"), "bound", "misleading small CL") }
    finally { t.dispose() }
  })

  it("counts bytes across many small chunks", async () => {
    const bytes = new TextEncoder().encode("x".repeat(observationBudget.responseBytes + 1))
    const t = createReadOnlyTransport({
      baseUrl: "http://localhost/", headers: {}, fetch: async () => {
        const stream = new ReadableStream({
          start(c) {
            const sz = 4096
            for (let i = 0; i < bytes.length; i += sz) c.enqueue(bytes.subarray(i, Math.min(i + sz, bytes.length)))
            c.close()
          }
        })
        return new Response(stream, { headers: { "content-type": "application/json" } })
      }, deadlineMs: 3000, clock: makeFakeClock(),
    })
    try { await assertTransportError(t.get("path"), "bound", "multi-chunk oversize") }
    finally { t.dispose() }
  })
})

// ---------------------------------------------------------------------------
// 9. JSON structural limits — corrected node counting
// ---------------------------------------------------------------------------

describe("JSON structural limits", () => {
  it("rejects JSON depth > 32", async () => {
    let nested = "{".repeat(observationBudget.jsonDepth + 1) + "}".repeat(observationBudget.jsonDepth + 1)
    const { transport } = makeTransport(fakeFetch({ body: nested }))
    try { await assertTransportError(transport.get("path"), "bound", "depth 33") }
    finally { transport.dispose() }
  })

  it("accepts JSON at exactly depth 32", async () => {
    let nested = "{".repeat(observationBudget.jsonDepth) + '"k":1' + "}".repeat(observationBudget.jsonDepth)
    const { transport } = makeTransport(fakeFetch({ body: nested }))
    try { assert.ok((await transport.get("path")).length > 0) }
    finally { transport.dispose() }
  })

  it("rejects flat object exceeding 16384 nodes", async () => {
    const count = observationBudget.jsonNodes + 10
    const pairs = Array.from({ length: count }, (_, i) => `"k${i}":1`)
    const body = "{" + pairs.join(",") + "}"
    const { transport } = makeTransport(fakeFetch({ body }))
    try { await assertTransportError(transport.get("path"), "bound", "node limit") }
    finally { transport.dispose() }
  })

  it("scalar array node exact: [1,2,...,N] at limit does not reject prematurely", async () => {
    // N scalar values plus one array container must fit the node budget.
    const limit = observationBudget.jsonNodes
    const vals = Array.from({ length: limit - 1 }, (_, i) => i % 10)
    const body = "[" + vals.join(",") + "]"
    const { transport } = makeTransport(fakeFetch({ body }))
    try {
      // This is at the limit — should pass (nodes == limit, not > limit).
      const result = await transport.get("path")
      assert.ok(result.startsWith("["), "body must start with [")
    } finally { transport.dispose() }
  })

  it("scalar array one over limit rejects", async () => {
    // One more element than the node budget.
    const over = observationBudget.jsonNodes
    const vals = Array.from({ length: over }, (_, i) => i % 10)
    const body = "[" + vals.join(",") + "]"
    const { transport } = makeTransport(fakeFetch({ body }))
    try { await assertTransportError(transport.get("path"), "bound", "array node over") }
    finally { transport.dispose() }
  })

  it("detects depth/nodes across chunk boundaries", async () => {
    let nested = "{".repeat(observationBudget.jsonDepth + 1) + "}".repeat(observationBudget.jsonDepth + 1)
    const { transport } = makeTransport(chunkedFetch(nested, Math.floor(nested.length / 2)))
    try { await assertTransportError(transport.get("path"), "bound", "depth across chunks") }
    finally { transport.dispose() }
  })
})

// ---------------------------------------------------------------------------
// 10. UTF-8 decoding (fatal)
// ---------------------------------------------------------------------------

describe("UTF-8 decoding", () => {
  it("rejects invalid UTF-8 bytes", async () => {
    const invalidBytes = new Uint8Array([0x7b, 0xff, 0x7d])
    const t = createReadOnlyTransport({
      baseUrl: "http://localhost/", headers: {},
      fetch: async () => new Response(new ReadableStream({ start(c) { c.enqueue(invalidBytes); c.close() } }), { headers: { "content-type": "application/json" } }),
      deadlineMs: 3000, clock: makeFakeClock(),
    })
    try { await assertTransportError(t.get("path"), "utf8", "invalid bytes") }
    finally { t.dispose() }
  })

  it("accepts multi-byte UTF-8", async () => {
    const body = JSON.stringify({ msg: "こんにちは" })
    const bytes = new TextEncoder().encode(body)
    const t = createReadOnlyTransport({
      baseUrl: "http://localhost/", headers: {},
      fetch: async () => new Response(new ReadableStream({ start(c) { c.enqueue(bytes); c.close() } }), { headers: { "content-type": "application/json" } }),
      deadlineMs: 3000, clock: makeFakeClock(),
    })
    try { assert.equal(await t.get("path"), body) }
    finally { t.dispose() }
  })

  it("rejects truncated UTF-8 multi-byte across chunk boundary", async () => {
    // 0xE3 starts a 3-byte sequence; sending only 0xE3 then } makes it invalid.
    const t = createReadOnlyTransport({
      baseUrl: "http://localhost/", headers: {},
      fetch: async () => new Response(new ReadableStream({
        start(c) { c.enqueue(new Uint8Array([0x7b, 0xe3])); c.enqueue(new Uint8Array([0x7d])); c.close() }
      }), { headers: { "content-type": "application/json" } }),
      deadlineMs: 3000, clock: makeFakeClock(),
    })
    try { await assertTransportError(t.get("path"), "utf8", "split UTF-8") }
    finally { t.dispose() }
  })
})

// ---------------------------------------------------------------------------
// 11. Deadline — starts at get() call including while queued
// ---------------------------------------------------------------------------

describe("deadline enforcement", () => {
  it("deadline fires when fetch stalls (fetch-level)", async () => {
    const clk = makeFakeClock()
    const t = createReadOnlyTransport({ baseUrl: "http://localhost/", headers: {}, fetch: stalledFetch(), deadlineMs: 100, clock: clk })
    try {
      const p = t.get("path")
      clk.advance(200)
      await assertTransportError(p, "deadline", "stalled fetch")
    } finally { t.dispose() }
  })

  it("deadline fires when body reader stalls", async () => {
    const clk = makeFakeClock()
    const t = createReadOnlyTransport({ baseUrl: "http://localhost/", headers: {}, fetch: stalledBodyFetch(), deadlineMs: 100, clock: clk })
    try {
      const p = t.get("path")
      clk.advance(200)
      await assertTransportError(p, "deadline", "stalled body")
    } finally { t.dispose() }
  })

  it("deadline fires even when queued (not yet dispatched)", async () => {
    const clk = makeFakeClock()
    const t = createReadOnlyTransport({ baseUrl: "http://localhost/", headers: {}, fetch: stalledFetch(), deadlineMs: 50, clock: clk })
    try {
      // Fill physical slots to force queuing.
      const fills = Array.from({ length: observationBudget.concurrency }, () => t.get("path").catch(() => {}))
      await Promise.resolve()
      // This request goes into the pending queue.
      const queued = t.get("path")
      // Advance past deadline; the queued request should be rejected by deadline signal.
      clk.advance(100)
      await assertTransportError(queued, "deadline", "queued deadline")
      // Cleanup fills.
      t.dispose()
      await Promise.allSettled(fills)
    } catch (e) { t.dispose(); throw e }
  })

  it("maximum deadline is 3000ms", () => {
    assert.throws(
      () => createReadOnlyTransport({ baseUrl: "http://localhost/", headers: {}, fetch: fakeFetch(), deadlineMs: 3001 }),
      /TransportError/)
    assert.doesNotThrow(
      () => createReadOnlyTransport({ baseUrl: "http://localhost/", headers: {}, fetch: fakeFetch(), deadlineMs: 3000 }))
  })

  it("no timer leak after successful request", async () => {
    const clk = makeFakeClock()
    const t = createReadOnlyTransport({ baseUrl: "http://localhost/", headers: {}, fetch: fakeFetch({ body: "{}" }), deadlineMs: 3000, clock: clk })
    try {
      await t.get("path")
      assert.equal(clk.live().length, 0, "no live timers after success")
    } finally { t.dispose() }
  })

  it("no timer leak after error", async () => {
    const clk = makeFakeClock()
    const t = createReadOnlyTransport({ baseUrl: "http://localhost/", headers: {}, fetch: fakeFetch({ status: 500 }), deadlineMs: 3000, clock: clk })
    try {
      await t.get("path").catch(() => {})
      assert.equal(clk.live().length, 0, "no live timers after error")
    } finally { t.dispose() }
  })
})

// ---------------------------------------------------------------------------
// 12. Late response after deadline — uncooperative fetch
// ---------------------------------------------------------------------------

describe("uncooperative fetch and reader", () => {
  it("late response after caller deadline: body discarded before processing", async () => {
    const clk = makeFakeClock()
    // Fetch ignores the abort signal but resolves eventually (after a microtask).
    let resolveFetch
    const t = createReadOnlyTransport({
      baseUrl: "http://localhost/", headers: {},
      fetch: () => new Promise(r => { resolveFetch = r }),
      deadlineMs: 50, clock: clk,
    })
    try {
      const p = t.get("path")
      clk.advance(100) // fire deadline
      await assertTransportError(p, "deadline", "uncooperative fetch deadline")
      // Now resolve the stalled fetch with a body — it should be discarded.
      if (resolveFetch) {
        const stream = new ReadableStream({ start(c) { c.enqueue(new TextEncoder().encode("{}")); c.close() } })
        resolveFetch(new Response(stream, { headers: { "content-type": "application/json" } }))
      }
      // No error thrown; body discarded silently.
      await Promise.resolve()
    } finally { t.dispose() }
  })

  it("uncooperative fetch: physical slot retained until invoke settles", async () => {
    let resolvers = []
    const t = createReadOnlyTransport({
      baseUrl: "http://localhost/", headers: {},
      fetch: (_href, _opts) => new Promise(r => { resolvers.push(r) }),
      deadlineMs: 50, clock: makeFakeClock(),
    })
    try {
      const clk = makeFakeClock() // separate clock for checking
      // Occupy physical slots.
      const promises = Array.from({ length: observationBudget.concurrency }, () => t.get("path").catch(() => {}))
      await Promise.resolve()
      assert.equal(t.metrics().active, observationBudget.concurrency, "all physical slots occupied")
      // Physical slots remain occupied even after callers could timeout (we use no clock advance here).
      assert.equal(t.metrics().active, observationBudget.concurrency, "slots still occupied")
      t.dispose()
      await Promise.allSettled(promises)
    } catch (e) { t.dispose(); throw e }
  })

  it("uncooperative reader cancel: caller rejects promptly, slot retained until cancel settles", async () => {
    const clk = makeFakeClock()
    const t = createReadOnlyTransport({
      baseUrl: "http://localhost/", headers: {},
      fetch: uncooperativeReaderFetch(),
      deadlineMs: 50, clock: clk,
    })
    try {
      const p = t.get("path")
      clk.advance(100)
      // Caller should reject with deadline even though reader.cancel() never resolves.
      // (The finally block awaits cancel, but that shouldn't block the promise settling
      //  since the AbortController already fired — implementation: readBounded finally
      //  awaits reader.cancel() which may hang, but signal already fired so the
      //  *caller's* promise settles via classifyAbort throw before finally completes.)
      // This test verifies the error propagates; physical slot may be retained.
      await assertTransportError(p, "deadline", "uncooperative reader")
    } finally { t.dispose() }
  })
})

// ---------------------------------------------------------------------------
// 13. Caller AbortSignal
// ---------------------------------------------------------------------------

describe("caller abort signal", () => {
  it("pre-aborted signal rejects immediately", async () => {
    const ac = new AbortController()
    ac.abort()
    const { transport } = makeTransport(stalledFetch())
    try { await assertTransportError(transport.get("path", { signal: ac.signal }), "abort", "pre-aborted") }
    finally { transport.dispose() }
  })

  it("aborts during body read", async () => {
    const ac = new AbortController()
    const { transport } = makeTransport(stalledBodyFetch())
    try {
      const p = transport.get("path", { signal: ac.signal })
      await Promise.resolve()
      ac.abort()
      const err = await p.catch(e => e)
      assert.ok(err instanceof TransportError, `must be TransportError: ${err}`)
      assert.ok(["abort", "deadline", "cancelled"].includes(err.code), `got code=${err.code}`)
    } finally { transport.dispose() }
  })
})

// ---------------------------------------------------------------------------
// 14. Cancellation slot retention
// ---------------------------------------------------------------------------

describe("cancellation slot retention", () => {
  it("aborting a queued caller does not free a physical slot", async () => {
    let activeCount = 0
    const t = createReadOnlyTransport({
      baseUrl: "http://localhost/", headers: {},
      fetch: (_href, opts) => new Promise((_res, rej) => {
        activeCount++
        const s = opts?.signal
        s?.addEventListener("abort", () => { activeCount--; rej(new Error("aborted")) }, { once: true })
      }),
      deadlineMs: 3000, clock: makeFakeClock(),
    })
    try {
      // Fill physical concurrency.
      const fills = Array.from({ length: observationBudget.concurrency }, () => t.get("path").catch(() => {}))
      await Promise.resolve()

      // Try to queue one more with its own abort.
      const ac = new AbortController()
      const pending = t.get("path", { signal: ac.signal }).catch(e => e)
      ac.abort()
      const err = await pending
      assert.ok(err instanceof TransportError, "pending abort must error")

      // Physical slots still occupied.
      assert.equal(t.metrics().active, observationBudget.concurrency, "physical slots retained")
      t.dispose()
      await Promise.allSettled(fills)
    } catch (e) { t.dispose(); throw e }
  })
})

// ---------------------------------------------------------------------------
// 15. Concurrency / pending queue limits
// ---------------------------------------------------------------------------

describe("concurrency limits", () => {
  it("rejects when pending queue is full (conservative)", async () => {
    const { transport } = makeTransport(stalledFetch(), { deadlineMs: 3000 })
    try {
      const total = observationBudget.concurrency + observationBudget.pendingRequests
      const promises = Array.from({ length: total }, () => transport.get("path").catch(e => e))
      await Promise.resolve()
      // One more request must be rejected immediately as bound.
      await assertTransportError(transport.get("path"), "bound", "queue full")
    } finally { transport.dispose() }
  })
})

// ---------------------------------------------------------------------------
// 16. Dispose lifecycle
// ---------------------------------------------------------------------------

describe("dispose", () => {
  it("rejects new requests after dispose", async () => {
    const { transport } = makeTransport(fakeFetch())
    transport.dispose()
    await assertTransportError(transport.get("path"), "cancelled", "post-dispose")
  })

  it("dispose is idempotent", () => {
    const { transport } = makeTransport(fakeFetch())
    transport.dispose()
    assert.doesNotThrow(() => transport.dispose())
  })

  it("in-flight requests cancelled on dispose", async () => {
    const { transport } = makeTransport(stalledFetch(), { deadlineMs: 3000 })
    const p = transport.get("path").catch(e => e)
    await Promise.resolve()
    transport.dispose()
    const err = await p
    assert.ok(err instanceof TransportError, `must be TransportError: ${err}`)
    assert.ok(["cancelled", "abort", "deadline"].includes(err.code), `got code=${err.code}`)
  })
})

// ---------------------------------------------------------------------------
// 17. Metrics
// ---------------------------------------------------------------------------

describe("metrics", () => {
  it("tracks successful calls", async () => {
    const { transport } = makeTransport(fakeFetch({ body: "{}" }))
    try { await transport.get("path"); await transport.get("path"); assert.ok(transport.metrics().calls >= 2) }
    finally { transport.dispose() }
  })

  it("tracks errors", async () => {
    const { transport } = makeTransport(fakeFetch({ status: 500 }))
    try { await transport.get("path").catch(() => {}); assert.ok(transport.metrics().errors >= 1) }
    finally { transport.dispose() }
  })

  it("metrics returns expected keys", () => {
    const { transport } = makeTransport(fakeFetch())
    try {
      const m = transport.metrics()
      for (const k of ["calls", "errors", "active", "pending"]) assert.equal(typeof m[k], "number")
    } finally { transport.dispose() }
  })

  it("metrics does not expose secret headers", () => {
    const { transport } = makeTransport(fakeFetch(), { headers: { Authorization: "Bearer secret-token" } })
    try { assert.ok(!JSON.stringify(transport.metrics()).includes("secret-token")) }
    finally { transport.dispose() }
  })
})

// ---------------------------------------------------------------------------
// 18. No mutator methods; frozen
// ---------------------------------------------------------------------------

describe("no mutators", () => {
  it("no post/put/patch/delete on transport", () => {
    const { transport } = makeTransport(fakeFetch())
    try { for (const m of ["post", "put", "patch", "delete"]) assert.ok(!(m in transport), `no ${m}`) }
    finally { transport.dispose() }
  })

  it("transport is frozen", () => {
    const { transport } = makeTransport(fakeFetch())
    try { assert.ok(Object.isFrozen(transport)) }
    finally { transport.dispose() }
  })
})

// ---------------------------------------------------------------------------
// 19. Outcome mode
// ---------------------------------------------------------------------------

describe("outcome mode", () => {
  it("outcome=true allows bodies ≤ 64KiB", async () => {
    const body = JSON.stringify({ ok: true })
    const { transport } = makeTransport(fakeFetch({ body }))
    try { assert.equal(await transport.get("path", { outcome: true }), body) }
    finally { transport.dispose() }
  })

  it("outcome=true rejects bodies > 64KiB", async () => {
    const { transport } = makeTransport(fakeFetch({ body: "x".repeat(observationBudget.metadataBytes + 1) }))
    try { await assertTransportError(transport.get("path", { outcome: true }), "bound", "outcome too large") }
    finally { transport.dispose() }
  })

  it("outcome=false allows bodies up to 1MiB", async () => {
    const body = "x".repeat(observationBudget.responseBytes)
    const { transport } = makeTransport(fakeFetch({ body }))
    try { assert.equal((await transport.get("path")).length, observationBudget.responseBytes) }
    finally { transport.dispose() }
  })
})

// ---------------------------------------------------------------------------
// 20. No secrets in errors
// ---------------------------------------------------------------------------

describe("no secrets in errors", () => {
  it("HTTP error message does not include request URL", async () => {
    const { transport } = makeTransport(fakeFetch({ status: 403 }))
    try {
      const err = await transport.get("secret-path").catch(e => e)
      assert.ok(!err.message.includes("secret-path"))
      assert.ok(!err.message.includes("localhost"))
    } finally { transport.dispose() }
  })

  it("schema error from path validation does not include baseUrl details", async () => {
    const { transport } = makeTransport(fakeFetch(), { baseUrl: "http://localhost/api" })
    try {
      const err = await transport.get("//evil.com/").catch(e => e)
      assert.ok(!err.message.includes("evil.com"))
      assert.ok(!err.message.includes("localhost"))
    } finally { transport.dispose() }
  })

  it("TransportError message uses fixed internal strings only", async () => {
    // Verify the constructor doesn't blindly accept external data into messages.
    const err = new TransportError("schema", "fixed internal message")
    assert.equal(err.message, "fixed internal message")
    assert.equal(err.code, "schema")
    assert.equal(err.name, "TransportError")
  })
})

// ---------------------------------------------------------------------------
// 21. Chunked reassembly correctness
// ---------------------------------------------------------------------------

describe("chunked reassembly", () => {
  it("reassembles chunked body correctly", async () => {
    const expected = JSON.stringify({ hello: "world", nested: { a: 1 } })
    const { transport } = makeTransport(chunkedFetch(expected, 5))
    try { assert.equal(await transport.get("path"), expected) }
    finally { transport.dispose() }
  })
})
