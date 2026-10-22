// bounded-transport.mjs — Read-only GET transport for OpenCode V2 P0.
// Prototype only. NOT a production claim. Production gate remains blocked (CONTRACT.md).
//
// Public API
//   createReadOnlyTransport({ baseUrl, headers, fetch, deadlineMs=3000, clock })
//     baseUrl:    absolute http/https, no userinfo/hash/query, string ≤4096 chars.
//     headers:    plain object, own string keys/values, ≤32 entries, ≤8 KiB total.
//     fetch:      injected; never uses globalThis.fetch.
//     deadlineMs: ≤3000 max, default 3000; must be a positive finite number ≤3000.
//     clock:      { now, setTimeout, clearTimeout } for testing.
//   .get(relativePath, { outcome=false, query, signal }) → Promise<string>
//     relativePath: relative, no leading slash, no dot/dotdot, no encoded dot/backslash,
//                   no query/hash/userinfo; appended to base path preserving prefix.
//     query:      optional plain object or URLSearchParams; keys/values must be
//                 strings or finite numbers; ≤32 pairs; no arbitrary syntax.
//     outcome:    true → 64 KiB; false → 1 MiB.
//     signal:     optional caller AbortSignal.
//     Deadline starts when .get() is called; fires even if queued or fetch ignores abort.
//     Physical slot not freed until actual invoke/reader-cancel settles.
//     Returns bounded UTF-8 string before JSON.parse. Rejects with TransportError.
//   .dispose() — idempotent; cancels queued and in-flight requests.
//   .metrics()  → { calls, errors, active, pending }
//
// TransportError.code values:
//   bound | schema | abort | deadline | http | redirect | cancelled | utf8

import { observationBudget, RequestBudget } from "../../integrations/opencode/core.mjs"

export class TransportError extends Error {
  constructor(code, message) {
    super(message)
    this.name = "TransportError"
    this.code = code
  }
}

// ---------------------------------------------------------------------------
// Input validation
// ---------------------------------------------------------------------------

const ALLOWED_PROTOCOLS = new Set(["http:", "https:"])
const MAX_BASE_URL_LEN = 4096
const MAX_HEADER_ENTRIES = 32
const MAX_HEADERS_TOTAL_BYTES = 8192
const MAX_DEADLINE_MS = 3000
const MAX_QUERY_PAIRS = 32

