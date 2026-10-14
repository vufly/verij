// Metadata projection for the OpenCode 1.x TUI contract.
// No prompts, message text, tool arguments/output or error bodies leave this module.
export function supportsVersion(version) {
  return typeof version === "string" && /^1\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.+-]+)?$/.test(version)
}
export const events = ["session.created", "session.updated", "session.deleted", "session.status",
  "session.idle", "session.error", "message.updated", "message.removed", "message.part.updated",
  "permission.asked", "permission.replied", "question.asked", "question.replied", "question.rejected",
  "server.instance.disposed", "server.connected"]

export function unknown(version) {
  return { schema_version: 1, version, activity: "unknown", pending_requests: [], background_task_count: 0 }
}

export function normalize(version, family) {
  const result = unknown(version)
  if (!family) return { ...result, activity: "idle" }
  result.conversation_id = family.root.id
  result.conversation_title = [...(family.root.title ?? "")].filter(c => c >= " " && c !== "\x7f").slice(0, 256).join("")
  const members = new Set(family.members.map(s => s.id))
  // The queried root must actually be a member; unrelated shared-server facts
  // never establish ownership, pending input or background work.
  if (!members.has(family.root.id)) return result
  result.pending_requests = [
    ...family.permissions.filter(r => members.has(r.sessionID)).map(r => ({ id: r.id, kind: "permission" })),
    ...family.questions.filter(r => members.has(r.sessionID)).map(r => ({ id: r.id, kind: "question" })),
  ]
  const busy = id => ["busy", "retry"].includes(family.statuses[id]?.type)
  result.background_task_count = family.members.filter(s => s.id !== family.root.id && busy(s.id)).length
  const messages = family.messages
  const userIndex = messages.findLastIndex(m => m.info.role === "user")
  const user = messages[userIndex]?.info
  if (user) result.turn_id = `${family.root.id}/${user.id}`
  const assistant = messages.slice(userIndex + 1).findLast(m => m.info.role === "assistant" && m.info.parentID === user?.id)?.info
  if (busy(family.root.id) || result.background_task_count) {
    result.activity = family.statuses[family.root.id]?.type === "retry" ? "retrying" : "working"
    return result
  }
  // Busy and explicit input remain observable while outcome recovery is pending.
  // Neither an unhydrated local cache nor missing history establishes idle/Done.
  if (family.messagesComplete === false) return result
  result.activity = "idle"
  if (assistant?.error && assistant.time?.completed && !result.pending_requests.length) {
    result.error_name = assistant.error.name ?? "UnknownError"
  } else if (user && assistant?.time?.completed && ["stop", "end_turn"].includes(assistant.finish)
      && !assistant.error && !result.pending_requests.length) {
    result.completed_at_ms = assistant.time.completed
  } else if (user && (!assistant || !assistant.time?.completed) && !result.pending_requests.length) {
    result.activity = "unknown"
  }
  return result
}

function data(response) {
  if (response?.error || response?.data === undefined) throw new Error("Incomplete observation")
  return response.data
}

export const validObservationID = id => typeof id === "string" && id.length > 0 && id.length <= 256
  && Buffer.byteLength(id) <= 256 && !/[\x00-\x1f\x7f-\x9f]/.test(id)
const validID = validObservationID
const sessionMetadata = info => {
  if (!validID(info?.id) || (info.parentID !== undefined && !validID(info.parentID))) throw Error("Invalid ancestry")
  let title = "", count = 0
  for (const character of typeof info.title === "string" ? info.title : "") {
    if (count++ >= 256) break
    title += character
  }
  return { id: info.id, parentID: info.parentID, title }
}
const messageMetadata = info => {
  if (!validID(info?.id) || !["user", "assistant"].includes(info.role)
      || (info.role === "assistant" && !validID(info.parentID))
      || (info.time?.completed !== undefined && (!Number.isSafeInteger(info.time.completed) || info.time.completed <= 0))) {
    throw Error("Invalid message metadata")
  }
  return { info: { id: info.id, role: info.role, parentID: info.role === "assistant" ? info.parentID : undefined,
    sessionID: info.sessionID,
    finish: typeof info.finish === "string" ? info.finish.slice(0, 256) : undefined,
    time: { created: info.time?.created, completed: info.time?.completed },
    error: info.error ? { name: validID(info.error.name) ? info.error.name : "UnknownError" } : undefined } }
}

