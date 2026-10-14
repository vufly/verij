// Metadata projection for the OpenCode 1.x TUI contract.
// No prompts, message text, tool arguments/output or error bodies leave this module.
export function supportsVersion(version) {
  return typeof version === "string" && /^1\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.+-]+)?$/.test(version)
}
export const events = ["session.created", "session.updated", "session.deleted", "session.status",
  "session.idle", "session.error", "message.updated", "message.removed", "message.part.updated",
  "permission.asked", "permission.replied", "question.asked", "question.replied", "question.rejected",
  "server.instance.disposed"]

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
  // Explicit full status response + full messages response establishes idle,
  // including empty sessions. api.state.ready/null caches alone never do.
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

// All requests are read-only. Complete snapshots repair startup hydration,
// missed broadcasts, child ancestry and permission/question reply ordering.
export async function reconcile(api, signal) {
  const selected = api.route.current
  if (!api.state.ready) throw new Error("Not ready")
  if (selected.name === "home") return null
  if (selected.name !== "session" || !selected.params?.sessionID) throw new Error("Unowned route")
  const client = api.client
  const options = { signal, throwOnError: true }
  const before = data(await client.session.status({}, options))
  let root = data(await client.session.get({ sessionID: selected.params.sessionID }, options))
  const ancestors = new Set([root.id])
  while (root.parentID) {
    if (ancestors.has(root.parentID) || ancestors.size > 64) throw new Error("Invalid ancestry")
    ancestors.add(root.parentID)
    root = data(await client.session.get({ sessionID: root.parentID }, options))
  }
  const members = [root]
  const seen = new Set([root.id])
  for (let index = 0; index < members.length; index++) {
    const parent = members[index]
    for (const child of data(await client.session.children({ sessionID: parent.id }, options))) {
      if (child.parentID !== parent.id || seen.has(child.id)) throw new Error("Invalid child ancestry")
      seen.add(child.id)
      if (seen.size > 64) throw new Error("Family exceeds bound")
      members.push(child)
    }
  }
  const [messages, permissions, questions, statuses] = await Promise.all([
    client.session.messages({ sessionID: root.id, limit: 100 }, options).then(data),
    client.permission.list({}, options).then(data),
    client.question.list({}, options).then(data),
    client.session.status({}, options).then(data),
  ])
  for (const parent of members) {
    const children = data(await client.session.children({ sessionID: parent.id }, options))
    const old = members.filter(s => s.parentID === parent.id).map(s => s.id).sort()
    if (JSON.stringify(children.map(s => s.id).sort()) !== JSON.stringify(old)) throw new Error("Family changed")
    if (JSON.stringify(before[parent.id] ?? { type: "idle" }) !== JSON.stringify(statuses[parent.id] ?? { type: "idle" })) {
      throw new Error("Activity changed during observation")
    }
  }
  const current = api.route.current
  if (current.name !== selected.name || current.params?.sessionID !== selected.params.sessionID) throw new Error("Route changed")
  return { root, members, messages, permissions, questions, statuses }
}

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
  for (const [namespace, methods] of Object.entries({ session: ["status", "get", "children", "messages"],
    permission: ["list"], question: ["list"] })) {
    client[namespace] = {}
    for (const method of methods) {
      client[namespace][method] = (args, options = {}) => budget.call(signal =>
        api.client[namespace][method](args, { ...options, signal }), options.signal)
    }
  }
  const observed = Object.create(api)
  Object.defineProperty(observed, "client", { value: client })
  return observed
}
