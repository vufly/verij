// Metadata projection for the verified OpenCode 1.18.x TUI contract.
// No prompts, message text, tool arguments/output or error bodies leave this module.
export const versions = ["1.18.32", "1.18.33", "1.18.34"]
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
  constructor(limit = 128) { this.limit = limit; this.items = []; this.overflow = false }
  push(value) {
    const encoded = JSON.stringify(value)
    if (encoded === this.items.at(-1)) return
    if (this.items.length >= this.limit) {
      this.items.length = 0
      this.overflow = true
      this.items.push(JSON.stringify(unknown(value.version)))
      return
    }
    this.items.push(encoded)
  }
  shift() { return this.items.shift() }
}