// The SDK's documented requestValidator hook receives the resolved fetch function,
// including worker/attach routing and authentication. Wrap only this request; never
// replace shared SDK configuration or use a different server/transport.
export function boundedReadOptions(options = {}, prefix = false, array = false, worker) {
  return { ...options, requestValidator: async resolved => {
    await options.requestValidator?.(resolved)
    const fetch = resolved.fetch
    resolved.fetch = async request => {
      // Stock standalone RPC buffers body text in the worker before returning.
      // Never issue a message-body RPC there: use verified read-only metadata,
      // or degrade conservatively if the storage contract cannot be established.
      if (prefix && new URL(request.url).origin === "http://opencode.internal") {
        if (!worker) throw Error("Worker metadata unavailable")
        return new Response(JSON.stringify(await worker.page(request)), {
          headers: { "content-type": "application/json" },
        })
      }
      const response = await fetch(request)
      if (!response.ok) {
        await response.body?.cancel().catch(() => {})
        throw Error(`Observation HTTP ${response.status}`) // Never parse error bodies.
      }
      if (!response.headers.get("content-type")?.toLowerCase().includes("application/json")) {
        await response.body?.cancel().catch(() => {})
        throw Error("Invalid observation content type")
      }
      if (prefix) {
        const value = await readMessagePrefix(response, array, request.signal)
        const headers = new Headers(response.headers)
        headers.delete("content-length"); headers.delete("content-encoding")
        return new Response(JSON.stringify(value), { status: 200, headers })
      }
      const value = await readBoundedJSON(response, request.signal)
      const headers = new Headers(response.headers)
      headers.delete("content-length"); headers.delete("content-encoding")
      return new Response(value, { status: 200, headers })
    }
  } }
}

