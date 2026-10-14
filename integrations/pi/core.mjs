// Pi 1.2.0 metadata-only projection and bounded reporter transport.
// Prompts, assistant text, tool arguments/results, provider bodies and dialog contents are strictly excluded.
import { randomUUID as nodeCryptoRandomUUID } from "node:crypto"
import { spawn as spawnProcess } from "node:child_process"
import { StringDecoder } from "node:string_decoder"

export const PROTOCOL_VERSION = 1
export const SUPPORTED_VERSION = "1.2.0"

export function supportsVersion(version) {
  return typeof version === "string" && version === SUPPORTED_VERSION
}

export const ERROR_KINDS = Object.freeze([
  "provider_auth",
  "provider_quota",
  "provider_rate_limit",
  "provider_unavailable",
  "session_corrupt",
  "extension_error",
  "terminal_failure",
])

export const REQUEST_KINDS = Object.freeze(["confirmation", "question", "ui"])
export const PI_ACTIVITIES = Object.freeze(["idle", "thinking", "working", "retrying"])

// Native AgentStatus serialized values from verij-types/src/agent.rs
export const ALLOWED_STATUSES = Object.freeze([
  "idle",
  "needs_input",
  "working",
  "done",
  "error",
  "unknown",
])

export const ALLOWED_READY_KEYS = Object.freeze([
  "ready",
  "agent_instance_id",
  "source_epoch",
  "source_revision",
  "status",
  "restored",
  "aggregate_run_id",
  "latest_snapshot",
])

export const ALLOWED_RECEIPT_KEYS = Object.freeze([
  "source_epoch",
  "source_revision",
  "record_revision",
  "completion_revision",
  "status",
])

export const ALLOWED_SNAPSHOT_KEYS = Object.freeze([
  "schema_version",
  "version",
  "conversation_id",
  "conversation_title",
  "run_id",
  "activity",
  "pending_requests",
  "background_task_count",
  "outcome",
])

export const observationBudget = Object.freeze({
  leaseMs: 5000,
  maxBackoffMs: 30000,
  startupTimeoutMs: 4000,
  receiptTimeoutMs: 4000,
  shutdownWaitMs: 500,
  maxQueueSize: 128,
  minWriteIntervalMs: 100,
})

export function isControlChar(code) {
  // Matches Rust char::is_control (C0: 0x00..0x1F, DEL: 0x7F, C1: 0x80..0x9F)
  return code <= 0x1f || (code >= 0x7f && code <= 0x9f)
}

export function hasLoneSurrogate(str) {
  if (typeof str !== "string") return false
  for (let i = 0; i < str.length; i++) {
    const code = str.charCodeAt(i)
    if (code >= 0xd800 && code <= 0xdbff) {
      i++
      if (i >= str.length) return true
      const next = str.charCodeAt(i)
      if (next < 0xdc00 || next > 0xdfff) return true
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      return true
    }
  }
  return false
}

export function isValidU32(val) {
  if (typeof val === "number") {
    return Number.isSafeInteger(val) && val >= 0 && val <= 0xffffffff
  }
  if (typeof val === "string" && /^\d+$/.test(val)) {
    const num = Number(val)
    return Number.isSafeInteger(num) && num >= 0 && num <= 0xffffffff
  }
  return false
}

export function sanitizeId(id) {
  if (typeof id !== "string" || id.length === 0) return undefined
  if (hasLoneSurrogate(id)) return undefined
  for (let i = 0; i < id.length; i++) {
    const code = id.charCodeAt(i)
    if (isControlChar(code)) return undefined
  }
  const bytes = Buffer.byteLength(id, "utf8")
  if (bytes > 256) return undefined
  return id
}

export function sanitizeTitle(title) {
  if (typeof title !== "string" || title.length === 0) return undefined
  let clean = ""
  for (let i = 0; i < title.length; i++) {
    const code = title.charCodeAt(i)
    if (code >= 0xd800 && code <= 0xdbff) {
      if (i + 1 < title.length) {
        const next = title.charCodeAt(i + 1)
        if (next >= 0xdc00 && next <= 0xdfff) {
          clean += title[i] + title[i + 1]
          i++
          continue
        }
      }
      continue // Drop lone high surrogate
    }
    if (code >= 0xdc00 && code <= 0xdfff) {
      continue // Drop lone low surrogate
    }
    if (!isControlChar(code)) {
      clean += title[i]
    }
  }
  if (clean.length === 0) return undefined
  let byteLen = 0
  let cutIndex = 0
  for (const ch of clean) {
    const chBytes = Buffer.byteLength(ch, "utf8")
    if (byteLen + chBytes > 256) break
    byteLen += chBytes
    cutIndex += ch.length
  }
  const result = clean.slice(0, cutIndex)
  return result.length ? result : undefined
}

export function classifyError(error) {
  let message = ""
  if (typeof error === "string") {
    message = error
  } else if (error && typeof error === "object") {
    if (typeof error.message === "string") message = error.message
    else if (typeof error.errorMessage === "string") message = error.errorMessage
    else if (typeof error.kind === "string") message = error.kind
  }
  const text = message.toLowerCase()
  if (/auth|401|unauthorized|api[ _-]?key|forbidden/.test(text)) return "provider_auth"
  if (/quota|billing|exceeded[ _-]?quota|insufficient[ _-]?quota/.test(text)) return "provider_quota"
  if (/rate[ _-]?limit|429|too many requests/.test(text)) return "provider_rate_limit"
  if (/503|service unavailable|overloaded|unavailable/.test(text)) return "provider_unavailable"
  if (/corrupt|malformed|json parse/.test(text)) return "session_corrupt"
  if (/extension/.test(text)) return "extension_error"
  return "terminal_failure"
}

