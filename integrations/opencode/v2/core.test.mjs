import test from "node:test"
import assert from "node:assert/strict"
import { LiveState, supportsVersion, unknown } from "./core.mjs"

export function fixture() {
  let route = { type: "session", sessionID: "root" }, now = 0
  const sessions = new Map([["root", { id: "root" }], ["child", { id: "child", parentID: "root" }], ["foreign", { id: "foreign" }]])
  const statuses = new Map(), permissions = new Map(), forms = new Map()
  let family = ["root", "child"]
  const forbidden = new Proxy({}, { get() { throw new Error("forbidden API") } })
  const context = { app: { version: "2.1.2" }, options: {}, ui: { router: { current: () => route } },
    client: forbidden, storage: forbidden,
    data: { session: { get: id => sessions.get(id), family: () => family,
      status: id => statuses.get(id) ?? "idle", sync: forbidden,
      message: forbidden, pending: forbidden,
      permission: { list: id => permissions.get(id), sync: forbidden },
      form: { list: id => forms.get(id), sync: forbidden, reply: forbidden } } } }
  const state = new LiveState(context, () => now)
  return { state, context, sessions, statuses, permissions, forms,
    route: value => { route = value }, family: value => { family = value }, time: value => { now = value },
    event: (type, data) => state.receive({ type, data }) }
}
function noOutcome(value) {
  assert.equal(value.schema_version, 2)
  assert.equal(value.turn_id, null)
  assert.equal(value.completed_at_ms, null)
  assert.equal(value.error_name, null)
}