// This optimization is exclusively for the stock standalone worker, whose env is
// copied from this TUI process. Attach transports always use the real target API.
// Open the producer's existing DB read-only; never create/migrate/write it. Each
// indexed page is a fresh SQLite snapshot. Parts are not selected; oversized info
// is rejected in SQL before JSON extraction or allocation in the JS adapter.
export class WorkerMetadata {
  constructor(api) { this.api = api; this.closed = false }
  async open() {
    if (this.closed) throw Error("Worker metadata disposed")
    if (this.db) return
    const [{ default: path }, { default: os }, fs] = await Promise.all([
      import("node:path"), import("node:os"), import("node:fs"),
    ])
    const env = process.env, home = os.homedir()
    const xdg = (name, fallback) => env[name] && path.isAbsolute(env[name]) ? env[name] : path.join(home, fallback)
    const dataDir = path.join(xdg("XDG_DATA_HOME", ".local/share"), "opencode")
    const stateDir = path.join(xdg("XDG_STATE_HOME", ".local/state"), "opencode")
    const configDir = env.OPENCODE_CONFIG_DIR ?? path.join(xdg("XDG_CONFIG_HOME", ".config"), "opencode")
    const observed = this.api.state.path
    if (!observed || path.resolve(observed.state ?? "") !== path.resolve(stateDir)
        || path.resolve(observed.config ?? "") !== path.resolve(configDir)
        || path.resolve(observed.directory ?? "") !== process.cwd()) throw Error("Unverified worker storage scope")
    if (env.OPENCODE_DB === ":memory:") throw Error("In-memory worker metadata unavailable")
    if (!env.OPENCODE_DB && !/^1\.[0-9]+\.[0-9]+(?:\+.*)?$/.test(this.api.app?.version ?? "")
        && !["1", "true"].includes(env.OPENCODE_DISABLE_CHANNEL_DB)) throw Error("Unverified worker database channel")
    const filename = env.OPENCODE_DB
      ? path.resolve(dataDir, env.OPENCODE_DB) : path.join(dataDir, "opencode.db")
    const stat = fs.statSync(filename)
    if (!stat.isFile() || stat.uid !== process.getuid()) throw Error("Unverified worker database")
    this.stat = () => fs.statSync(filename)
    this.identity = `${stat.dev}/${stat.ino}`
    const sqlite = globalThis.Bun ? await import("bun:sqlite") : await import("node:sqlite")
    if (this.closed) throw Error("Worker metadata disposed")
    const db = globalThis.Bun ? new sqlite.Database(filename, { readonly: true, create: false, strict: true })
      : new sqlite.DatabaseSync(filename, { readOnly: true, timeout: 0 })
    try {
      db.exec("PRAGMA query_only = ON; PRAGMA busy_timeout = 0; PRAGMA cache_size = -512; PRAGMA mmap_size = 0")
      const prepare = sql => globalThis.Bun ? db.query(sql) : db.prepare(sql)
      this.schema = prepare("PRAGMA schema_version")
      this.schemaVersion = this.schema.get().schema_version
      const columns = prepare("PRAGMA table_info(message)").all().map(row => row.name)
      if (!["id", "session_id", "time_created", "data"].every(name => columns.includes(name))) throw Error("Unsupported worker schema")
      // Avoid an unindexed scan if a changed schema removed the stock index.
      const indices = prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'message' LIMIT 17").all()
      if (indices.length > 16) throw Error("Worker schema exceeds index bound")
      if (!indices.some(row => {
        const name = String(row.name).replaceAll('"', '""')
        const fields = prepare(`PRAGMA index_info("${name}")`).all().map(field => field.name)
        return fields.slice(0, 3).join("/") === "session_id/time_created/id"
      })) throw Error("Unindexed worker metadata")
      if (!prepare("PRAGMA table_info(session)").all().some(column => column.name === "id" && column.pk === 1)) {
        throw Error("Unindexed worker session identity")
      }
      this.session = prepare("SELECT id FROM session WHERE id = ? LIMIT 1")
      // octet_length(column) reads SQLite's stored length without materializing
      // oversized text. CASE short-circuits before any JSON extraction.
      const projection = `CASE WHEN octet_length(data) <= ${observationBudget.metadataBytes}
        THEN json_object('id', id, 'sessionID', session_id, 'role', json_extract(data, '$.role'),
          'parentID', json_extract(data, '$.parentID'), 'finish', json_extract(data, '$.finish'),
          'time', json_object('created', json_extract(data, '$.time.created'), 'completed', json_extract(data, '$.time.completed')),
          'error', CASE WHEN json_type(data, '$.error') IS NOT NULL AND json_type(data, '$.error') != 'null'
            THEN json_object('name', json_extract(data, '$.error.name')) ELSE NULL END)
        ELSE NULL END AS info`
      this.head = prepare(`SELECT id, time_created, ${projection} FROM message WHERE session_id = ?
        ORDER BY time_created DESC, id DESC LIMIT 1`)
      this.before = prepare(`SELECT id, time_created, ${projection} FROM message WHERE session_id = ?
        AND (time_created, id) < (?, ?) ORDER BY time_created DESC, id DESC LIMIT 1`)
      this.db = db
    } catch (error) { db.close(); throw error }
  }
  async page(request) {
    await this.open()
    if (this.closed || request.signal.aborted) throw Error("Worker metadata cancelled")
    const stat = this.stat()
    if (`${stat.dev}/${stat.ino}` !== this.identity) { this.close(); throw Error("Worker database replaced") }
    if (this.schema.get().schema_version !== this.schemaVersion) { this.close(); throw Error("Worker schema changed") }
    const url = new URL(request.url)
    const match = url.pathname.match(/^\/session\/([^/]+)\/message$/)
    const root = match && decodeURIComponent(match[1])
    if (!validID(root) || url.searchParams.get("limit") !== "1") throw Error("Invalid worker metadata request")
    if (!this.session.get(root)) throw Error("Worker session absent from verified database")
    const cursor = url.searchParams.get("before")
    let row
    if (cursor) {
      if (cursor.length > 1024) throw Error("Invalid worker cursor")
      const decoded = JSON.parse(Buffer.from(cursor, "base64url").toString("utf8"))
      if (!validID(decoded.id) || !Number.isSafeInteger(decoded.time) || decoded.time < 0) throw Error("Invalid worker cursor")
      row = this.before.get(root, decoded.time, decoded.id)
    } else row = this.head.get(root)
    if (!row) return []
    if (!row.info || Buffer.byteLength(row.info) > observationBudget.metadataBytes) throw Error("Worker metadata exceeds byte bound")
    const info = JSON.parse(row.info)
    if (info.time.completed === null) delete info.time.completed
    if (info.time.created !== row.time_created) throw Error("Inconsistent worker message time")
    return [{ ...messageMetadata(info), cursor: Buffer.from(JSON.stringify({ id: row.id, time: row.time_created })).toString("base64url") }]
  }
  close() {
    this.db?.close(); this.db = undefined; this.head = undefined; this.before = undefined
    this.session = undefined; this.schema = undefined
  }
  dispose() { this.closed = true; this.close() }
}

async function withReader(response, signal, read) {
  if (!response.body) throw Error("Missing observation body")
  const reader = response.body.getReader()
  const abort = () => { void reader.cancel().catch(() => {}) }
  signal?.addEventListener("abort", abort, { once: true })
  try {
    if (signal?.aborted) throw Error("Observation cancelled")
    const result = await read(reader)
    if (signal?.aborted) throw Error("Observation cancelled")
    return result
  } finally {
    signal?.removeEventListener("abort", abort)
    await reader.cancel().catch(() => {})
    reader.releaseLock()
  }
}