export function mapPromptKind(kind) {
  switch (kind) {
    case "confirm":
      return "confirmation"
    case "select":
    case "input":
      return "question"
    case "editor":
    case "custom":
    default:
      return "ui"
  }
}

export function unknown(version = SUPPORTED_VERSION) {
  return {
    schema_version: PROTOCOL_VERSION,
    version,
    pending_requests: [],
    background_task_count: 0,
  }
}

export function validateStrictSnapshot(snap, expectedConvId = null) {
  if (!snap || typeof snap !== "object" || Array.isArray(snap)) return false
  for (const k of Object.keys(snap)) {
    if (!ALLOWED_SNAPSHOT_KEYS.includes(k)) return false
  }
  if (snap.schema_version !== PROTOCOL_VERSION) return false
  if (snap.version !== SUPPORTED_VERSION) return false

  if (snap.conversation_id !== undefined && snap.conversation_id !== null) {
    if (typeof snap.conversation_id !== "string" || snap.conversation_id.length === 0) return false
    if (hasLoneSurrogate(snap.conversation_id)) return false
    if (Buffer.byteLength(snap.conversation_id, "utf8") > 256) return false
    for (let i = 0; i < snap.conversation_id.length; i++) {
      if (isControlChar(snap.conversation_id.charCodeAt(i))) return false
    }
    if (expectedConvId && snap.conversation_id !== expectedConvId) return false
  }

  if (snap.conversation_title !== undefined && snap.conversation_title !== null) {
    if (typeof snap.conversation_title !== "string") return false
    if (hasLoneSurrogate(snap.conversation_title)) return false
    if (Buffer.byteLength(snap.conversation_title, "utf8") > 256) return false
    for (let i = 0; i < snap.conversation_title.length; i++) {
      if (isControlChar(snap.conversation_title.charCodeAt(i))) return false
    }
  }

  if (snap.run_id !== undefined && snap.run_id !== null) {
    if (typeof snap.run_id !== "string" || snap.run_id.length === 0) return false
    if (hasLoneSurrogate(snap.run_id)) return false
    if (Buffer.byteLength(snap.run_id, "utf8") > 256) return false
    for (let i = 0; i < snap.run_id.length; i++) {
      if (isControlChar(snap.run_id.charCodeAt(i))) return false
    }
  }

  if (snap.activity !== undefined && snap.activity !== null) {
    if (!PI_ACTIVITIES.includes(snap.activity)) return false
  }

  if (snap.pending_requests !== undefined) {
    if (!Array.isArray(snap.pending_requests) || snap.pending_requests.length > 1) return false
    for (const req of snap.pending_requests) {
      if (!req || typeof req !== "object" || Array.isArray(req)) return false
      for (const rk of Object.keys(req)) {
        if (rk !== "id" && rk !== "kind") return false
      }
      if (typeof req.id !== "string" || !sanitizeId(req.id)) return false
      if (!REQUEST_KINDS.includes(req.kind)) return false
    }
  }

  if (snap.background_task_count !== undefined) {
    if (!Number.isSafeInteger(snap.background_task_count) || snap.background_task_count < 0 || snap.background_task_count > 64) {
      return false
    }
  }

  if (snap.outcome !== undefined && snap.outcome !== null) {
    if (!snap.outcome || typeof snap.outcome !== "object" || Array.isArray(snap.outcome)) return false
    if (!snap.run_id || !snap.conversation_id) return false
    if (snap.activity !== "idle") return false
    if (snap.pending_requests && snap.pending_requests.length > 0) return false
    if (snap.background_task_count !== 0) return false

    const kind = snap.outcome.kind
    if (kind === "completed") {
      for (const ok of Object.keys(snap.outcome)) {
        if (ok !== "kind" && ok !== "completed_at_ms") return false
      }
      if (!Number.isSafeInteger(snap.outcome.completed_at_ms) || snap.outcome.completed_at_ms <= 0) return false
    } else if (kind === "error") {
      for (const ok of Object.keys(snap.outcome)) {
        if (ok !== "kind" && ok !== "error_kind") return false
      }
      if (!ERROR_KINDS.includes(snap.outcome.error_kind)) return false
    } else if (kind === "aborted") {
      for (const ok of Object.keys(snap.outcome)) {
        if (ok !== "kind") return false
      }
    } else {
      return false
    }
  }

  return true
}

export function snapshotsSemanticallyEqual(a, b) {
  if (!a || !b) return false
  if (a.schema_version !== b.schema_version) return false
  if (a.version !== b.version) return false
  if ((a.conversation_id ?? null) !== (b.conversation_id ?? null)) return false
  if ((a.conversation_title ?? null) !== (b.conversation_title ?? null)) return false
  if ((a.run_id ?? null) !== (b.run_id ?? null)) return false
  if ((a.activity ?? null) !== (b.activity ?? null)) return false
  if ((a.background_task_count ?? 0) !== (b.background_task_count ?? 0)) return false

  const aReqs = a.pending_requests ?? []
  const bReqs = b.pending_requests ?? []
  if (aReqs.length !== bReqs.length) return false
  for (let i = 0; i < aReqs.length; i++) {
    if (aReqs[i].id !== bReqs[i].id || aReqs[i].kind !== bReqs[i].kind) return false
  }

  const aOut = a.outcome ?? null
  const bOut = b.outcome ?? null
  if (!aOut && !bOut) return true
  if (!aOut || !bOut) return false
  if (aOut.kind !== bOut.kind) return false
  if (aOut.kind === "completed" && aOut.completed_at_ms !== bOut.completed_at_ms) return false
  if (aOut.kind === "error" && aOut.error_kind !== bOut.error_kind) return false
  return true
}

