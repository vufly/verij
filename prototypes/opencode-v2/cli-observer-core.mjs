// P0 observation only: no SDK/cache reads, navigation, replies, or reporter.
import { createHash } from "node:crypto"

export const EVENTS = Object.freeze([
  "session.execution.started", "session.execution.succeeded",
  "session.execution.failed", "session.execution.interrupted",
])
const hash = value => {
  if (typeof value !== "string" || !value.length || value.length > 256) throw new Error("identity_bound")
  return createHash("sha256").update(value).digest("hex").slice(0, 20)
}

// Opt-in fixed projection for the separate terminal-payload experiment. Never
// retain arbitrary upstream error types, messages, status bodies, or envelopes.
const ERROR_TYPES = new Set([
  "provider.rate-limit", "provider.auth", "provider.quota", "provider.content-filter",
  "provider.transport", "provider.internal", "provider.invalid-output", "provider.invalid-request",
  "provider.unsupported-operation", "provider.no-route", "provider.unknown", "provider.timeout",
  "permission.rejected", "tool.execution", "unknown",
])
export function terminalMetadata(value) {
  let error_type = null, interruption_reason = null
  if (value.type === "session.execution.failed") {
    const type = value.data?.error?.type
    if (typeof type !== "string" || !type.length || type.length > 128) throw new Error("error_schema")
    error_type = ERROR_TYPES.has(type) ? type : "other"
  }
  if (value.type === "session.execution.interrupted") {
    const reason = value.data?.reason
    if (!["user", "shutdown", "superseded", "inactivity"].includes(reason)) throw new Error("reason_schema")
    interruption_reason = reason
  }
  return { error_type, interruption_reason }
}

export function observe(context, emit, { clock = globalThis, projectEvent = () => ({}) } = {}) {
  const generation = context.options.generation
  if (![1, 2, 3].includes(generation)) throw new Error("fixture_generation")
  let disposed = false
  let failed = false
  let lastRoute
  let index = 0
  const frame = (kind, fields = {}) => {
    if (index >= 96) return
    // Reserve a fault and cleanup slot; an event storm must not crash the CLI.
    if (index >= 94 && kind !== "fault" && kind !== "cleanup") return fault("frame_bound")
    try { emit({ kind, generation, index: index++, ...fields }) } catch { failed = true }
  }
  const fault = reason => {
    if (failed) return
    failed = true
    frame("fault", { reason })
  }
  const safe = callback => {
    try { callback() } catch { fault("schema") }
  }
  const route = () => {
    const value = context.ui.router.current()
    const type = ["home", "session", "plugin"].includes(value?.type) ? value.type : "unknown"
    return { route: type, selected: type === "session" ? hash(value.sessionID) : null }
  }
  const observeRoute = () => {
    if (disposed || failed) return
    const value = route()
    const key = JSON.stringify(value)
    if (key === lastRoute) return
    lastRoute = key
    frame("route", value)
  }
  const event = (channel, value) => {
    if (disposed || failed || !EVENTS.includes(value?.type)) return
    const session = hash(value.data?.sessionID)
    const durable = value.durable
    if (!durable || durable.aggregateID !== value.data.sessionID || !Number.isSafeInteger(durable.seq) || durable.seq < 0 || durable.version !== 1)
      throw new Error("durable_schema")
    observeRoute()
    if (failed) return
    const metadata = projectEvent(value)
    frame("event", { ...metadata, channel, event: value.type, session, event_id: hash(value.id),
      aggregate: hash(durable.aggregateID), seq: durable.seq, version: durable.version, ...route() })
  }
  frame("setup", { version_matches: context.app.version === "2.0.32",
    options_match: context.options.fixture === "verij-v2-cli" })
  const subscriptions = []
  try {
    for (const type of EVENTS) subscriptions.push(context.data.on(type, value => safe(() => event("typed", value))))
    subscriptions.push(context.data.listen(value => safe(() => event("envelope", value?.details))))
  } catch {
    fault("subscription")
    for (const unsubscribe of subscriptions.splice(0)) safe(unsubscribe)
  }
  safe(observeRoute)
  const timer = clock.setInterval(() => safe(observeRoute), 100)
  return () => {
    if (disposed) return
    disposed = true
    safe(() => clock.clearInterval(timer))
    for (const unsubscribe of subscriptions) safe(unsubscribe)
    frame("cleanup")
  }
}