// Check every chunk before copying or decoding. The transport owns its delivered
// chunk (and may buffer upstream); adapter buffers and JSON parsing are capped.
export function readBoundedJSON(response, signal) {
  const limit = observationBudget.responseBytes
  const advertised = response.headers.get("content-length")
  if (advertised && Number(advertised) > limit) {
    void response.body?.cancel().catch(() => {})
    return Promise.reject(Error("Observation response exceeds byte bound"))
  }
  return withReader(response, signal, async reader => {
    const buffer = new Uint8Array(limit)
    let size = 0, depth = 0, nodes = 0, quoted = false, escaped = false
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      if (size + value.byteLength > limit) throw Error("Observation response exceeds byte bound")
      for (const byte of value) {
        if (quoted) {
          if (escaped) escaped = false
          else if (byte === 92) escaped = true
          else if (byte === 34) quoted = false
        } else if (byte === 34) quoted = true
        else if (byte === 123 || byte === 91) {
          if (++depth > observationBudget.jsonDepth) throw Error("Observation response exceeds depth bound")
          if (++nodes > observationBudget.jsonNodes) throw Error("Observation response exceeds node bound")
        } else if (byte === 125 || byte === 93) depth--
        else if ((byte === 44 || byte === 58) && ++nodes > observationBudget.jsonNodes) throw Error("Observation response exceeds node bound")
      }
      buffer.set(value, size); size += value.byteLength
    }
    return new TextDecoder("utf-8", { fatal: true }).decode(buffer.subarray(0, size))
  })
}

// Stock WithParts serialization places `info` first. Read one metadata object and
// cancel before parts; never scan, concatenate, decode or JSON.parse tool bodies.
// Changed serialization is unsupported evidence, not permission to read everything.
export function readMessagePrefix(response, array, signal) {
  return withReader(response, signal, async reader => {
    const buffer = new Uint8Array(observationBudget.metadataBytes)
    const prefix = array ? '[{"info":' : '{"info":'
    let prefixIndex = 0, scanned = 0, size = 0, depth = 0, quoted = false, escaped = false, started = false
    for (;;) {
      const { done, value } = await reader.read()
      if (done) throw Error("Incomplete message metadata")
      for (const byte of value) {
        if (++scanned > buffer.length) throw Error("Message metadata exceeds byte bound")
        if (!started) {
          if ([9, 10, 13, 32].includes(byte) && (array ? [0, 1, 2, 8, 9] : [0, 1, 7, 8]).includes(prefixIndex)) continue
          if (array && prefixIndex === 1 && byte === 93) return []
          if (prefixIndex < prefix.length) {
            if (byte !== prefix.charCodeAt(prefixIndex++)) throw Error("Unsupported message serialization")
            continue
          }
          if (byte !== 123) throw Error("Invalid message metadata")
          started = true
        }
        if (size >= buffer.length) throw Error("Message metadata exceeds byte bound")
        buffer[size++] = byte
        if (quoted) {
          if (escaped) escaped = false
          else if (byte === 92) escaped = true
          else if (byte === 34) quoted = false
          continue
        }
        if (byte === 34) quoted = true
        else if (byte === 123 || byte === 91) {
          if (++depth > observationBudget.jsonDepth) throw Error("Message metadata exceeds depth bound")
        } else if (byte === 125 || byte === 93) {
          if (--depth === 0) {
            const info = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(buffer.subarray(0, size)))
            const metadata = messageMetadata(info)
            return array ? [metadata] : metadata
          }
          if (depth < 0) throw Error("Invalid message metadata")
        }
      }
    }
  })
}

const fingerprint = value => JSON.stringify(value)
const messageAfter = (left, right) => left.time.created > right.time.created
  || (left.time.created === right.time.created && left.id > right.id)