// Bounded FIFO queue for serialized snapshots.
// Total capacity encompasses in-flight and queued items up to the limit (128).
export class SnapshotQueue {
  constructor(limit = observationBudget.maxQueueSize, onOverflow = null) {
    this.limit = limit
    this.onOverflow = onOverflow
    this.items = []
    this.overflow = false
    this.inFlight = undefined
    this.delivered = undefined
  }

  totalCount() {
    return this.items.length + (this.inFlight !== undefined ? 1 : 0)
  }

  push(value, renew = false) {
    const encoded = typeof value === "string" ? value : JSON.stringify(value)
    if (!renew && encoded === (this.items.at(-1) ?? this.inFlight ?? this.delivered)) return

    if (this.totalCount() >= this.limit) {
      this.items.length = 0
      this.inFlight = undefined
      this.overflow = true
      if (typeof this.onOverflow === "function") {
        this.onOverflow()
      }
      this.items.push(JSON.stringify(unknown(value?.version)))
      return
    }
    this.items.push(encoded)
  }

  shift() {
    const item = this.items.shift()
    if (item) this.inFlight = item
    return item
  }

  acknowledge() {
    this.delivered = this.inFlight
    this.inFlight = undefined
  }

  requeueInFlight() {
    if (this.inFlight !== undefined) {
      const item = this.inFlight
      this.inFlight = undefined
      if (this.items.length >= this.limit) {
        this.items.length = 0
        this.overflow = true
        if (typeof this.onOverflow === "function") {
          this.onOverflow()
        }
        this.items.push(JSON.stringify(unknown()))
      } else {
        this.items.unshift(item)
      }
    }
  }

  reset() {
    this.items.length = 0
    this.inFlight = undefined
    this.delivered = undefined
    this.overflow = false
  }
}

// Pure event-to-snapshot state projection.
// Maintains conservative aggregate run identity, final outcome settlement,
// and single outer blocking UI prompt metadata.
export class StateProjector {
  constructor(options = {}) {
    this.version = options.version ?? SUPPORTED_VERSION
    this.randomUUID = options.randomUUID ?? nodeCryptoRandomUUID
    this.runtimeId = options.runtimeId ?? this.randomUUID()
    this.conversationId = sanitizeId(options.conversationId)
    this.conversationTitle = sanitizeTitle(options.conversationTitle)
    this.aggregateRunId = null
    this.activity = "idle"
    this.previousExecutionActivity = "idle"
    this.pendingRequests = []
    this.backgroundTaskCount = 0
    this.candidateOutcome = null
    this.outcome = null
    this.activePrompt = null
    this.promptSavedState = null
    this.runActive = false
    this.genuineAggregate = false
    this.compactionActive = false
    this.activeToolCount = 0
    this.degradedToUnknown = false
  }

  invalidateOnOverflow() {
    this.degradedToUnknown = true
    this.candidateOutcome = null
    this.outcome = null
    this.activity = null
    this.previousExecutionActivity = null
    if (this.promptSavedState) {
      this.promptSavedState.outcome = null
      this.promptSavedState.stateProgressedDuringPrompt = true
    }
  }