test("version contract is separate from V1 and rejects banners and malformed versions", () => {
  for (const version of ["2.0.32", "2.1.2", "2.99.42+build.1", "2.1.2-beta.1"]) assert.ok(supportsVersion(version), version)
  for (const version of ["1.19.0", "2.0.31", "3.0.0", "opencode v2.1.2", "2.1", "2.1.2\n", "2.1.2-", "2.999999999999999999999999.0", null]) assert.ok(!supportsVersion(version), version)
})
test("Home is Idle without ownership; selected cache-idle and plugin routes are Unknown", () => {
  const f = fixture()
  const selected = f.state.sample()
  assert.equal(selected.conversation_id, "root"); assert.equal(selected.activity, "unknown"); noOutcome(selected)
  f.route({ type: "home" }); assert.equal(f.state.sample().activity, "idle"); assert.equal(f.state.sample().conversation_id, null)
  f.route({ type: "plugin", data: { sessionID: "foreign" } }); assert.deepEqual(f.state.sample(), unknown("2.1.2"))
})
test("child selection resolves root; active descendants yield Working", () => {
  const f = fixture(); f.route({ type: "session", sessionID: "child" }); f.statuses.set("child", "running")
  const value = f.state.sample()
  assert.equal(value.conversation_id, "root"); assert.equal(value.activity, "working"); assert.equal(value.background_task_count, 1)
  noOutcome(value)
})
test("foreign events cannot assign pane ownership or input", () => {
  const f = fixture(); f.state.sample()
  f.event("session.execution.started", { sessionID: "foreign" })
  f.event("permission.asked", { sessionID: "foreign", id: "private" })
  const value = f.state.sample()
  assert.equal(value.activity, "unknown"); assert.deepEqual(value.pending_requests, []); assert.equal(value.conversation_id, "root")
})
test("live starts survive late cache; all terminal types revoke activity without outcomes", () => {
  for (const type of ["session.execution.succeeded", "session.execution.failed", "session.execution.interrupted"]) {
    const f = fixture(); f.state.sample()
    f.event("session.execution.started", { sessionID: "root" }); assert.equal(f.state.sample().activity, "working")
    f.statuses.set("root", "running"); f.state.sample()
    f.event(type, { sessionID: "root", error: { type: "provider.auth", message: "SECRET", response: { body: "SECRET" } }, reason: "user" })
    const value = f.state.sample(); assert.equal(value.activity, "unknown"); noOutcome(value)
    assert.ok(!JSON.stringify(value).includes("SECRET"))
  }
})
test("permission/form deltas overlay late cache without retaining body data", () => {
  const f = fixture(); f.state.sample()
  f.event("permission.asked", { sessionID: "child", id: "p", metadata: "SECRET" })
  f.event("form.created", { form: { sessionID: "root", id: "f", fields: "SECRET" } })
  assert.deepEqual(f.state.sample().pending_requests, [{ id: "p", kind: "permission" }, { id: "f", kind: "question" }])
  f.permissions.set("child", [{ id: "p", sessionID: "child", detail: "SECRET" }]); f.forms.set("root", [{ id: "f", sessionID: "root" }]); f.state.sample()
  f.event("permission.replied", { sessionID: "child", requestID: "p", answer: "SECRET" })
  f.event("form.cancelled", { sessionID: "root", id: "f" })
  const value = f.state.sample(); assert.deepEqual(value.pending_requests, []); assert.ok(!JSON.stringify(value).includes("SECRET"))
})
test("cache input recovery uses only bounded identities and validates session attribution", () => {
  const f = fixture(); f.permissions.set("root", [{ id: "p", sessionID: "root", patterns: ["SECRET"] }])
  assert.deepEqual(f.state.sample().pending_requests, [{ id: "p", kind: "permission" }])
  f.permissions.set("root", [{ id: "p", sessionID: "foreign" }]); assert.equal(f.state.sample().conversation_id, null)
})
test("route A-B-A and explicit reset fence old activity/input deltas", () => {
  const f = fixture(); f.state.sample(); f.event("permission.asked", { sessionID: "root", id: "p" })
  f.route({ type: "home" }); f.event("session.execution.started", { sessionID: "root" })
  f.route({ type: "session", sessionID: "root" }); assert.deepEqual(f.state.sample().pending_requests, [])
  f.event("session.execution.started", { sessionID: "root" }); assert.equal(f.state.sample().activity, "working")
  f.state.reset(); assert.equal(f.state.sample().activity, "unknown")
})
test("observable connection invalidation clears hints; TTL bounds unreconciled deltas", () => {
  const f = fixture(); f.state.sample(); f.event("session.execution.started", { sessionID: "root" })
  f.event("server.disconnected", {}); assert.equal(f.state.sample().activity, "unknown")
  f.event("permission.asked", { sessionID: "root", id: "p" }); f.time(15001)
  assert.deepEqual(f.state.sample().pending_requests, [])
})
test("malformed/cyclic/missing ancestry and oversized family fail closed", () => {
  for (const change of [
    f => f.sessions.delete("root"),
    f => f.sessions.set("root", { id: "wrong" }),
    f => f.sessions.set("root", { id: "root", parentID: "child" }),
    f => f.family(Array(65).fill("root")),
    f => f.family(["root", "foreign"]),
    f => f.statuses.set("root", "bogus"),
  ]) { const f = fixture(); change(f); assert.deepEqual(f.state.sample(), unknown("2.1.2")) }
})
test("deep ancestry cannot escape traversal bound", () => {
  const f = fixture()
  for (let i = 0; i < 66; i++) f.sessions.set(`s${i}`, { id: `s${i}`, parentID: i < 65 ? `s${i + 1}` : undefined })
  f.route({ type: "session", sessionID: "s0" }); assert.deepEqual(f.state.sample(), unknown("2.1.2"))
})
test("input bounds apply across family and deltas; malformed IDs never persist", () => {
  for (const input of [Array.from({ length: 101 }, (_, i) => ({ id: `p${i}` })), [{ id: "x".repeat(257) }], [{ id: "bad\nID" }]]) {
    const f = fixture(); f.permissions.set("root", input); assert.deepEqual(f.state.sample(), unknown("2.1.2"))
  }
  const f = fixture(); f.state.sample()
  for (let i = 0; i < 101; i++) f.event("permission.asked", { sessionID: "root", id: `p${i}` })
  assert.equal(f.state.inputs.size, 0); assert.deepEqual(f.state.sample().pending_requests, [])
})
test("missing collection hydration cannot produce Done or selected Idle", () => {
  const f = fixture(); f.context.data.session.get = id => ({ id, outcome: "succeeded", time: { idle: 42 } })
  f.family(["root"])
  const value = f.state.sample(); assert.equal(value.activity, "unknown"); noOutcome(value)
})