// Authoritative narrow suffix, never an empty/unhydrated local cache. Retain only
// head/current-user/latest-assistant metadata and one cursor, not conversation history.
export class OutcomeRecovery {
  constructor() { this.reset() }
  reset() { this.scan = undefined }
  async sample(api, root, options) {
    const client = api.client
    const page = async before => {
      const response = await client.session.messages({ sessionID: root, limit: 1, ...(before ? { before } : {}) }, options)
      const items = data(response)
      if (!Array.isArray(items) || items.length > 1) throw Error("Invalid metadata page")
      const info = items[0] ? messageMetadata(items[0].info).info : undefined
      if (info && (info.sessionID !== root || !Number.isSafeInteger(info.time.created) || info.time.created < 0
          || (info.role === "user" && !validID(`${root}/${info.id}`)))) throw Error("Foreign or invalid message metadata")
      const cursor = response.response?.headers?.get("x-next-cursor") ?? items[0]?.cursor
      if (cursor && (cursor.length > 1024 || /[\x00-\x20\x7f]/.test(cursor))) throw Error("Invalid history cursor")
      return { info, cursor }
    }
    const head = await page()
    if (!head.info) { this.reset(); return [] }
    const headKey = fingerprint(head.info)
    // An unchanged head does not exclude a backdated inserted user or deleted
    // predecessor. Reverify the suffix; cached/local metadata never skips that proof.
    if (this.scan?.head !== headKey) {
      this.scan = { head: headKey, info: head.info, cursor: head.cursor, count: 1,
        assistant: head.info.role === "assistant" ? head.info : undefined, previous: head.info }
    }
    const scan = this.scan
    let user = scan.info.role === "user" ? scan.info : undefined
    // Includes the head query and final head recheck; at most six history calls
    // per collection. Slow scans retain their cursor for the next scheduled slice.
    for (let calls = 1; !user && calls < observationBudget.historySliceCalls - 1; calls++) {
      if (!scan.cursor) throw Error("Current user unavailable in metadata suffix")
      if (scan.count >= observationBudget.historyMessages) throw Error("Current turn exceeds metadata scan bound")
      const next = await page(scan.cursor)
      if (!next.info || !messageAfter(scan.previous, next.info)) throw Error("Inconsistent metadata suffix")
      scan.previous = next.info; scan.cursor = next.cursor; scan.count++
      if (next.info.role === "user") user = next.info
      else scan.assistant ??= next.info
    }
    if (!user) return undefined
    const after = await page()
    if (fingerprint(after.info) !== headKey) { this.reset(); return undefined }
    const messages = [{ info: user }]
    if (scan.assistant && scan.assistant.parentID === user.id && messageAfter(scan.assistant, user)) messages.push({ info: scan.assistant })
    this.scan = undefined
    return messages
  }
}