  handleEvent(type, event = {}, ctx = {}) {
    if (ctx && typeof ctx === "object" && ctx.sessionManager) {
      try {
        const id = ctx.sessionManager.getSessionId?.()
        if (id !== undefined && id !== null) {
          const cleanId = sanitizeId(id)
          if (!cleanId || (this.conversationId && cleanId !== this.conversationId)) {
            // Unexpected context conversation change or invalid ID
            this.conversationId = cleanId ?? undefined
            this.conversationTitle = undefined
            this.runtimeId = this.randomUUID()
            this.aggregateRunId = null
            this.candidateOutcome = null
            this.outcome = null
            this.activePrompt = null
            this.promptSavedState = null
            this.pendingRequests = []
            this.backgroundTaskCount = 0
            this.runActive = false
            this.genuineAggregate = false
            this.compactionActive = false
            this.activeToolCount = 0
            this.degradedToUnknown = true
            this.activity = null
            this.previousExecutionActivity = null
            return this.getSnapshot()
          } else if (!this.conversationId && cleanId) {
            this.conversationId = cleanId
          }
        }
        const name = ctx.sessionManager.getSessionName?.()
        if (name === undefined || name === null || name === "") {
          this.conversationTitle = undefined
        } else {
          this.conversationTitle = sanitizeTitle(name)
        }
      } catch {}
    }

    switch (type) {
      case "session_start": {
        const rawId = event?.session ?? (ctx?.sessionManager?.getSessionId ? ctx.sessionManager.getSessionId() : undefined) ?? this.conversationId
        const cleanId = sanitizeId(rawId)
        if (cleanId !== this.conversationId) {
          this.runtimeId = this.randomUUID()
          this.conversationId = cleanId ?? undefined
        }
        const rawName = event?.name ?? (ctx?.sessionManager?.getSessionName ? ctx.sessionManager.getSessionName() : undefined)
        if (rawName === undefined || rawName === null || rawName === "") {
          this.conversationTitle = undefined
        } else {
          this.conversationTitle = sanitizeTitle(rawName)
        }

        this.runActive = false
        this.genuineAggregate = false
        this.compactionActive = false
        this.activeToolCount = 0
        this.activePrompt = null
        this.promptSavedState = null
        this.pendingRequests = []
        this.backgroundTaskCount = 0
        this.candidateOutcome = null
        this.outcome = null
        this.degradedToUnknown = false

        if (event.reason === "startup" || event.reason === "new" || event.reason === "resume" || event.reason === "fork") {
          this.aggregateRunId = null
          this.activity = "idle"
          this.previousExecutionActivity = "idle"
        } else if (event.reason === "reload") {
          // A reload cannot prove active work/UI continuity: reports Unknown
          this.aggregateRunId = null
          this.activity = null
          this.previousExecutionActivity = null
        } else {
          this.aggregateRunId = null
          this.activity = "idle"
          this.previousExecutionActivity = "idle"
        }
        break
      }

      case "session_info_changed": {
        const rawName = event.name ?? (ctx.sessionManager?.getSessionName ? ctx.sessionManager.getSessionName() : undefined)
        if (rawName === undefined || rawName === null || rawName === "") {
          this.conversationTitle = undefined
        } else {
          this.conversationTitle = sanitizeTitle(rawName)
        }
        break
      }

      case "session_tree": {
        // Retire branch-sensitive execution facts conservatively
        this.candidateOutcome = null
        this.aggregateRunId = null
        this.outcome = null
        if (this.promptSavedState) {
          this.promptSavedState.stateProgressedDuringPrompt = true
        }
        this.promptSavedState = null
        this.runActive = false
        this.genuineAggregate = false

        // Preserve active compaction or Unknown, do not blindly set idle
        if (this.degradedToUnknown || this.activity === null) {
          this.activity = null
          this.previousExecutionActivity = null
        } else if (this.compactionActive) {
          this.activity = "working"
          this.previousExecutionActivity = "working"
        } else {
          this.activity = "idle"
          this.previousExecutionActivity = "idle"
        }
        break
      }

      case "session_before_compact": {
        this.compactionActive = true
        // Work after pre-settle invalidates prior candidate
        this.candidateOutcome = null
        this.activity = "working"
        break
      }

      case "session_compact":
      case "session_compact_failed": {
        this.compactionActive = false
        if (this.runActive || this.activeToolCount > 0) {
          this.activity = "working"
        } else if (this.degradedToUnknown || this.previousExecutionActivity === null) {
          this.activity = null
        } else {
          // Restore previous qualified execution activity when not running new work
          this.activity = this.previousExecutionActivity ?? "idle"
        }
        break
      }

      case "before_agent_start": {
        this.genuineAggregate = true
        this.runActive = true
        this.aggregateRunId = `run-${this.randomUUID()}`
        this.candidateOutcome = null
        this.outcome = null
        if (this.promptSavedState) {
          this.promptSavedState.stateProgressedDuringPrompt = true
        }
        this.promptSavedState = null
        this.degradedToUnknown = false
        this.activity = "working"
        this.previousExecutionActivity = "working"
        break
      }

      case "agent_start": {
        this.runActive = true
        if (!this.aggregateRunId) {
          this.aggregateRunId = `run-${this.randomUUID()}`
        }
        // Tools/agent_start must clear old outcome for resumed work
        this.candidateOutcome = null
        this.outcome = null
        if (this.promptSavedState) {
          this.promptSavedState.stateProgressedDuringPrompt = true
        }
        this.promptSavedState = null
        this.activity = "working"
        this.previousExecutionActivity = "working"
        break
      }

      case "turn_start": {
        // Active turn resumption clears old outcomes and candidates
        this.candidateOutcome = null
        this.outcome = null
        this.activity = "working"
        this.previousExecutionActivity = "working"
        break
      }

      case "tool_execution_start": {
        this.activeToolCount++
        this.candidateOutcome = null
        this.outcome = null
        if (this.activeToolCount > 64) {
          this.degradedToUnknown = true
          this.activity = null
          this.previousExecutionActivity = null
        } else {
          this.activity = "working"
          if (this.runActive) {
            this.previousExecutionActivity = "working"
          }
        }
        break
      }

      case "tool_execution_end": {
        this.activeToolCount = Math.max(0, this.activeToolCount - 1)
        if (this.activeToolCount > 64) {
          this.degradedToUnknown = true
          this.activity = null
          this.previousExecutionActivity = null
          this.outcome = null
        } else if (!this.degradedToUnknown) {
          if (this.runActive) {
            this.activity = "working"
            this.previousExecutionActivity = "working"
          } else if (this.activeToolCount === 0) {
            // Standalone idle tool execution finished: restore prior execution activity, not working forever
            this.activity = this.previousExecutionActivity ?? null
          }
        }
        break
      }

      case "turn_end": {
        // Turn boundary: captured for context, never published as terminal outcome here
        break
      }

      case "agent_end": {
        // Low-level boundary: do not settle or mark idle; retries or continuation may follow
        break
      }

      case "agent_before_settle": {
        // Capture candidate outcome for qualification at agent_settled only
        if (event.outcome === "completed") {
          this.candidateOutcome = { outcome: "completed" }
        } else if (event.outcome === "error") {
          const errKind = classifyError(event.errorMessage ?? event.message?.errorMessage ?? event.error)
          this.candidateOutcome = { outcome: "error", error_kind: errKind }
        } else if (event.outcome === "aborted") {
          this.candidateOutcome = { outcome: "aborted" }
        } else {
          this.candidateOutcome = null
        }
        break
      }

      case "agent_settled": {
        this.runActive = false
        const candidate = this.candidateOutcome

        if (typeof event.backgroundTaskCount === "number") {
          this.backgroundTaskCount = event.backgroundTaskCount
        } else if (typeof event.background_tasks === "number") {
          this.backgroundTaskCount = event.background_tasks
        }

        if (this.promptSavedState) {
          this.promptSavedState.stateProgressedDuringPrompt = true
        }

        const isQuiescent = (
          this.activeToolCount === 0 &&
          !this.compactionActive &&
          this.pendingRequests.length === 0 &&
          this.backgroundTaskCount === 0 &&
          !this.degradedToUnknown &&
          (!ctx || typeof ctx.hasPendingMessages !== "function" || !ctx.hasPendingMessages())
        )

        const candidateOutcome = candidate?.outcome
        const isAborted = event.aborted === true
        const isExplicitlyNotAborted = event.aborted === false

        if (isAborted) {
          // Conflicting aborted candidate vs non-aborted unknown
          if (candidateOutcome && candidateOutcome !== "aborted") {
            this.activity = null
            this.previousExecutionActivity = null
            this.outcome = null
          } else if (isQuiescent) {
            this.activity = "idle"
            this.previousExecutionActivity = "idle"
            this.outcome = { kind: "aborted" }
          } else {
            this.activity = null
            this.previousExecutionActivity = null
            this.outcome = null
          }
        } else if (isExplicitlyNotAborted && this.genuineAggregate && isQuiescent && candidateOutcome === "completed") {
          const completedAt = typeof ctx.now === "function" ? ctx.now() : (typeof event.now === "function" ? event.now() : Date.now())
          this.activity = "idle"
          this.previousExecutionActivity = "idle"
          this.outcome = { kind: "completed", completed_at_ms: Math.max(1, Math.round(completedAt)) }
        } else if (isExplicitlyNotAborted && this.genuineAggregate && isQuiescent && candidateOutcome === "error") {
          this.activity = "idle"
          this.previousExecutionActivity = "idle"
          this.outcome = { kind: "error", error_kind: candidate.error_kind ?? "terminal_failure" }
        } else {
          // Missing pre-settlement evidence, nonquiescent state, conflicting aborted flag, or missing event.aborted === false
          this.activity = null
          this.previousExecutionActivity = null
          this.outcome = null
        }

        this.candidateOutcome = null
        return this.getSnapshot()
      }

      case "ui_prompt_start": {
        if (!this.activePrompt) {
          const id = `req-${this.randomUUID()}`
          const kind = mapPromptKind(event.kind)
          this.activePrompt = { id, kind }
          this.pendingRequests = [{ id, kind }]
          this.promptSavedState = {
            runId: this.aggregateRunId,
            outcome: this.outcome,
            activity: this.previousExecutionActivity,
            stateProgressedDuringPrompt: false,
          }
        }
        break
      }

      case "ui_prompt_end": {
        this.activePrompt = null
        this.pendingRequests = []
        const saved = this.promptSavedState
        this.promptSavedState = null

        if (this.runActive || this.compactionActive || this.activeToolCount > 0) {
          this.activity = "working"
          this.outcome = null
        } else if (this.degradedToUnknown) {
          this.activity = null
          this.previousExecutionActivity = null
          this.outcome = null
        } else if (saved?.stateProgressedDuringPrompt) {
          // State settled or progressed while dialog was open:
          // Closing prompt must never itself complete; conservatively Unknown
          this.activity = null
          this.previousExecutionActivity = null
          this.outcome = null
        } else if (saved && saved.runId === this.aggregateRunId) {
          // Saved state matches current run and state did not progress:
          this.activity = saved.activity
          this.outcome = saved.outcome ?? null
        } else {
          // Run changed or no matching saved state
          this.activity = this.previousExecutionActivity ?? null
          this.outcome = null
        }
        break
      }

      case "session_shutdown": {
        this.runActive = false
        this.genuineAggregate = false
        this.compactionActive = false
        this.activeToolCount = 0
        this.activePrompt = null
        this.promptSavedState = null
        this.pendingRequests = []
        this.candidateOutcome = null
        this.outcome = null
        break
      }
    }

    return this.getSnapshot()
  }

