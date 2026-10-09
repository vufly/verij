import test from "node:test"
import assert from "node:assert/strict"
import { normalize, reconcile, SnapshotQueue } from "./core.mjs"

const version = "1.18.34"
const family = () => ({ root: { id: "root", title: "Title" }, members: [{ id: "root" }, { id: "child", parentID: "root" }],
  messages: [{ info: { id: "user", role: "user" } }, { info: { id: "assistant", parentID: "user", role: "assistant",
    finish: "stop", time: { completed: 42 } } }], permissions: [], questions: [], statuses: {} })

test("only current root outcome and quiescent family establish completion", () => {
  const f = family()
  assert.equal(normalize(version, f).completed_at_ms, 42)
  f.statuses.child = { type: "busy" }
  assert.equal(normalize(version, f).activity, "working")
  assert.equal(normalize(version, f).completed_at_ms, undefined)
  f.statuses = {}
  f.messages[1].info.finish = "tool-calls"
  assert.equal(normalize(version, f).completed_at_ms, undefined)
  f.messages.push({ info: { id: "new-user", role: "user" } })
  assert.equal(normalize(version, f).activity, "unknown")
  assert.equal(normalize(version, f).turn_id, "root/new-user")
})

test("shared-server permission/question isolation and multiple pending requests", () => {
  const f = family()
  f.permissions = [{ id: "foreign", sessionID: "another-tui", metadata: { secret: "never-forward" } },
    { id: "permission", sessionID: "child" }]
  f.questions = [{ id: "question", sessionID: "root", questions: [{ question: "never-forward" }] }]
  const snapshot = normalize(version, f)
  assert.deepEqual(snapshot.pending_requests, [{ id: "permission", kind: "permission" }, { id: "question", kind: "question" }])
  assert.equal(snapshot.completed_at_ms, undefined)
  assert.ok(!JSON.stringify(snapshot).includes("never-forward"))
  f.permissions = []; f.questions = []
  assert.equal(normalize(version, f).completed_at_ms, 42)
})

test("retry, abort and recoverable tool failure never manufacture success", () => {
  const f = family()
  f.statuses.root = { type: "retry" }
  assert.equal(normalize(version, f).activity, "retrying")
  f.statuses = {}
  f.messages[1].info.error = { name: "MessageAbortedError", data: { message: "private error" } }
  assert.equal(normalize(version, f).error_name, "MessageAbortedError")
  assert.equal(normalize(version, f).completed_at_ms, undefined)
  delete f.messages[1].info.error
  f.messages[1].parts = [{ type: "tool", state: { status: "error", error: "private tool output" } }]
  assert.equal(normalize(version, f).completed_at_ms, 42)
})

test("full query recovers idle despite ready preceding cache hydration", async () => {
  const result = value => Promise.resolve({ data: value })
  const api = { state: { ready: true }, route: { current: { name: "session", params: { sessionID: "root" } } },
    client: { session: { get: () => result({ id: "root" }), children: () => result([]), messages: () => result([]), status: () => result({}) },
      permission: { list: () => result([]) }, question: { list: () => result([]) } } }
  assert.equal(normalize(version, await reconcile(api)).activity, "idle")
  api.client.session.messages = () => Promise.resolve({ error: "unavailable" })
  await assert.rejects(reconcile(api))
  api.route.current = { name: "home" }
  assert.equal(await reconcile(api), null)
  assert.equal(normalize(version, null).turn_id, undefined)
})

test("cyclic or foreign ancestry refuses snapshots", async () => {
  const api = { state: { ready: true }, route: { current: { name: "session", params: { sessionID: "root" } } },
    client: { session: { status: async () => ({ data: {} }), get: async () => ({ data: { id: "root", parentID: "root" } }) } } }
  await assert.rejects(reconcile(api), /ancestry/)
})

test("queue preserves distinct edges and overflow invalidates authority", () => {
  const queue = new SnapshotQueue(3)
  for (const activity of ["working", "idle", "working"]) queue.push({ version, activity })
  assert.equal(queue.items.length, 3)
  queue.push({ version, activity: "idle" })
  assert.equal(queue.overflow, true)
  assert.equal(JSON.parse(queue.shift()).activity, "unknown")
})

test("activity changing across a recovery observation stays uncertain", async () => {
  let calls = 0
  const result = data => Promise.resolve({ data })
  const api = { state: { ready: true }, route: { current: { name: "session", params: { sessionID: "root" } } },
    client: { session: { get: () => result({ id: "root" }), children: () => result([]), messages: () => result([]),
      status: () => result(calls++ === 0 ? { root: { type: "busy" } } : {}) },
      permission: { list: () => result([]) }, question: { list: () => result([]) } } }
  await assert.rejects(reconcile(api), /Activity changed/)
})