// Route/producer-local topology cache. Large scans resume across scheduler slices;
// stream invalidations reject semantic samples without throwing away ancestry work.
// Only topology/ownership invalidations retire the scan itself.
export class ObservationRecovery {
  constructor(clock = runtimeClock) { this.clock = clock; this.reset() }
  reset() {
    this.epoch = (this.epoch ?? 0) + 1; this.route = undefined
    this.family = undefined; this.hint = undefined; this.scan = undefined; this.verifiedAt = -Infinity
    this.outcome = new OutcomeRecovery()
    this.invalidateMessages(); this.historyAt = -Infinity; this.wasBusy = false
  }
  invalidateMessages() {
    this.messageGeneration = (this.messageGeneration ?? 0) + 1; this.messages = undefined
    this.outcome?.reset()
  }
  tracks(id) {
    return validID(id) && (this.hint?.members.some(s => s.id === id)
      || this.scan?.known.has(id) || this.scan?.next === id)
  }
  topology(type, info) {
    if (!info || (!this.tracks(info.id) && !this.tracks(info.parentID) && info.id !== this.route)) return false
    const old = this.hint?.members.find(s => s.id === info.id) ?? this.scan?.known.get(info.id)
    if (type === "session.updated" && old && old.parentID === info.parentID) {
      if (this.family?.root.id === info.id) this.family.root = sessionMetadata(info)
      if (this.hint?.root.id === info.id) this.hint.root = sessionMetadata(info)
      return true
    }
    const removed = new Set([info.id])
    for (const member of this.hint?.members ?? []) if (removed.has(member.parentID)) removed.add(member.id)
    // Hints qualify immediate input/activity for unaffected existing members;
    // they never establish complete family membership or a terminal outcome.
    if (removed.has(this.hint?.root.id) || removed.has(this.route)) this.hint = undefined
    else if (this.hint) this.hint = { ...this.hint, members: this.hint.members.filter(s => !removed.has(s.id)) }
    this.epoch++; this.family = undefined; this.scan = undefined
    this.invalidateMessages()
    return true
  }
  async topologySlice(client, options, startedAt = this.clock.now()) {
    if (this.family && this.clock.now() - this.verifiedAt < observationBudget.topologyMs) return this.family
    this.family = undefined
    const epoch = this.epoch
    const scan = this.scan ??= { phase: "ancestors", next: this.route, known: new Map(),
      ancestors: [], members: [], cursor: 0 }
    const request = async (method, id) => {
      const value = data(await client.session[method]({ sessionID: id }, options))
      if (epoch !== this.epoch || options.signal?.aborted) throw Error("Topology invalidated")
      return value
    }
    for (let calls = 0; calls < observationBudget.topologySliceCalls; calls++) {
      if (scan.phase === "ancestors") {
        const info = sessionMetadata(await request("get", scan.next))
        if (info.id !== scan.next || scan.known.has(info.id) || scan.ancestors.length >= 64) throw Error("Invalid ancestry")
        scan.known.set(info.id, info); scan.ancestors.push(info)
        if (info.parentID) { scan.next = info.parentID; continue }
        scan.root = info; scan.members.push(info); scan.seen = new Set([info.id])
        scan.phase = "children"; scan.cursor = 0
        continue
      }
      if (scan.phase === "ancestors-check") {
        const old = scan.ancestors[scan.cursor]
        const info = sessionMetadata(await request("get", old.id))
        if (info.id !== old.id || info.parentID !== old.parentID) throw Error("Family changed")
        if (info.id === scan.root.id) scan.root = info
        if (++scan.cursor === scan.ancestors.length) {
          if (!scan.seen.has(this.route)) throw Error("Selected session outside family")
          this.family = { root: scan.root, members: scan.members }
          this.hint = { ...this.family }
          this.scan = undefined; this.verifiedAt = startedAt
          return this.family
        }
        continue
      }
      const parent = scan.members[scan.cursor]
      const response = await request("children", parent.id)
      if (!Array.isArray(response) || response.length > 63) throw Error("Family exceeds bound")
      const children = response.map(sessionMetadata), ids = new Set()
      for (const child of children) {
        if (child.parentID !== parent.id || ids.has(child.id)) throw Error("Invalid child ancestry")
        ids.add(child.id)
      }
      if (scan.phase === "children") {
        for (const child of children) {
          if (scan.seen.has(child.id)) throw Error("Invalid child ancestry")
          if (scan.seen.size >= 64) throw Error("Family exceeds bound")
          scan.seen.add(child.id); scan.known.set(child.id, child); scan.members.push(child)
        }
      } else {
        const old = scan.members.filter(s => s.parentID === parent.id)
        if (old.length !== ids.size || old.some(s => !ids.has(s.id))) throw Error("Family changed")
      }
      if (++scan.cursor === scan.members.length) {
        scan.phase = scan.phase === "children" ? "children-check" : "ancestors-check"
        scan.cursor = 0
      }
    }
    return undefined // Incomplete traversal never establishes family quiescence.
  }
  async collect(api, signal) {
    const selected = api.route.current
    if (!api.state.ready) throw Error("Not ready")
    if (selected.name === "home") { this.reset(); return null }
    if (selected.name !== "session" || !validID(selected.params?.sessionID)) throw Error("Unowned route")
    if (this.route !== selected.params.sessionID) { this.reset(); this.route = selected.params.sessionID }
    const epoch = this.epoch, options = { signal, throwOnError: true }, client = api.client
    const startedAt = this.clock.now()
    try {
      const family = await this.topologySlice(client, options, startedAt)
      if (!family) return undefined
      const before = data(await client.session.status({}, options))
      const validateStatuses = statuses => {
        if (!statuses || typeof statuses !== "object" || Array.isArray(statuses)
            || family.members.some(s => statuses[s.id] !== undefined && !["idle", "busy", "retry"].includes(statuses[s.id]?.type))) {
          throw Error("Invalid activity response")
        }
      }
      validateStatuses(before)
      const busy = family.members.some(s => ["busy", "retry"].includes(before[s.id]?.type))
      if (busy !== this.wasBusy) this.invalidateMessages()
      this.wasBusy = busy
      if (busy) {
        // Local structured metadata is useful for a Working turn label, never
        // for Idle/Done. No global-ready or hydration assumption is involved.
        const local = api.state.session?.messages?.(family.root.id)
        const user = Array.isArray(local) && local.length <= 100 ? local.findLast(m => m.role === "user") : undefined
        if (user?.sessionID === family.root.id && validID(user.id) && validID(`${family.root.id}/${user.id}`)) {
          this.messages = [messageMetadata(user)]
        }
      }
      const messageGeneration = this.messageGeneration
      // While working, tool bodies convey no terminal authority. Narrow metadata
      // repair has independent deadlines; unverified local hydration is never idle.
      const recoverHistory = !busy && (!this.messages || this.clock.now() - this.historyAt >= observationBudget.historyMs)
      const [history, permissions, questions] = await Promise.all([
        recoverHistory ? this.outcome.sample(api, family.root.id, options) : undefined,
        client.permission.list({}, options).then(data), client.question.list({}, options).then(data),
      ])
      const statuses = data(await client.session.status({}, options))
      validateStatuses(statuses)
      for (const member of family.members) {
        if ((before[member.id]?.type ?? "idle") !== (statuses[member.id]?.type ?? "idle")) throw Error("Activity changed during observation")
      }
      if (epoch !== this.epoch || api.route.current.name !== selected.name
          || api.route.current.params?.sessionID !== selected.params.sessionID || signal?.aborted) throw Error("Route changed")
      if (messageGeneration !== this.messageGeneration) return undefined
      if (recoverHistory) {
        this.messages = history
        if (history) this.historyAt = startedAt
      }
      const inputs = requests => {
        if (!Array.isArray(requests)) throw Error("Invalid input response")
        const owned = requests.filter(r => family.members.some(s => s.id === r.sessionID))
        if (owned.length > 100 || owned.some(r => !validID(r.id)) || new Set(owned.map(r => r.id)).size !== owned.length) {
          throw Error("Input exceeds bound")
        }
        return owned.map(r => ({ id: r.id, sessionID: r.sessionID }))
      }
      const ownedPermissions = inputs(permissions), ownedQuestions = inputs(questions)
      if (ownedPermissions.length + ownedQuestions.length > 100) throw Error("Input exceeds bound")
      return { ...family, statuses, messages: this.messages ?? [], messagesComplete: !busy && Boolean(this.messages),
        permissions: ownedPermissions, questions: ownedQuestions }
    } catch (error) {
      if (epoch === this.epoch) {
        // A timed-out read contributes no graph facts, but completed reads may
        // resume. Ownership/topology invalidations have already retired the epoch.
        if (!signal?.aborted || !this.scan) {
          this.epoch++; this.family = undefined; this.hint = undefined; this.scan = undefined
        }
        this.invalidateMessages()
      }
      throw error
    }
  }
}