  getSnapshot() {
    if (this.degradedToUnknown) {
      const snap = unknown(this.version)
      if (this.conversationId) snap.conversation_id = this.conversationId
      if (this.activePrompt && this.pendingRequests.length > 0) {
        // Reseed blocking dialog metadata conservatively to NeedsInput without outcome
        snap.pending_requests = this.pendingRequests.slice(0, 1)
      }
      return snap
    }

    const snap = {
      schema_version: PROTOCOL_VERSION,
      version: this.version,
    }
    if (this.conversationId) snap.conversation_id = this.conversationId
    if (this.conversationTitle !== undefined) snap.conversation_title = this.conversationTitle
    if (this.aggregateRunId) snap.run_id = this.aggregateRunId

    const bgCount = Math.min(Math.max(0, this.backgroundTaskCount), 64)
    snap.background_task_count = bgCount

    if (this.pendingRequests.length > 0) {
      // Prompt attention active: do not emit terminal outcome while dialog is unresolved
      if (this.activity && PI_ACTIVITIES.includes(this.activity)) snap.activity = this.activity
      snap.pending_requests = this.pendingRequests.slice(0, 1)
      return snap
    }

    snap.pending_requests = []
    if (this.activity && PI_ACTIVITIES.includes(this.activity)) {
      snap.activity = this.activity
    }

    // Terminal outcomes require strict quiescence:
    // activity === "idle", zero tools, no compaction, zero pending requests, zero background tasks,
    // explicit run_id and conversation_id.
    const isQuiescent = (
      this.activity === "idle" &&
      this.activeToolCount === 0 &&
      !this.compactionActive &&
      this.pendingRequests.length === 0 &&
      bgCount === 0 &&
      Boolean(this.aggregateRunId) &&
      Boolean(this.conversationId)
    )

    if (isQuiescent && this.outcome) {
      snap.outcome = this.outcome
    }

    return snap
  }
}