function validateBaseUrl(raw) {
  if (typeof raw !== "string") throw new TransportError("schema", "baseUrl must be a string")
  if (raw.length > MAX_BASE_URL_LEN || /[\u0000-\u0020\u007f?#]/.test(raw))
    throw new TransportError("schema", "baseUrl too long or contains controls")
  let url
  try { url = new URL(raw) } catch { throw new TransportError("schema", "baseUrl is not a valid URL") }
  if (!ALLOWED_PROTOCOLS.has(url.protocol)) throw new TransportError("schema", "baseUrl uses unsupported protocol")
  if (url.username || url.password) throw new TransportError("schema", "baseUrl must not contain userinfo")
  if (url.hash) throw new TransportError("schema", "baseUrl must not contain a hash")
  if (url.search) throw new TransportError("schema", "baseUrl must not contain a query string")
  return url
}

function validateHeaders(raw) {
  if (raw === null || typeof raw !== "object" || ![null, Object.prototype].includes(Object.getPrototypeOf(raw)))
    throw new TransportError("schema", "headers must be a plain object")
  const entries = Object.keys(raw)
  if (entries.length > MAX_HEADER_ENTRIES)
    throw new TransportError("schema", "headers exceeds maximum entry count")
  let total = 0
  for (const k of entries) {
    if (!Object.prototype.hasOwnProperty.call(raw, k))
      throw new TransportError("schema", "headers must have only own properties")
    const descriptor = Object.getOwnPropertyDescriptor(raw, k)
    if (!descriptor || !Object.hasOwn(descriptor, "value"))
      throw new TransportError("schema", "headers must not contain accessors")
    const v = descriptor.value
    if (typeof k !== "string" || typeof v !== "string" || /[\r\n\u0000]/.test(k + v))
      throw new TransportError("schema", "header keys and values must be strings")
    total += new TextEncoder().encode(k + v).byteLength
    if (total > MAX_HEADERS_TOTAL_BYTES)
      throw new TransportError("schema", "headers total size exceeds limit")
  }
}

// Build query string from validated plain map or URLSearchParams.
// Keys and values must be strings or finite numbers; ≤MAX_QUERY_PAIRS entries.
function buildQuery(query) {
  if (query == null) return ""
  let pairs
  if (query instanceof URLSearchParams) {
    pairs = []
    for (const pair of query) {
      if (pairs.length >= MAX_QUERY_PAIRS)
        throw new TransportError("schema", "query exceeds maximum pair count")
      pairs.push(pair)
    }
  } else if (typeof query === "object" && [null, Object.prototype].includes(Object.getPrototypeOf(query))) {
    pairs = Object.keys(query).map(key => {
      const descriptor = Object.getOwnPropertyDescriptor(query, key)
      if (!descriptor || !Object.hasOwn(descriptor, "value"))
        throw new TransportError("schema", "query must not contain accessors")
      return [key, descriptor.value]
    })
  } else {
    throw new TransportError("schema", "query must be a plain object or URLSearchParams")
  }
  if (pairs.length > MAX_QUERY_PAIRS)
    throw new TransportError("schema", "query exceeds maximum pair count")
  const sp = new URLSearchParams()
  for (const [k, v] of pairs) {
    if (typeof k !== "string")
      throw new TransportError("schema", "query keys must be strings")
    if (typeof v !== "string" && !(typeof v === "number" && Number.isFinite(v)))
      throw new TransportError("schema", "query values must be strings or finite numbers")
    if (k.length > 256 || String(v).length > 4096)
      throw new TransportError("schema", "query entry exceeds length bound")
    sp.append(k, String(v))
  }
  const s = sp.toString()
  if (s.length > 16384) throw new TransportError("schema", "query exceeds byte bound")
  return s ? "?" + s : ""
}

// ---------------------------------------------------------------------------
// Path validation — strict prefix-preserving
// ---------------------------------------------------------------------------

// Reject any path that would escape or alter the base pathname prefix.
// Rules:
//   - Must not start with "/" (would replace entire path, not append).
//   - Must not start with "./" or be "." (current-dir relative is fine only as first segment check).
//   - Must not contain ".." segments (encoded or raw), encoded dots (%2E), or backslashes (%5C/\).
//   - Must not contain "://" (absolute URL), "//" (protocol-relative), or "@", "?", "#".
//   - After URL resolution, joined pathname must start with the base pathname prefix.
function validateRelativePath(baseUrl, relativePath) {
  if (typeof relativePath !== "string" || relativePath.length === 0 || relativePath.length > 4096 ||
      /[\u0000-\u0020\u007f]/.test(relativePath) || /%(2f|25)/i.test(relativePath))
    throw new TransportError("schema", "relativePath must be a non-empty string")

  // Reject leading slash (would navigate to root, bypassing prefix).
  if (relativePath.startsWith("/"))
    throw new TransportError("schema", "relativePath must not start with a slash")

  // Reject protocol-relative.
  if (relativePath.startsWith("//"))
    throw new TransportError("schema", "relativePath must not be protocol-relative")

  // Reject absolute URL scheme.
  if (/^[A-Za-z][A-Za-z0-9+\-.]*:\/\//.test(relativePath))
    throw new TransportError("schema", "relativePath must not be an absolute URL")

  // Reject forbidden characters: @, #, ?
  if (relativePath.includes("@") || relativePath.includes("#") || relativePath.includes("?"))
    throw new TransportError("schema", "relativePath must not contain userinfo, hash, or query")

  // Reject backslash (literal or encoded: %5C, %5c).
  if (relativePath.includes("\\") || /%(5[Cc])/i.test(relativePath))
    throw new TransportError("schema", "relativePath must not contain backslash")

  // Reject encoded dots that could form dot-segments after normalization.
  // %2E is ".", so %2E%2E or .%2E or %2E. are all ".."
  if (/(%2[Ee])/i.test(relativePath))
    throw new TransportError("schema", "relativePath must not contain encoded dots")

  // Reject dot-segments: ".." or "." as path segments (before or after slash).
  // Split on "/" and check each segment.
  const segments = relativePath.split("/")
  for (const seg of segments) {
    if (seg === "." || seg === "..")
      throw new TransportError("schema", "relativePath must not contain dot-segments")
  }

  // Resolve and verify prefix is preserved.
  let joined
  try {
    const base = baseUrl.href.endsWith("/") ? baseUrl.href : baseUrl.href + "/"
    joined = new URL(relativePath, base)
  } catch {
    throw new TransportError("schema", "relativePath cannot be resolved")
  }

  // Origin must not change.
  if (joined.origin !== baseUrl.origin)
    throw new TransportError("schema", "relativePath must not change the authority")

  // Pathname must start with base's pathname (prefix preserved).
  const basePathname = baseUrl.pathname.endsWith("/") ? baseUrl.pathname : baseUrl.pathname + "/"
  if (!joined.pathname.startsWith(basePathname))
    throw new TransportError("schema", "relativePath must not escape the base pathname prefix")

  // No hash/search (query appended separately via buildQuery).
  if (joined.hash || joined.search)
    throw new TransportError("schema", "Resolved URL contains hash or query")

  return joined
}

// ---------------------------------------------------------------------------
// Content-Type validation — media-type match, not substring
// ---------------------------------------------------------------------------

function isJsonContentType(header) {
  if (!header) return false
  // Parse media type: "type/subtype[; params...]"
  const mediaType = header.split(";")[0].trim().toLowerCase()
  // Accept application/json or any +json structured syntax suffix (RFC 6839).
  return mediaType === "application/json" || /^application\/[a-z0-9!#$&^_.+-]+\+json$/.test(mediaType)
}

// ---------------------------------------------------------------------------
// Abort classification
// Priority: deadline > disposal (cancelled) > caller (abort)
// ---------------------------------------------------------------------------

function classifyAbort(deadlineSignal, disposalSignal) {
  if (deadlineSignal?.aborted) return new TransportError("deadline", "Request deadline exceeded")
  if (disposalSignal?.aborted) return new TransportError("cancelled", "Transport disposed")
  return new TransportError("abort", "Request aborted by caller")
}

// ---------------------------------------------------------------------------
// Bounded body reader
// ---------------------------------------------------------------------------

// One root/value allowance plus container starts and separators conservatively
// bounds nodes before parsing. A flat array of N scalar values counts N + 1;
// object/nested-container accounting may over-count, never under-count.
async function readBounded(response, combinedSignal, deadlineSignal, disposalSignal, outcome) {
  const limit = outcome ? observationBudget.metadataBytes : observationBudget.responseBytes

  const advertised = response.headers.get("content-length")
  if (advertised !== null) {
    const n = Number(advertised)
    if (Number.isFinite(n) && n > limit) {
      // Caller deadline is independent; keep the physical slot until cancelled.
      await response.body?.cancel().catch(() => {})
      throw new TransportError("bound", "Response Content-Length exceeds byte limit")
    }
  }

  if (!response.body) throw new TransportError("schema", "Response has no body")

  const reader = response.body.getReader()
  let cancellation
  const cancel = () => cancellation ??= Promise.resolve().then(() => reader.cancel()).catch(() => {})
  const abortHandler = () => { void cancel() }
  combinedSignal?.addEventListener("abort", abortHandler, { once: true })

  try {
    if (combinedSignal?.aborted) throw classifyAbort(deadlineSignal, disposalSignal)

    const buffer = new Uint8Array(limit)
    let size = 0, depth = 0, nodes = 1, quoted = false, escaped = false

    for (;;) {
      const { done, value } = await reader.read()
      if (done) break

      if (size + value.byteLength > limit)
        throw new TransportError("bound", "Response body exceeds byte limit")

      for (const byte of value) {
        if (quoted) {
          if (escaped) { escaped = false }
          else if (byte === 0x5c) { escaped = true }
          else if (byte === 0x22) { quoted = false }
        } else if (byte === 0x22) {
          quoted = true
        } else if (byte === 0x7b || byte === 0x5b) {
          ++nodes
          if (++depth > observationBudget.jsonDepth)
            throw new TransportError("bound", "Response body exceeds JSON depth limit")
          if (nodes > observationBudget.jsonNodes)
            throw new TransportError("bound", "Response body exceeds JSON node limit")
        } else if (byte === 0x7d || byte === 0x5d) {
          depth--
        } else if ((byte === 0x2c || byte === 0x3a) && depth > 0) {
          // Count commas and colons as separators between values (conservative overcount).
          if (++nodes > observationBudget.jsonNodes)
            throw new TransportError("bound", "Response body exceeds JSON node limit")
        }
      }

      buffer.set(value, size)
      size += value.byteLength

      if (combinedSignal?.aborted) throw classifyAbort(deadlineSignal, disposalSignal)
    }

    if (combinedSignal?.aborted) throw classifyAbort(deadlineSignal, disposalSignal)

    try {
      return new TextDecoder("utf-8", { fatal: true }).decode(buffer.subarray(0, size))
    } catch {
      throw new TransportError("utf8", "Response body is not valid UTF-8")
    }
  } catch (error) {
    if (error instanceof TransportError) throw error
    if (combinedSignal?.aborted) throw classifyAbort(deadlineSignal, disposalSignal)
    throw new TransportError("schema", "Unexpected error reading response body")
  } finally {
    combinedSignal?.removeEventListener("abort", abortHandler)
    // Await cancel so physical reader is released; deadline already fired via signal.
    await cancel()
    reader.releaseLock()
  }
}

// ---------------------------------------------------------------------------
// createReadOnlyTransport
// ---------------------------------------------------------------------------

export function createReadOnlyTransport({
  baseUrl,
  headers,
  fetch: fetchImpl,
  deadlineMs = MAX_DEADLINE_MS,
  clock,
}) {
  if (typeof fetchImpl !== "function")
    throw new TransportError("schema", "fetch must be a function")
  if (typeof deadlineMs !== "number" || !Number.isFinite(deadlineMs) || deadlineMs <= 0 || deadlineMs > MAX_DEADLINE_MS)
    throw new TransportError("schema", "deadlineMs must be a positive finite number ≤3000")

  validateHeaders(headers)                    // throws TransportError
  const validatedBase = validateBaseUrl(baseUrl) // throws TransportError

  // Private immutable copy of headers; never exposed to callers or error messages.
  const frozenHeaders = Object.freeze(Object.assign(Object.create(null), headers))

  const _clock = clock ?? {
    now: () => performance.now(),
    setTimeout: (fn, ms) => setTimeout(fn, ms),
    clearTimeout: (t) => clearTimeout(t),
  }

  const budget = new RequestBudget(_clock)

  let _calls = 0, _errors = 0
  let disposed = false

  // Wrap plain Errors from RequestBudget into TransportError.
  function wrapBudgetError(err, deadlineSignal) {
    if (err instanceof TransportError) return err
    const msg = err?.message ?? ""
    if (/queue full/i.test(msg)) return new TransportError("bound", "Request pending queue full")
    // Cancelled (disposal or caller abort).
    if (deadlineSignal?.aborted) return new TransportError("deadline", "Request deadline exceeded")
    if (budget.disposal.signal.aborted) return new TransportError("cancelled", "Transport disposed")
    return new TransportError("abort", "Request aborted")
  }

  // executeGet: runs inside budget.call; budgetSignal = disposal+caller combined by RequestBudget.
  async function executeGet(href, outcome, callerSignal, deadlineSignal, budgetSignal) {
    // Combined: deadline + budgetSignal (which includes disposal + caller).
    const combined = AbortSignal.any([deadlineSignal, budgetSignal])

    _calls++
    try {
      let response
      try {
        response = await fetchImpl(href, {
          method: "GET",
          headers: frozenHeaders,
          redirect: "error",
          signal: combined,
        })
      } catch (err) {
        if (combined.aborted) throw classifyAbort(deadlineSignal, budget.disposal.signal)
        const msg = err?.message ?? ""
        if (/redirect/i.test(msg)) throw new TransportError("redirect", "Server redirected")
        throw new TransportError("schema", "Fetch failed")
      }

      // If combined aborted after fetch returned (uncooperative fetch): discard body.
      if (combined.aborted) {
        await response.body?.cancel().catch(() => {})
        throw classifyAbort(deadlineSignal, budget.disposal.signal)
      }

      if (response.status >= 300 && response.status < 400) {
        await response.body?.cancel().catch(() => {})
        throw new TransportError("redirect", "Server issued a redirect response")
      }

      if (!response.ok) {
        await response.body?.cancel().catch(() => {})
        throw new TransportError("http", `HTTP ${response.status}`)
      }

      const ct = response.headers.get("content-type") ?? ""
      if (!isJsonContentType(ct)) {
        await response.body?.cancel().catch(() => {})
        throw new TransportError("schema", "Response is not application/json")
      }

      return await readBounded(response, combined, deadlineSignal, budget.disposal.signal, outcome)
    } catch (err) {
      _errors++
      if (err instanceof TransportError) throw err
      if (combined.aborted) throw classifyAbort(deadlineSignal, budget.disposal.signal)
      throw new TransportError("schema", "Unexpected transport error")
    }
  }

  // .get — deadline starts here, outside budget.call, so queued requests are also bounded.
  function get(relativePath, { outcome = false, query, signal } = {}) {
    if (disposed || budget.disposal.signal.aborted)
      return Promise.reject(new TransportError("cancelled", "Transport disposed"))
    if (typeof outcome !== "boolean" || (signal !== undefined && !(signal instanceof AbortSignal)))
      return Promise.reject(new TransportError("schema", "Invalid request options"))

    // Validate path and build query string synchronously — fail fast.
    let joinedUrl
    try {
      joinedUrl = validateRelativePath(validatedBase, relativePath)
    } catch (err) {
      return Promise.reject(err)
    }
    let queryStr
    try {
      queryStr = buildQuery(query)
    } catch (err) {
      return Promise.reject(err)
    }
    const href = joinedUrl.href + queryStr

    // Deadline controller: armed now, before entering budget queue.
    const deadlineController = new AbortController()
    const deadlineTimer = _clock.setTimeout(() => deadlineController.abort(), deadlineMs)

    // callerSignal + deadlineSignal combined for budget.call's signal arg.
    const signals = [deadlineController.signal]
    if (signal) signals.push(signal)
    const callerDeadline = signals.length > 1 ? AbortSignal.any(signals) : deadlineController.signal

    return budget.call(
      (budgetSignal) => executeGet(href, outcome, signal ?? null, deadlineController.signal, budgetSignal),
      callerDeadline,  // budget sees deadline+caller; queue abort fires on deadline too
    ).catch(err => {
      throw wrapBudgetError(err, deadlineController.signal)
    }).finally(() => {
      _clock.clearTimeout(deadlineTimer)
    })
  }

  function dispose() {
    if (disposed) return
    disposed = true
    budget.dispose()
  }

  // No secret data in metrics.
  function metrics() {
    return { calls: _calls, errors: _errors, active: budget.active, pending: budget.waiting.length }
  }

  return Object.freeze({ get, dispose, metrics })
}