// One-shot small-family compatibility helper; production retains recovery state.
export const reconcile = (api, signal, recovery = new ObservationRecovery()) => recovery.collect(api, signal)

// Bounded FIFO: no asynchronous reporter per event, no shell interpolation.
// Overload invalidates semantic authority until a fresh reconciliation arrives.
export class SnapshotQueue {
  constructor(limit = 128) {
    this.limit = limit; this.items = []; this.overflow = false
    this.inFlight = undefined; this.delivered = undefined
  }
  push(value, renew = false) {
    const encoded = JSON.stringify(value)
    // Compare the effective tail, including the frame awaiting a native receipt.
    // A -> B -> A remains ordered even when the first A was already delivered.
    if (!renew && encoded === (this.items.at(-1) ?? this.inFlight ?? this.delivered)) return
    if (this.items.length >= this.limit) {
      this.items.length = 0
      this.overflow = true
      this.items.push(JSON.stringify(unknown(value.version)))
      return
    }
    this.items.push(encoded)
  }
  shift() {
    const item = this.items.shift()
    if (item) this.inFlight = item
    return item
  }
  acknowledge() { this.delivered = this.inFlight; this.inFlight = undefined }
  reset() {
    this.items.length = 0; this.inFlight = undefined; this.delivered = undefined
    this.overflow = false
  }
}

export const observationBudget = Object.freeze({
  activeMs: 1000, idleMs: 10000, leaseMs: 5000, maxBackoffMs: 30000,
  topologyMs: 10000, historyMs: 10000, topologySliceCalls: 16,
  historySliceCalls: 6, historyMessages: 100,
  responseBytes: 1024 * 1024, metadataBytes: 64 * 1024, jsonDepth: 32, jsonNodes: 16384,
  requestsPerSecond: 32, requestBurst: 8, concurrency: 4, pendingRequests: 4,
})

const runtimeClock = {
  now: () => performance.now(),
  setTimeout: (fn, ms) => setTimeout(fn, ms), clearTimeout: timer => clearTimeout(timer),
}

