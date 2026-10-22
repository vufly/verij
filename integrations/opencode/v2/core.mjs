// Experimental live hints only. No SDK, sync, history, credentials, or mutation.
export const SCHEMA = 2
export const limits = Object.freeze({ family: 64, requests: 100, idBytes: 256, hintMs: 15000 })
export const validID = value => typeof value === "string" && value.length > 0
  && Buffer.byteLength(value) <= limits.idBytes && !/[\x00-\x1f\x7f-\x9f]/u.test(value)
export function supportsVersion(version) {
  if (typeof version !== "string" || version.length > 128) return false
  const match = /^(2)\.(\d+)\.(\d+)(?:[-+][a-zA-Z0-9.+-]+)?$/.exec(version)
  return !!match && Number.isSafeInteger(+match[2]) && Number.isSafeInteger(+match[3])
    && (+match[2] > 0 || +match[3] >= 32)
}
export const unknown = version => ({ schema_version: SCHEMA, version, activity: "unknown",
  conversation_id: null, conversation_title: null, turn_id: null, pending_requests: [],
  background_task_count: 0, completed_at_ms: null, error_name: null })
const terminal = new Set(["session.execution.succeeded", "session.execution.failed", "session.execution.interrupted"])
const connections = new Set(["server.connected", "server.disconnected", "server.instance.disposed"])

export class LiveState {
  constructor(context, now = () => performance.now()) {
    this.context = context; this.version = context.app.version; this.now = now
    this.reset()
  }
  reset() {
    this.route = undefined; this.root = undefined; this.members = new Set()
    this.activity = new Map(); this.inputs = new Map()
  }
  routeID() {
    const route = this.context.ui.router.current()
    if (route?.type === "home") return "home"
    if (route?.type === "session" && validID(route.sessionID)) return `session/${route.sessionID}`
    return "unknown"
  }
  sample() {
    try {
      const route = this.routeID()
      if (route !== this.route) { this.reset(); this.route = route }
      const result = unknown(this.version)
      if (route === "home") return { ...result, activity: "idle" }
      if (!route.startsWith("session/")) return result
      const cache = this.context.data.session
      const roots = new Map()
      const rootOf = id => {
        const seen = new Set()
        let current = id
        while (validID(current) && !seen.has(current) && seen.size < limits.family) {
          if (roots.has(current)) {
            for (const member of seen) roots.set(member, roots.get(current))
            return roots.get(current)
          }
          seen.add(current)
          const session = cache.get(current)
          if (!session || session.id !== current) throw new Error("ancestry")
          if (session.parentID == null) {
            for (const member of seen) roots.set(member, current)
            return current
          }
          if (!validID(session.parentID)) throw new Error("parent")
          current = session.parentID
        }
        throw new Error("cycle_or_bound")
      }
      const selected = route.slice(8), root = rootOf(selected)
      const family = cache.family(root)
      if (!Array.isArray(family) || family.length > limits.family) throw new Error("family_bound")
      const members = new Set([root, selected])
      for (const id of family) {
        if (!validID(id) || rootOf(id) !== root) throw new Error("family_identity")
        members.add(id)
        if (members.size > limits.family) throw new Error("family_bound")
      }
      if (this.root !== root) { this.activity.clear(); this.inputs.clear() }
      this.root = root; this.members = members
      const pending = new Map(), statuses = new Map()
      for (const id of members) {
        const status = cache.status(id)
        if (!["running", "idle"].includes(status)) throw new Error("status")
        statuses.set(id, status === "running")
        for (const [kind, collection] of [["permission", cache.permission], ["question", cache.form]]) {
          const items = collection.list(id)
          if (items === undefined) continue // Missing input cache is not a quiescence proof.
          if (!Array.isArray(items) || items.length > limits.requests) throw new Error("input_bound")
          for (const item of items) {
            if (!validID(item?.id) || (item.sessionID !== undefined && item.sessionID !== id)) throw new Error("input_identity")
            const key = `${kind}/${item.id}`
            if (pending.has(key) && pending.get(key).session !== id) throw new Error("input_collision")
            pending.set(key, { session: id, id: item.id, kind })
            if (pending.size > limits.requests) throw new Error("input_bound")
          }
        }
      }
      for (const [id, change] of this.activity) {
        if (!members.has(id) || statuses.get(id) === change.running || this.now() - change.at > limits.hintMs) this.activity.delete(id)
        else statuses.set(id, change.running)
      }
      for (const [key, change] of this.inputs) {
        if (!members.has(change.session) || pending.has(key) === change.present || this.now() - change.at > limits.hintMs) this.inputs.delete(key)
        else if (change.present) pending.set(key, change)
        else pending.delete(key)
      }
      if (pending.size > limits.requests) throw new Error("input_bound")
      result.conversation_id = root
      result.activity = [...statuses.values()].some(Boolean) ? "working" : "unknown"
      result.background_task_count = [...statuses].filter(([id, running]) => id !== root && running).length
      result.pending_requests = [...pending.values()].map(({ id, kind }) => ({ id, kind }))
        .sort((a, b) => a.kind.localeCompare(b.kind) || a.id.localeCompare(b.id))
      return result
    } catch {
      this.reset()
      return unknown(this.version)
    }
  }
  receive(event) {
    try {
      if (connections.has(event?.type)) { this.reset(); return }
      // A route change revokes the old family even before the next sample.
      if (this.routeID() !== this.route) { this.reset(); return }
      const type = event?.type, data = event?.data
      const session = type === "form.created" ? data?.form?.sessionID : data?.sessionID
      if (!validID(session) || !this.members.has(session)) return
      if (type === "session.deleted") { this.reset(); return }
      if (type === "session.execution.started" || terminal.has(type)) {
        this.activity.set(session, { running: type === "session.execution.started", at: this.now() })
      }
      let kind, id, present
      if (type === "permission.asked") { kind = "permission"; id = data.id; present = true }
      if (type === "permission.replied") { kind = "permission"; id = data.requestID; present = false }
      if (type === "form.created") { kind = "question"; id = data.form.id; present = true }
      if (type === "form.replied" || type === "form.cancelled") { kind = "question"; id = data.id; present = false }
      if (!kind) return
      if (!validID(id)) throw new Error("input_identity")
      const key = `${kind}/${id}`
      if (!this.inputs.has(key) && this.inputs.size >= limits.requests) throw new Error("input_bound")
      this.inputs.set(key, { id, kind, session, present, at: this.now() })
    } catch { this.reset() }
  }
}
