// P0 instrumentation only: bounded, whitelist-only traces, never raw events.
import { createHash } from "node:crypto"
import { appendFileSync, statSync } from "node:fs"

export const EVENTS = Object.freeze([
  "session_start", "session_shutdown", "session_info_changed", "session_before_switch",
  "session_before_fork", "session_before_tree", "session_tree", "session_before_compact",
  "session_compact", "session_compact_failed", "before_agent_start", "agent_start",
  "agent_end", "agent_before_settle", "agent_settled", "turn_start", "turn_end",
  "message_start", "message_end", "tool_execution_start", "tool_execution_end",
  "ui_prompt_start", "ui_prompt_end",
])
export const MARKERS = Object.freeze([
  "observer_loaded", "fixture_loaded", "fixture_ready", "command_start", "command_done",
  "queue_submitted", "continuation_requested", "nested_child_start", "nested_child_end",
  "manual_compact_done", "manual_compact_error", "fixture_error", "postrun_failure_injected",
])
export const CASES = Object.freeze([
  "success", "parallel", "nested", "tool_error", "provider_error", "retry_success",
  "retry_exhausted", "cancel", "queued", "late_queued", "continuation", "auto_compact",
  "ui", "ui_nested", "manual_compact", "compact_error", "name", "new", "resume",
  "fork", "tree", "reload", "quit", "postrun_error", "idle_ui", "continuation_error",
])
const enums = {
  reason: ["startup", "reload", "new", "resume", "fork", "quit", "manual", "threshold", "overflow", "ui_prompt"],
  outcome: ["completed", "aborted", "error"],
  kind: ["select", "confirm", "input", "editor", "custom"],
  role: ["user", "assistant", "toolResult", "custom", "bashExecution"],
  stop_reason: ["stop", "length", "toolUse", "aborted", "error"],
  case: CASES,
  operation: ["run", "compact", "name", "new", "resume", "fork", "tree", "reload", "quit", "idle_ui"],
  tool: ["probe_leaf", "probe_nested", "probe_queue", "probe_ui", "probe_ui_nested"],
  mode: ["tui", "print", "json", "rpc"],
  engine: ["bun", "node"],
}
export const BOOLEAN_FIELDS = Object.freeze([
  "idle", "pending", "signal_aborted", "aborted", "will_retry", "from_extension",
  "continue_proposed", "can_continue", "is_error", "error_present", "name_present",
])
export const HASH_FIELDS = Object.freeze(["session", "call", "parent_call", "entry"])
export function opaque(value) {
  return typeof value === "string" && value.length ? createHash("sha256").update(value).digest("hex").slice(0, 20) : undefined
}

// Unknown fields are discarded, even if they look like harmless metadata.
export function project(type, source, runtime, values = {}) {
  if (![...EVENTS, ...MARKERS].includes(type) || !["observer", "fixture"].includes(source)
    || !/^[a-f0-9]{20}$/.test(runtime)) throw new Error("invalid trace identity")
  const record = { schema_version: 1, source, runtime, event: type }
  for (const [name, allowed] of Object.entries(enums)) if (allowed.includes(values[name])) record[name] = values[name]
  for (const name of BOOLEAN_FIELDS) if (typeof values[name] === "boolean") record[name] = values[name]
  for (const name of HASH_FIELDS) if (typeof values[name] === "string" && /^[a-f0-9]{20}$/.test(values[name])) record[name] = values[name]
  for (const name of ["pid", "turn_index", "tool_count", "at_ms"]) {
    const n = values[name]
    if (Number.isSafeInteger(n) && n >= 0) record[name] = n
  }
  if (values.version === "1.2.0") record.version = values.version
  if (typeof values.engine_version === "string" && /^\d+\.\d+\.\d+$/.test(values.engine_version)) record.engine_version = values.engine_version
  return record
}

export function contextMetadata(ctx) {
  if (!ctx) return {}
  return {
    session: opaque(ctx.sessionManager.getSessionId()), mode: ctx.mode,
    idle: ctx.isIdle(), pending: ctx.hasPendingMessages(), signal_aborted: ctx.signal?.aborted,
    name_present: !!ctx.sessionManager.getSessionName(),
  }
}

export function eventMetadata(event) {
  const message = event.message
  return {
    reason: event.reason, outcome: event.outcome, kind: event.kind, aborted: event.aborted,
    will_retry: event.willRetry, from_extension: event.fromExtension,
    continue_proposed: event.continue, can_continue: event.context?.canContinue,
    turn_index: event.turnIndex, call: opaque(event.toolCallId), parent_call: opaque(event.parentToolCallId),
    entry: opaque(event.messageEntryId), tool: event.toolName, is_error: event.isError,
    role: message?.role, stop_reason: message?.role === "assistant" ? message.stopReason : undefined,
    tool_count: message?.role === "assistant" ? message.content?.filter(c => c.type === "toolCall").length : undefined,
    error_present: event.errorMessage !== undefined || message?.errorMessage !== undefined,
  }
}

export function recorder(path, source, runtime, version) {
  let disabled = false
  return (type, values = {}) => {
    if (disabled || !path) return
    // Synchronous bounded append is intentional P0 instrumentation, not production transport.
    try {
      let size = 0
      try { size = statSync(path).size } catch (error) { if (error.code !== "ENOENT") throw error }
      if (size >= 1024 * 1024) { disabled = true; return }
      const record = project(type, source, runtime, { ...values, version, pid: process.pid, at_ms: Math.round(performance.now()),
        engine: process.versions.bun ? "bun" : "node", engine_version: process.versions.bun ?? process.versions.node })
      appendFileSync(path, JSON.stringify(record) + "\n", { mode: 0o600 })
    } catch { disabled = true } // A recorder failure cannot throw from a Pi handler.
  }
}