// One in-flight refresh, one dirty bit and one timer. Events during a refresh
// reject that generation and retain one later retry, regardless of event rate.
export class ReconcileScheduler {
  constructor(collect, active, clock = runtimeClock) {
    this.collect = collect; this.active = active; this.clock = clock
    this.dirty = false; this.inFlight = false; this.closed = false
    this.generation = 0; this.lastStart = -Infinity; this.retryAt = 0; this.failures = 0
    this.timer = undefined; this.due = Infinity
  }
  invalidate({ resetBackoff = false } = {}) {
    if (this.closed) return
    this.generation++; this.dirty = true
    if (resetBackoff) { this.failures = 0; this.retryAt = 0 }
    this.arm()
  }
  arm() {
    if (this.closed || this.inFlight) return
    const interval = this.dirty || this.active() ? observationBudget.activeMs : observationBudget.idleMs
    const due = Math.max(this.clock.now(), this.lastStart + interval, this.retryAt)
    // Repeated invalidations must not allocate another timer/microtask.
    if (this.timer !== undefined && this.due <= due) return
    if (this.timer !== undefined) this.clock.clearTimeout(this.timer)
    this.due = due
    this.timer = this.clock.setTimeout(() => { this.timer = undefined; void this.run() }, Math.max(0, due - this.clock.now()))
  }
  async run() {
    if (this.closed || this.inFlight) return
    this.inFlight = true; this.dirty = false; this.lastStart = this.clock.now()
    const generation = this.generation
    try {
      const accepted = await this.collect()
      if (accepted === false || generation !== this.generation) this.dirty = true
      this.failures = 0; this.retryAt = 0
    } catch {
      this.dirty = true
      this.failures = Math.min(this.failures + 1, 6)
      this.retryAt = this.clock.now() + Math.min(observationBudget.maxBackoffMs,
        observationBudget.activeMs * 2 ** (this.failures - 1))
    } finally {
      this.inFlight = false
      this.arm()
    }
  }
  dispose() {
    this.closed = true; this.dirty = false
    if (this.timer !== undefined) this.clock.clearTimeout(this.timer)
    this.timer = undefined
  }
}

// Charge actual endpoint invocations, not refresh function entries. Token bucket
// bounds bursts; one dispatcher timer and fixed pending/concurrency limits bound
// waiting work. Aborting a caller does not free its physical request slot early.
export class RequestBudget {
  constructor(clock = runtimeClock) {
    this.clock = clock; this.tokens = observationBudget.requestBurst; this.refilledAt = clock.now()
    this.waiting = []; this.active = 0; this.calls = 0; this.timer = undefined
    this.disposal = new AbortController()
  }
  call(invoke, signal) {
    const combined = signal ? AbortSignal.any([signal, this.disposal.signal]) : this.disposal.signal
    if (combined.aborted) return Promise.reject(new Error("Observation cancelled"))
    if (this.waiting.length >= observationBudget.pendingRequests) return Promise.reject(new Error("Request budget queue full"))
    return new Promise((resolve, reject) => {
      const job = { invoke, signal: combined, resolve, reject }
      job.cancel = () => {
        const index = this.waiting.indexOf(job)
        if (index >= 0) { this.waiting.splice(index, 1); combined.removeEventListener("abort", job.cancel) }
        reject(new Error("Observation cancelled"))
        this.drain()
      }
      combined.addEventListener("abort", job.cancel, { once: true })
      this.waiting.push(job)
      this.drain()
    })
  }
  drain() {
    if (this.timer !== undefined) { this.clock.clearTimeout(this.timer); this.timer = undefined }
    if (this.disposal.signal.aborted) return
    const now = this.clock.now()
    this.tokens = Math.min(observationBudget.requestBurst,
      this.tokens + (now - this.refilledAt) * observationBudget.requestsPerSecond / 1000)
    this.refilledAt = now
    while (this.waiting.length && this.active < observationBudget.concurrency && this.tokens >= 1) {
      const job = this.waiting.shift()
      if (job.signal.aborted) {
        job.signal.removeEventListener("abort", job.cancel)
        job.reject(new Error("Observation cancelled"))
        continue
      }
      this.tokens--; this.active++; this.calls++
      let response
      try { response = Promise.resolve(job.invoke(job.signal)) } catch (error) { response = Promise.reject(error) }
      response.then(job.resolve, job.reject).finally(() => {
        job.signal.removeEventListener("abort", job.cancel)
        this.active--
        this.drain()
      })
    }
    if (this.waiting.length && this.active < observationBudget.concurrency) {
      this.timer = this.clock.setTimeout(() => { this.timer = undefined; this.drain() },
        Math.max(1, (1 - this.tokens) * 1000 / observationBudget.requestsPerSecond))
    }
  }
  dispose() {
    if (this.timer !== undefined) this.clock.clearTimeout(this.timer)
    this.timer = undefined
    this.disposal.abort()
  }
}

export function budgetedAPI(api, budget) {
  const client = {}
  const worker = new WorkerMetadata(api)
  budget.disposal.signal.addEventListener("abort", () => worker.dispose(), { once: true })
  for (const [namespace, methods] of Object.entries({ session: ["status", "get", "children", "messages"],
    permission: ["list"], question: ["list"] })) {
    client[namespace] = {}
    for (const method of methods) {
      client[namespace][method] = (args, options = {}) => budget.call(signal =>
        api.client[namespace][method](args, boundedReadOptions({ ...options, signal },
          namespace === "session" && method === "messages", method === "messages", worker)), options.signal)
    }
  }
  const observed = Object.create(api)
  Object.defineProperty(observed, "client", { value: client })
  return observed
}