// Bounded reporter child process lifecycle, framing, backoff and lease management.
export function createBridge(options = {}, runtime = {}) {
  const clock = runtime.clock ?? {
    now: () => Date.now(),
    setTimeout: (fn, ms) => {
      const timer = globalThis.setTimeout(fn, ms)
      if (timer && typeof timer.unref === "function") timer.unref()
      return timer
    },
    clearTimeout: timer => globalThis.clearTimeout(timer),
  }
  const spawn = runtime.spawn ?? spawnProcess
  const randomUUID = runtime.randomUUID ?? nodeCryptoRandomUUID
  const version = options.version ?? SUPPORTED_VERSION

  const pane = String(options.pane ?? "")
  const session = options.session !== undefined && options.session !== null ? String(options.session) : null
  const pid = options.pid ?? process.pid
  const reporterPath = options.reporter ?? process.env.VERIJ_BIN ?? "verij"

  const projector = runtime.projector ?? new StateProjector({
    version,
    randomUUID,
    conversationId: options.conversationId,
    conversationTitle: options.conversationTitle,
  })

  let activeTuiSession = false
  let child = null
  let childGeneration = 0
  let spawnedConversationId = null
  let connected = false
  let stopped = false
  let shuttingDown = false
  let writing = false
  let lastSnapshot = null
  let lastWriteMs = 0
  let reporterFailures = 0
  let retryAfter = 0

  let currentEpoch = null
  let expectedRevision = 1

  let startupTimeout = null
  let receiptTimeout = null
  let renewalTimeout = null
  let throttleTimeout = null
  let backoffTimeout = null
  let shutdownFlushTimeout = null
  let shutdownGraceTimeout = null
  let shutdownKillTimeout = null

  const env = { ...process.env }
  if (options.runtimeDir) env.VERIJ_AGENT_STATE_DIR ??= options.runtimeDir
  if (options.statesDir) env.VERIJ_STATES_DIR ??= options.statesDir

  const queue = new SnapshotQueue(observationBudget.maxQueueSize, () => {
    projector.invalidateOnOverflow()
    lastSnapshot = projector.getSnapshot()
  })

  const clearChildTimers = () => {
    clock.clearTimeout(startupTimeout)
    startupTimeout = null
    clock.clearTimeout(receiptTimeout)
    receiptTimeout = null
    clock.clearTimeout(renewalTimeout)
    renewalTimeout = null
    clock.clearTimeout(throttleTimeout)
    throttleTimeout = null
    clock.clearTimeout(shutdownFlushTimeout)
    shutdownFlushTimeout = null
  }

  const proceedToEofAndEscalation = () => {
    clock.clearTimeout(shutdownFlushTimeout)
    shutdownFlushTimeout = null
    stopped = true
    shuttingDown = false
    connected = false
    writing = false
    clearChildTimers()
    queue.reset()

    if (child && !child.hasClosed) {
      const recipient = child
      const gen = childGeneration
      try { recipient.stdin.end() } catch {}

      clock.clearTimeout(shutdownGraceTimeout)
      shutdownGraceTimeout = clock.setTimeout(() => {
        if (child === recipient && childGeneration === gen && !recipient.hasClosed) {
          try { recipient.kill("SIGTERM") } catch {}
          clock.clearTimeout(shutdownKillTimeout)
          shutdownKillTimeout = clock.setTimeout(() => {
            if (child === recipient && childGeneration === gen && !recipient.hasClosed) {
              try { recipient.kill("SIGKILL") } catch {}
            }
          }, observationBudget.shutdownWaitMs)
        }
      }, observationBudget.shutdownWaitMs)
    }
  }

  const flush = () => {
    if (!connected || writing || stopped || !child || child.isRetiring) return
    if (child && projector.conversationId && projector.conversationId !== spawnedConversationId) {
      retireChild(child, "SIGTERM")
      return
    }
    if (queue.items.length === 0) return

    const now = clock.now()
    const elapsed = now - lastWriteMs
    if (elapsed < observationBudget.minWriteIntervalMs) {
      if (!throttleTimeout) {
        throttleTimeout = clock.setTimeout(() => {
          throttleTimeout = null
          flush()
        }, observationBudget.minWriteIntervalMs - elapsed)
      }
      return
    }

    const item = queue.shift()
    if (!item) return

    writing = true
    lastWriteMs = clock.now()
    const recipient = child
    const gen = childGeneration

    receiptTimeout = clock.setTimeout(() => {
      if (child === recipient && childGeneration === gen) {
        retireChild(recipient, "SIGKILL")
      }
    }, observationBudget.receiptTimeoutMs)

    try {
      recipient.stdin.write(item + "\n", error => {
        if (error && child === recipient && childGeneration === gen) {
          retireChild(recipient, "SIGTERM")
        }
      })
    } catch {
      if (child === recipient && childGeneration === gen) {
        retireChild(recipient, "SIGTERM")
      }
    }
  }

  const renewLease = () => {
    clock.clearTimeout(renewalTimeout)
    renewalTimeout = clock.setTimeout(() => {
      if (connected && !stopped && !writing && !queue.items.length && lastSnapshot) {
        queue.push(lastSnapshot, true)
        flush()
      }
      if (!stopped) {
        renewLease()
      }
    }, observationBudget.leaseMs)
  }

  const publish = (snapshot, force = false) => {
    if (!snapshot || shuttingDown || stopped) return
    lastSnapshot = snapshot
    queue.push(snapshot, force)
    flush()
  }

  const onChildClosed = (candidate, gen) => {
    if (candidate !== child || gen !== childGeneration) return
    candidate.hasClosed = true

    clearChildTimers()
    clock.clearTimeout(shutdownFlushTimeout)
    shutdownFlushTimeout = null
    clock.clearTimeout(shutdownGraceTimeout)
    shutdownGraceTimeout = null
    clock.clearTimeout(shutdownKillTimeout)
    shutdownKillTimeout = null

    // Recover ambiguous in-flight frame across any close/crash
    queue.requeueInFlight()

    child = null
    spawnedConversationId = null
    connected = false
    writing = false
    currentEpoch = null
    expectedRevision = 1

    if (stopped || shuttingDown) {
      stopped = true
      shuttingDown = false
      queue.reset()
      return
    }

    reporterFailures = Math.min(reporterFailures + 1, 6)
    const backoffMs = Math.min(observationBudget.maxBackoffMs, 2000 * 2 ** (reporterFailures - 1))
    retryAfter = clock.now() + backoffMs
    clock.clearTimeout(backoffTimeout)
    backoffTimeout = clock.setTimeout(() => {
      if (!stopped && !shuttingDown && !child && activeTuiSession) startReporter()
    }, backoffMs)
  }

  const retireChild = (candidate, initialSignal = "SIGTERM") => {
    if (!candidate || candidate !== child) return
    if (candidate.isRetiring) return
    candidate.isRetiring = true

    connected = false
    writing = false
    clearChildTimers()

    if (candidate.hasClosed) {
      onChildClosed(candidate, childGeneration)
      return
    }

    try { candidate.stdin.end() } catch {}
    try { candidate.kill(initialSignal) } catch {}

    const myGen = childGeneration

    const scheduleEscalation = () => {
      clock.clearTimeout(shutdownKillTimeout)
      if (candidate.hasClosed || child !== candidate) return
      shutdownKillTimeout = clock.setTimeout(() => {
        if (child === candidate && childGeneration === myGen && !candidate.hasClosed) {
          try { candidate.kill("SIGKILL") } catch {}
        }
      }, observationBudget.shutdownWaitMs)
    }

    if (initialSignal !== "SIGKILL") {
      clock.clearTimeout(shutdownGraceTimeout)
      if (candidate.hasClosed || child !== candidate) return
      shutdownGraceTimeout = clock.setTimeout(() => {
        if (child === candidate && childGeneration === myGen && !candidate.hasClosed) {
          try { candidate.kill("SIGTERM") } catch {}
          scheduleEscalation()
        }
      }, observationBudget.shutdownWaitMs)
    } else {
      scheduleEscalation()
    }
  }

  const startReporter = () => {
    if (child || stopped || shuttingDown || clock.now() < retryAfter) return
    if (!supportsVersion(version)) return
    if (!isValidU32(pane)) return
    if (!isValidU32(pid)) return
    if (!projector.conversationId) return

    const args = ["agent", "pi"]
    if (session) {
      args.push("--session", session)
    }
    args.push("--pane", pane, "--pid", String(pid))

    let candidate = null
    try {
      candidate = spawn(reporterPath, args, { env, stdio: ["pipe", "pipe", "ignore"] })
    } catch {
      reporterFailures = Math.min(reporterFailures + 1, 6)
      const backoffMs = Math.min(observationBudget.maxBackoffMs, 2000 * 2 ** (reporterFailures - 1))
      retryAfter = clock.now() + backoffMs
      clock.clearTimeout(backoffTimeout)
      backoffTimeout = clock.setTimeout(() => {
        if (!stopped && !shuttingDown && !child && activeTuiSession) startReporter()
      }, backoffMs)
      return
    }

    childGeneration++
    const gen = childGeneration
    candidate.hasExited = false
    candidate.hasClosed = false
    candidate.isRetiring = false
    child = candidate

    let buffer = ""
    let bufferBytes = 0
    const decoder = new StringDecoder("utf8")
    spawnedConversationId = projector.conversationId

    // Attach listeners BEFORE writing hello frame!
    candidate.stdin.on("error", () => retireChild(candidate, "SIGTERM"))
    candidate.stdout.on("error", () => retireChild(candidate, "SIGTERM"))
    candidate.on("error", () => retireChild(candidate, "SIGTERM"))
    candidate.on("exit", () => {
      candidate.hasExited = true
      // On spontaneous exit without separate close event, close handler covers cleanup
      onChildClosed(candidate, gen)
    })
    candidate.on("close", () => onChildClosed(candidate, gen))

    candidate.stdout.on("data", chunk => {
      if (child !== candidate || childGeneration !== gen || stopped) return
      const decoded = decoder.write(chunk)
      buffer += decoded
      bufferBytes += chunk.length

      if (bufferBytes > 262144) {
        retireChild(candidate, "SIGKILL")
        return
      }

      while (buffer.includes("\n")) {
        const idx = buffer.indexOf("\n")
        const line = buffer.slice(0, idx)
        buffer = buffer.slice(idx + 1)
        bufferBytes = Buffer.byteLength(buffer, "utf8")

        let reply
        try {
          reply = JSON.parse(line)
        } catch {
          retireChild(candidate, "SIGTERM")
          return
        }

        if (!reply || typeof reply !== "object" || Array.isArray(reply)) {
          retireChild(candidate, "SIGTERM")
          return
        }

        if (reply.ready !== undefined) {
          // Reject unknown keys
          for (const k of Object.keys(reply)) {
            if (!ALLOWED_READY_KEYS.includes(k)) {
              retireChild(candidate, "SIGTERM")
              return
            }
          }

          if (
            connected ||
            reply.ready !== true ||
            typeof reply.agent_instance_id !== "string" ||
            !sanitizeId(reply.agent_instance_id) ||
            !Number.isSafeInteger(reply.source_epoch) ||
            reply.source_epoch < 1 ||
            reply.source_revision !== 1 ||
            !ALLOWED_STATUSES.includes(reply.status) ||
            typeof reply.restored !== "boolean"
          ) {
            retireChild(candidate, "SIGTERM")
            return
          }

          // Conversation must match hello conversation at spawn
          if (projector.conversationId !== spawnedConversationId) {
            retireChild(candidate, "SIGTERM")
            return
          }

          // If restored: validate latest_snapshot and aggregate_run_id with exact schema
          if (reply.restored) {
            if (reply.aggregate_run_id !== undefined && reply.aggregate_run_id !== null) {
              if (typeof reply.aggregate_run_id !== "string" || !sanitizeId(reply.aggregate_run_id)) {
                retireChild(candidate, "SIGTERM")
                return
              }
            }
            if (reply.latest_snapshot !== undefined && reply.latest_snapshot !== null) {
              if (!validateStrictSnapshot(reply.latest_snapshot, spawnedConversationId)) {
                retireChild(candidate, "SIGTERM")
                return
              }
              if (reply.aggregate_run_id && reply.latest_snapshot.run_id && reply.latest_snapshot.run_id !== reply.aggregate_run_id) {
                retireChild(candidate, "SIGTERM")
                return
              }

              // Check if acknowledged frame matches queued head
              if (queue.items.length > 0) {
                try {
                  const headParsed = JSON.parse(queue.items[0])
                  if (snapshotsSemanticallyEqual(headParsed, reply.latest_snapshot)) {
                    queue.shift()
                    queue.acknowledge()
                  }
                } catch {}
              }
            }
          }

          clock.clearTimeout(startupTimeout)
          startupTimeout = null
          connected = true
          currentEpoch = reply.source_epoch
          expectedRevision = 2

          // Newer observed projector snapshot wins; do not overwrite with stale greeting
          publish(projector.getSnapshot())
          renewLease()
          flush()
        } else {
          // Receipt frame validation with strict whitelist
          for (const k of Object.keys(reply)) {
            if (!ALLOWED_RECEIPT_KEYS.includes(k)) {
              retireChild(candidate, "SIGTERM")
              return
            }
          }

          if (
            !connected ||
            !writing ||
            reply.source_epoch !== currentEpoch ||
            reply.source_revision !== expectedRevision ||
            !Number.isSafeInteger(reply.record_revision) ||
            reply.record_revision <= 0 ||
            !Number.isSafeInteger(reply.completion_revision) ||
            reply.completion_revision < 0 ||
            !ALLOWED_STATUSES.includes(reply.status)
          ) {
            retireChild(candidate, "SIGTERM")
            return
          }

          clock.clearTimeout(receiptTimeout)
          receiptTimeout = null
          writing = false
          expectedRevision++
          queue.acknowledge()
          reporterFailures = 0

          if (shuttingDown) {
            if (queue.items.length === 0 && queue.inFlight === undefined) {
              proceedToEofAndEscalation()
            } else {
              flush()
            }
          } else {
            renewLease()
            flush()
          }
        }
      }
    })

    startupTimeout = clock.setTimeout(() => {
      if (child === candidate && childGeneration === gen) {
        retireChild(candidate, "SIGKILL")
      }
    }, observationBudget.startupTimeoutMs)

    const hello = {
      type: "hello",
      schema_version: PROTOCOL_VERSION,
      version,
      runtime_id: projector.runtimeId,
      conversation_id: spawnedConversationId,
    }

    try {
      candidate.stdin.write(JSON.stringify(hello) + "\n", error => {
        if (error && child === candidate && childGeneration === gen) {
          retireChild(candidate, "SIGTERM")
        }
      })
    } catch {
      if (child === candidate && childGeneration === gen) {
        retireChild(candidate, "SIGTERM")
      }
    }
  }

  const teardown = () => {
    if (stopped || shuttingDown) return
    shuttingDown = true
    activeTuiSession = false
    clock.clearTimeout(backoffTimeout)
    backoffTimeout = null
    clock.clearTimeout(renewalTimeout)
    renewalTimeout = null

    if (!child || child.hasClosed || !connected || (queue.items.length === 0 && queue.inFlight === undefined)) {
      proceedToEofAndEscalation()
      return
    }

    // Bounded nonblocking flush of already-observed semantic frames within total fixed grace (500ms)
    shutdownFlushTimeout = clock.setTimeout(() => {
      proceedToEofAndEscalation()
    }, observationBudget.shutdownWaitMs)

    flush()
  }

  const handleEvent = (type, event = {}, ctx = {}) => {
    try {
      if (type === "session_start") {
        if (!supportsVersion(version)) return
        if (ctx && ctx.mode && ctx.mode !== "tui") return
        if (!isValidU32(pane)) return
        if (!isValidU32(pid)) return

        const rawId = event?.session ?? (ctx?.sessionManager?.getSessionId ? ctx.sessionManager.getSessionId() : undefined) ?? projector.conversationId
        const cleanId = sanitizeId(rawId)
        if (!cleanId) return

        activeTuiSession = true
        projector.handleEvent("session_start", event, ctx)
        startReporter()
        publish(projector.getSnapshot())
        return
      }

      if (!activeTuiSession) return

      if (type === "session_shutdown") {
        activeTuiSession = false
        projector.handleEvent("session_shutdown", event, ctx)
        teardown()
        return
      }

      const snapshot = projector.handleEvent(type, event, ctx)

      if (child && projector.conversationId && projector.conversationId !== spawnedConversationId) {
        retireChild(child, "SIGTERM")
        queue.reset()
        startReporter()
      }

      publish(snapshot)
    } catch {}
  }

  const install = (target = options.pi) => {
    if (!target || typeof target.on !== "function") return

    const events = [
      "session_start", "session_shutdown", "session_info_changed",
      "session_tree", "session_before_compact", "session_compact", "session_compact_failed",
      "before_agent_start", "agent_start", "agent_end", "agent_before_settle", "agent_settled",
      "turn_start", "turn_end", "tool_execution_start", "tool_execution_end",
      "ui_prompt_start", "ui_prompt_end",
    ]

    for (const type of events) {
      target.on(type, (event, ctx) => handleEvent(type, event, ctx))
    }
  }

  return {
    install,
    handleEvent,
    startReporter,
    publish,
    teardown,
    getProjector: () => projector,
    getQueue: () => queue,
    isConnected: () => connected,
    isStopped: () => stopped,
  }
}
