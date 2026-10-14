import test from "node:test"
import assert from "node:assert/strict"
import { EventEmitter } from "node:events"
import { createBridge } from "./bridge.mjs"
import { fixture } from "../../prototypes/resource-usage/fixture.mjs"
import { normalize, reconcile, ObservationRecovery, supportsVersion, SnapshotQueue, ReconcileScheduler,
  RequestBudget, budgetedAPI, observationBudget } from "./core.mjs"

const version = "1.18.34"
const family = () => ({ root: { id: "root", title: "Title" }, members: [{ id: "root" }, { id: "child", parentID: "root" }],
  messages: [{ info: { id: "user", role: "user" } }, { info: { id: "assistant", parentID: "user", role: "assistant",
    finish: "stop", time: { completed: 42 } } }], permissions: [], questions: [], statuses: {} })

test("OpenCode 1.x minor and patch upgrades stay supported", () => {
  for (const version of ["1.0.0", "1.18.32", "1.18.34", "1.18.35", "1.19.0", "1.99.999", "1.19.0-beta.1", "1.19.0+build.42"]) {
    assert.equal(supportsVersion(version), true, version)
  }
  for (const version of ["0.18.34", "2.0.0", "10.0.0", "1.18", "1.x.x", "1.18.35-", "1.18.35+", "1.18.35.0", "", undefined]) {
    assert.equal(supportsVersion(version), false, String(version))
  }
})

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

const flush = async () => { for (let n = 0; n < 200; n++) await Promise.resolve() }
class Clock {
  time = 0
  next = 0
  tasks = new Map()
  created = 0
  now = () => this.time
  setTimeout = (fn, ms) => {
    this.created++
    const id = ++this.next
    this.tasks.set(id, { fn, at: this.time + ms })
    return id
  }
  clearTimeout = id => this.tasks.delete(id)
  async advance(ms) {
    const until = this.time + ms
    await flush()
    for (;;) {
      const next = [...this.tasks].sort((a, b) => a[1].at - b[1].at)[0]
      if (!next || next[1].at > until) break
      this.time = next[1].at
      this.tasks.delete(next[0]); next[1].fn()
      await flush()
    }
    this.time = until
    await flush()
  }
}

test("stream frequency cannot multiply refreshes, timers or dirty work", async () => {
  const clock = new Clock()
  let calls = 0
  const scheduler = new ReconcileScheduler(async () => { calls++; return true }, () => true, clock)
  scheduler.invalidate()
  const firstTimer = clock.created
  for (let n = 0; n < 10_000; n++) scheduler.invalidate()
  assert.equal(clock.created, firstTimer)
  assert.equal(clock.tasks.size, 1)
  await clock.advance(0)
  assert.equal(calls, 1)
  for (let n = 0; n < 1_000; n++) { scheduler.invalidate(); await clock.advance(1) }
  assert.equal(calls, 2)
  assert.equal(clock.tasks.size, 1)
  scheduler.dispose()
  assert.equal(clock.tasks.size, 0)
})

test("event during collection survives rejected sample as exactly one later retry", async () => {
  const clock = new Clock()
  let finish, calls = 0
  const scheduler = new ReconcileScheduler(() => {
    calls++
    return new Promise(resolve => { finish = resolve })
  }, () => false, clock)
  scheduler.invalidate()
  await clock.advance(0)
  assert.equal(scheduler.inFlight, true)
  for (let n = 0; n < 10_000; n++) scheduler.invalidate()
  assert.equal(clock.tasks.size, 0)
  await clock.advance(400)
  finish(false); await flush()
  assert.equal(scheduler.dirty, true)
  await clock.advance(599)
  assert.equal(calls, 1)
  await clock.advance(1)
  assert.equal(calls, 2)
  finish(true); await flush()
  assert.equal(scheduler.dirty, false)
  await clock.advance(9999)
  assert.equal(calls, 2)
  scheduler.dispose()
})

test("quiet idle recovery and API failures have independent bounded deadlines", async () => {
  const clock = new Clock()
  let calls = 0, fail = false
  const scheduler = new ReconcileScheduler(async () => {
    calls++
    if (fail) throw Error("offline")
    return true
  }, () => false, clock)
  scheduler.invalidate()
  await clock.advance(0)
  await clock.advance(9999)
  assert.equal(calls, 1)
  await clock.advance(1)
  assert.equal(calls, 2)
  fail = true; scheduler.invalidate()
  await clock.advance(1000)
  assert.equal(calls, 3)
  await clock.advance(1000)
  assert.equal(calls, 4)
  for (let n = 0; n < 1000; n++) scheduler.invalidate()
  await clock.advance(1999)
  assert.equal(calls, 4)
  await clock.advance(1)
  assert.equal(calls, 5)
  scheduler.invalidate({ resetBackoff: true })
  fail = false
  await clock.advance(1000)
  assert.equal(calls, 6)
  scheduler.dispose()
})

test("disposing in-flight scheduler prevents retry or recovery timer revival", async () => {
  const clock = new Clock()
  let finish
  const scheduler = new ReconcileScheduler(() => new Promise(resolve => { finish = resolve }), () => true, clock)
  scheduler.invalidate(); await clock.advance(0)
  scheduler.invalidate(); scheduler.dispose()
  finish(false); await flush()
  assert.equal(clock.tasks.size, 0)
  assert.equal(scheduler.closed, true)
})

test("request budget charges each actual endpoint, bounds burst, concurrency and pending work", async () => {
  const clock = new Clock()
  const budget = new RequestBudget(clock)
  let active = 0, peak = 0
  const calls = []
  const invoke = () => new Promise(resolve => {
    active++; peak = Math.max(peak, active)
    calls.push(() => { active--; resolve({ data: [] }) })
  })
  const promises = Array.from({ length: 8 }, () => budget.call(invoke))
  await assert.rejects(budget.call(invoke), /queue full/)
  assert.equal(budget.active, observationBudget.concurrency)
  assert.equal(budget.waiting.length, observationBudget.pendingRequests)
  for (let n = 0; n < 8; n++) { calls[n](); await flush() }
  await Promise.all(promises)
  assert.equal(peak, 4)
  assert.equal(budget.calls, 8)
  const extra = budget.call(async () => ({ data: [] }))
  await clock.advance(30)
  assert.equal(budget.calls, 8)
  await clock.advance(2)
  await extra
  assert.equal(budget.calls, 9)
  budget.dispose()
  assert.equal(clock.tasks.size, 0)
})

test("sustained fan-out cannot exceed token envelope and failures are still charged", async () => {
  const clock = new Clock(), budget = new RequestBudget(clock)
  let observed = 0
  const work = async () => {
    for (let n = 0; n < 72; n++) {
      try {
        await budget.call(async () => { observed++; if (n % 2) throw Error("failed HTTP request"); return { data: {} } })
      } catch { /* Failed invocations consume exactly the same budget. */ }
    }
  }
  const running = work()
  await clock.advance(1000)
  assert.equal(observed, 40)
  await clock.advance(1000)
  await running
  assert.equal(observed, 72)
  assert.equal(budget.calls, 72)
  budget.dispose()
})

test("cancelled physical request retains slot and queued cancellation never invokes endpoint", async () => {
  const clock = new Clock()
  const budget = new RequestBudget(clock)
  const controller = new AbortController()
  const responses = []
  const invoke = () => new Promise(resolve => responses.push(resolve))
  const promises = Array.from({ length: 8 }, () => budget.call(invoke, controller.signal))
  const rejected = promises.map(p => assert.rejects(p, /cancelled/))
  controller.abort()
  await Promise.all(rejected)
  assert.equal(budget.active, 4)
  assert.equal(budget.waiting.length, 0)
  assert.equal(budget.calls, 4)
  const next = budget.call(async () => ({ data: {} }))
  assert.equal(budget.calls, 4)
  responses.forEach(resolve => resolve({ data: {} })); await flush(); await next
  assert.equal(budget.calls, 5)
  budget.dispose()
  await assert.rejects(budget.call(invoke), /cancelled/)
})

test("budget wrapper retains read-only SDK options and charges family fan-out", async () => {
  const result = data => Promise.resolve({ data })
  const clock = new Clock(), budget = new RequestBudget(clock)
  const api = { state: { ready: true }, route: { current: { name: "session", params: { sessionID: "root" } } },
    client: { session: { get: () => result({ id: "root" }), children: () => result([]), messages: () => result([]),
      status: (_args, options) => { assert.equal(options.throwOnError, true); assert.ok(options.signal); return result({}) } },
    permission: { list: () => result([]) }, question: { list: () => result([]) } } }
  Object.freeze(api)
  const collecting = reconcile(budgetedAPI(api, budget))
  await clock.advance(1000)
  assert.equal(normalize(version, await collecting).activity, "idle")
  assert.equal(budget.calls, 9)
  budget.dispose()
})

test("successfully delivered semantic observations dedupe but lease and distinct edges remain ordered", () => {
  const queue = new SnapshotQueue()
  const working = { version, activity: "working" }, permission = { ...working, pending_requests: [{ id: "p", kind: "permission" }] }
  queue.push(working); queue.shift()
  queue.push(working); assert.equal(queue.items.length, 0)
  queue.acknowledge()
  queue.push(working); assert.equal(queue.items.length, 0)
  queue.push(permission); queue.push(working)
  assert.equal(queue.items.length, 2)
  assert.deepEqual(JSON.parse(queue.shift()).pending_requests, permission.pending_requests)
  queue.acknowledge(); queue.shift(); queue.acknowledge()
  queue.push(working, true)
  assert.equal(queue.items.length, 1)
  queue.reset(); queue.push(working)
  assert.equal(queue.items.length, 1)
})

function bridgeFixture() {
  const clock = new Clock(), intervals = new Map(), children = []
  const runtime = { clock,
    setInterval: (fn, ms) => {
      const id = Symbol()
      const tick = () => {
        if (!intervals.has(id)) return
        fn()
        if (intervals.has(id)) intervals.set(id, clock.setTimeout(tick, ms))
      }
      intervals.set(id, clock.setTimeout(tick, ms))
      return id
    },
    clearInterval: id => { clock.clearTimeout(intervals.get(id)); intervals.delete(id) },
    spawn: () => {
      const child = new EventEmitter()
      child.stdout = new EventEmitter(); child.stdin = new EventEmitter(); child.frames = []
      let revision = 0, exited = false
      child.kill = () => {
        if (exited) return
        exited = true
        clock.setTimeout(() => child.emit("exit", 0), 0)
      }
      child.stdin.end = child.kill
      child.stdin.write = (frame, callback) => {
        child.frames.push(JSON.parse(frame)); callback()
        clock.setTimeout(() => child.stdout.emit("data", Buffer.from(JSON.stringify({ record_revision: ++revision }) + "\n")), 0)
      }
      clock.setTimeout(() => child.stdout.emit("data", Buffer.from('{"ready":true}\n')), 0)
      children.push(child)
      return child
    },
  }
  const subject = fixture()
  process.env.ZELLIJ_PANE_ID = "0"; process.env.ZELLIJ_SESSION_NAME = "bridge-test"
  const start = () => createBridge(runtime).tui(subject.api, { reporter: "/fake/reporter" })
  const dispose = async () => {
    const closing = subject.api.dispose()
    await clock.advance(0); await closing
    assert.equal(clock.tasks.size, 0)
    assert.equal(intervals.size, 0)
    assert.equal(subject.listeners.size, 0)
  }
  return { clock, subject, children, start, dispose }
}

test("real bridge dedupes recovery receipts and renews idle source without API work", async () => {
  const probe = bridgeFixture()
  await probe.start(); await probe.clock.advance(1500)
  const child = probe.children[0], frames = child.frames.length, calls = probe.subject.count()
  assert.equal(child.frames.at(-1).activity, "idle")
  await probe.clock.advance(4600) // Includes token-bucket delay before the idle receipt.
  assert.equal(probe.subject.count(), calls)
  assert.equal(child.frames.length, frames + 1)
  assert.deepEqual(child.frames.at(-1), child.frames.at(-2))
  await probe.clock.advance(5000) // Periodic recovery and independent renewal coincide.
  assert.equal(probe.subject.count(), calls + 9) // Ten-second eligibility starts before API latency.
  assert.equal(child.frames.length, frames + 2)
  await probe.dispose()
})

test("real bridge keeps explicit question open/reply edges ordered through coalescing", async () => {
  const probe = bridgeFixture()
  probe.subject.activity("busy")
  await probe.start(); await probe.clock.advance(1500)
  const child = probe.children[0]
  probe.subject.listeners.get("question.asked")({ properties: { id: "q", sessionID: "root" } })
  await probe.clock.advance(0)
  assert.deepEqual(child.frames.at(-1).pending_requests, [{ id: "q", kind: "question" }])
  const frames = child.frames.length
  for (let n = 0; n < 1000; n++) {
    probe.subject.listeners.get("message.part.updated")({ properties: { part: { sessionID: "root" } } })
  }
  assert.equal(child.frames.length, frames)
  probe.subject.listeners.get("message.updated")({ properties: { info: { id: "user-1", role: "user", sessionID: "root" } } })
  await probe.clock.advance(0)
  assert.deepEqual(child.frames.at(-1).pending_requests, [{ id: "q", kind: "question" }])
  assert.equal(child.frames.at(-1).turn_id, "root/user-1")
  assert.equal(child.frames.length, frames + 1)
  probe.subject.listeners.get("question.replied")({ properties: { requestID: "q", sessionID: "root" } })
  await probe.clock.advance(0)
  assert.deepEqual(child.frames.at(-1).pending_requests, [])
  assert.equal(child.frames.at(-1).completed_at_ms, undefined)
  assert.equal(child.frames.length, frames + 2)
  await probe.dispose()
})

test("reporter reconnect publishes conservative presence before recovering completion", async () => {
  const probe = bridgeFixture()
  probe.subject.finish()
  await probe.start(); await probe.clock.advance(1500)
  assert.ok(probe.children[0].frames.at(-1).completed_at_ms)
  probe.children[0].kill(); await probe.clock.advance(0)
  await probe.clock.advance(2500)
  assert.equal(probe.children.length, 2)
  assert.equal(probe.children[1].frames[0].activity, "unknown")
  assert.equal(probe.children[1].frames[0].completed_at_ms, undefined)
  await probe.clock.advance(1500)
  assert.ok(probe.children[1].frames.at(-1).completed_at_ms)
  await probe.dispose()
})

test("malformed native receipts only retire reporter and disposal cancels all timers", async () => {
  const probe = bridgeFixture()
  await probe.start(); await probe.clock.advance(1500)
  assert.doesNotThrow(() => probe.children[0].stdout.emit("data", Buffer.from("null\n")))
  await probe.clock.advance(0)
  assert.equal(probe.subject.listeners.size > 0, true)
  await probe.dispose()
})

test("input ingress beyond supported request bound degrades without oversized native frames", async () => {
  const probe = bridgeFixture()
  await probe.start(); await probe.clock.advance(1500)
  for (let n = 0; n < 101; n++) {
    probe.subject.listeners.get("permission.asked")({ properties: { id: `p-${n}`, sessionID: "root" } })
  }
  await probe.clock.advance(0)
  assert.equal(probe.children[0].frames.at(-1).activity, "unknown")
  assert.ok(probe.children[0].frames.every(frame => frame.pending_requests.length <= 100))
  await probe.dispose()
})

test("validated family cache separates status recovery from ancestry and tool history", async () => {
  const clock = new Clock(), recovery = new ObservationRecovery(clock), subject = fixture(16)
  let result
  for (let slice = 0; slice < 3; slice++) result = await recovery.collect(subject.api)
  assert.equal(result.members.length, 16)
  const topologyCalls = subject.calls["session.children"] + subject.calls["session.get"]
  const historyCalls = subject.calls["session.messages"]
  subject.activity("busy")
  for (let n = 0; n < 9; n++) {
    await clock.advance(1000)
    const before = subject.count()
    result = await recovery.collect(subject.api)
    assert.equal(subject.count() - before, 4)
    assert.equal(normalize(version, result).activity, "working")
    assert.equal(result.messagesComplete, false)
  }
  assert.equal(subject.calls["session.children"] + subject.calls["session.get"], topologyCalls)
  assert.equal(subject.calls["session.messages"], historyCalls)
  assert.ok(recovery.family.members.every(s => Object.keys(s).every(k => ["id", "parentID", "title"].includes(k))))
})

for (const count of [1, 16, 64]) {
  test(`${count}-member family resumes within bounded slices and token/concurrency envelope`, async () => {
    const clock = new Clock(), budget = new RequestBudget(clock), recovery = new ObservationRecovery(clock)
    const subject = fixture(count), api = budgetedAPI(subject.api, budget)
    subject.finish()
    let result, slices = 0
    while (!result && slices < 12) {
      const before = subject.count(), collecting = recovery.collect(api)
      assert.ok(budget.active <= 4); assert.ok(budget.waiting.length <= 4)
      await clock.advance(1000)
      result = await collecting; slices++
      assert.ok(subject.count() - before <= observationBudget.topologySliceCalls + 5)
      assert.ok(budget.calls <= 8 + 32 * clock.now() / 1000)
      if (!result) assert.equal(recovery.family, undefined)
    }
    assert.equal(result.members.length, count)
    assert.ok(normalize(version, result).completed_at_ms)
    assert.equal(subject.calls["session.children"], count * 2)
    assert.equal(subject.calls["session.messages"], 3)
    budget.dispose()
    assert.equal(clock.tasks.size, 0)
  })
}

test("topology invalidation retires partial scans while unrelated stream events do not", async () => {
  const clock = new Clock(), recovery = new ObservationRecovery(clock), subject = fixture(64)
  assert.equal(await recovery.collect(subject.api), undefined)
  assert.equal(recovery.topology("session.created", { id: "foreign", parentID: "other" }), false)
  assert.equal(recovery.topology("session.updated", { id: "child-0", parentID: "foreign" }), true)
  assert.equal(recovery.scan, undefined)
  assert.equal(recovery.family, undefined)
  assert.equal(await recovery.collect(subject.api), undefined)
  recovery.reset() // Reporter/server epoch replacement must not reuse ancestry.
  assert.equal(recovery.scan, undefined)
  assert.equal(recovery.messages, undefined)
})

test("missed topology notification recovers on fallback without promoting child Done", async () => {
  const clock = new Clock(), recovery = new ObservationRecovery(clock), subject = fixture()
  subject.finish()
  assert.ok(normalize(version, await recovery.collect(subject.api)).completed_at_ms)
  subject.api.client.session.children = async ({ sessionID }) => {
    subject.calls["session.children"]++
    return { data: sessionID === "root" ? [{ id: "new-child", parentID: "root" }] : [] }
  }
  subject.api.client.session.status = async () => {
    subject.calls["session.status"]++
    return { data: { "new-child": { type: "busy" } } }
  }
  await clock.advance(10000)
  const snapshot = normalize(version, await recovery.collect(subject.api))
  assert.equal(snapshot.activity, "working")
  assert.equal(snapshot.background_task_count, 1)
  assert.equal(snapshot.completed_at_ms, undefined)
})

test("cancelled topology read retains completed cursor work and later verifies full family", async () => {
  const recovery = new ObservationRecovery(new Clock()), subject = fixture(64), controller = new AbortController()
  const children = subject.api.client.session.children
  let calls = 0
  subject.api.client.session.children = async (...args) => {
    if (++calls === 5) { controller.abort(); throw Error("Read cancelled") }
    return children(...args)
  }
  await assert.rejects(recovery.collect(subject.api, controller.signal), /cancelled/)
  assert.equal(recovery.scan.cursor, 4)
  let result
  for (let n = 0; n < 9 && !result; n++) result = await recovery.collect(subject.api)
  assert.equal(result.members.length, 64)
  assert.equal(subject.calls["session.children"], 128)
})

test("inconsistent second pass, oversized family and wrong get identity refuse terminal authority", async () => {
  const recovery = new ObservationRecovery(new Clock()), subject = fixture()
  let queries = 0
  subject.api.client.session.children = async () => ({ data: ++queries === 1 ? [] : [{ id: "late", parentID: "root" }] })
  await assert.rejects(recovery.collect(subject.api), /Family changed/)
  assert.equal(recovery.family, undefined)
  assert.equal(recovery.scan, undefined)
  const oversized = fixture(65)
  await assert.rejects(recovery.collect(oversized.api), /exceeds bound/)
  const wrong = fixture()
  wrong.api.client.session.get = async () => ({ data: { id: "foreign" } })
  await assert.rejects(recovery.collect(wrong.api), /ancestry/)
})

test("selected child ancestry must belong to independently verified root family", async () => {
  const recovery = new ObservationRecovery(new Clock()), subject = fixture(16)
  subject.api.route.current.params.sessionID = "child-0"
  subject.api.client.session.get = async ({ sessionID }) => ({ data: sessionID === "root"
    ? { id: "root" } : { id: "child-0", parentID: "root" } })
  let result
  for (let n = 0; n < 3; n++) result = await recovery.collect(subject.api)
  assert.equal(result.root.id, "root")
  subject.api.client.session.children = async () => ({ data: [] })
  recovery.reset()
  await assert.rejects(recovery.collect(subject.api), /outside family/)
})

test("route and producer replacement retire previously verified completion metadata", async () => {
  const recovery = new ObservationRecovery(new Clock()), subject = fixture()
  subject.finish()
  assert.ok(normalize(version, await recovery.collect(subject.api)).completed_at_ms)
  subject.api.route.current = { name: "home" }
  assert.equal(await recovery.collect(subject.api), null)
  assert.equal(recovery.family, undefined)
  assert.equal(recovery.messages, undefined)
  subject.api.route.current = { name: "session", params: { sessionID: "root" } }
  subject.api.client.session.messages = async () => ({ error: "offline" })
  await assert.rejects(recovery.collect(subject.api), /Incomplete observation/)
  assert.equal(recovery.messages, undefined)
})

test("recovery retains only bounded structured outcome metadata, never tool bodies or private errors", async () => {
  const recovery = new ObservationRecovery(new Clock()), subject = fixture()
  const privateValue = "private-output".repeat(100000)
  const messages = [
    { info: { id: "u", sessionID: "root", role: "user", time: { created: 1 }, system: privateValue, parentID: privateValue }, parts: [{ text: privateValue }] },
    { info: { id: "a", sessionID: "root", role: "assistant", parentID: "u", finish: "stop",
      time: { created: 2, completed: 42 }, error: { name: "MessageAbortedError", data: { message: privateValue } } },
      parts: [{ state: { output: privateValue } }] },
  ]
  subject.api.client.session.messages = async ({ before }) => ({ data: [messages[before ? 0 : 1]],
    response: { headers: new Headers(before ? {} : { "x-next-cursor": "cursor" }) } })
  const result = await recovery.collect(subject.api)
  assert.equal(normalize(version, result).error_name, "MessageAbortedError")
  assert.ok(JSON.stringify(recovery.messages).length < 512)
  assert.ok(!JSON.stringify(result).includes("private-output"))
  subject.api.client.session.messages = async () => ({ data: [{ info: { id: "u", sessionID: "foreign", role: "user" } }] })
  recovery.invalidateMessages()
  await assert.rejects(recovery.collect(subject.api), /Foreign or invalid/)
})

test("explicit descendant input uses validated ancestry; creation alone cannot claim ownership", async () => {
  const probe = bridgeFixture()
  const children = probe.subject.api.client.session.children
  probe.subject.api.client.session.children = async ({ sessionID }) => sessionID === "root"
    ? { data: [{ id: "child", parentID: "root" }] } : children({ sessionID })
  await probe.start(); await probe.clock.advance(1500)
  const child = probe.children[0]
  probe.subject.listeners.get("question.asked")({ properties: { id: "q", sessionID: "child" } })
  await probe.clock.advance(0)
  assert.deepEqual(child.frames.at(-1).pending_requests, [{ id: "q", kind: "question" }])
  probe.subject.listeners.get("permission.asked")({ properties: { id: "q", sessionID: "root" } })
  await probe.clock.advance(0)
  assert.equal(child.frames.at(-1).pending_requests.length, 2)
  probe.subject.listeners.get("question.rejected")({ properties: { requestID: "q", sessionID: "root" } })
  await probe.clock.advance(0)
  assert.equal(child.frames.at(-1).pending_requests.length, 2) // Wrong owner cannot resolve child's request.
  probe.subject.listeners.get("question.rejected")({ properties: { requestID: "q", sessionID: "child" } })
  await probe.clock.advance(0)
  assert.deepEqual(child.frames.at(-1).pending_requests, [{ id: "q", kind: "permission" }])
  probe.subject.listeners.get("session.created")({ properties: { info: { id: "new", parentID: "root" } } })
  await probe.clock.advance(0)
  assert.equal(child.frames.at(-1).activity, "unknown")
  assert.deepEqual(child.frames.at(-1).pending_requests, [{ id: "q", kind: "permission" }])
  const frames = child.frames.length
  probe.subject.listeners.get("permission.asked")({ properties: { id: "unverified", sessionID: "new" } })
  await probe.clock.advance(0)
  assert.equal(child.frames.length, frames)
  probe.subject.listeners.get("session.created")({ properties: { info: { id: "second-new", parentID: "root" } } })
  probe.subject.listeners.get("session.updated")({ properties: { info: { id: "root", title: "Updated title" } } })
  await probe.clock.advance(0)
  assert.deepEqual(child.frames.at(-1).pending_requests, [{ id: "q", kind: "permission" }])
  probe.subject.listeners.get("permission.replied")({ properties: { requestID: "q", sessionID: "root" } })
  await probe.clock.advance(0)
  assert.deepEqual(child.frames.at(-1).pending_requests, [])
  await probe.dispose()
})

test("reparent and server reconnect immediately revoke cached family input/completion authority", async () => {
  const probe = bridgeFixture()
  probe.subject.finish()
  await probe.start(); await probe.clock.advance(1500)
  const child = probe.children[0]
  assert.ok(child.frames.at(-1).completed_at_ms)
  probe.subject.listeners.get("session.updated")({ properties: { info: { id: "root", parentID: "foreign" } } })
  await probe.clock.advance(0)
  assert.equal(child.frames.at(-1).activity, "unknown")
  assert.equal(child.frames.at(-1).completed_at_ms, undefined)
  probe.subject.listeners.get("server.connected")({ properties: {} })
  await probe.clock.advance(0)
  assert.equal(child.frames.at(-1).activity, "unknown")
  await probe.dispose()
})

test("child retry projects Working rather than root Retrying and malformed request IDs never reach native", async () => {
  const probe = bridgeFixture()
  probe.subject.api.client.session.children = async ({ sessionID }) => ({ data: sessionID === "root"
    ? [{ id: "child", parentID: "root" }] : [] })
  await probe.start(); await probe.clock.advance(1500)
  const child = probe.children[0]
  probe.subject.listeners.get("session.status")({ properties: { sessionID: "child", status: { type: "retry" } } })
  await probe.clock.advance(0)
  assert.equal(child.frames.at(-1).activity, "working")
  assert.equal(child.frames.at(-1).background_task_count, 1)
  const frames = child.frames.length
  for (const id of [undefined, 42, {}, { toString: 42 }, "", "x".repeat(257), "p\n", "p\x1b", "😀".repeat(100)]) {
    probe.subject.listeners.get("permission.asked")({ properties: { id, sessionID: "root" } })
  }
  await probe.clock.advance(0)
  assert.equal(child.frames.length, frames)
  await probe.dispose()
})

test("continuous streaming cannot starve verified-family input projection at startup", async () => {
  const probe = bridgeFixture()
  probe.subject.activity("busy")
  const status = probe.subject.api.client.session.status
  probe.subject.api.client.session.status = async (...args) => {
    probe.subject.listeners.get("message.part.updated")?.({ properties: { part: { sessionID: "root" } } })
    return status(...args)
  }
  await probe.start(); await probe.clock.advance(1500)
  probe.subject.listeners.get("question.asked")({ properties: { id: "q", sessionID: "root" } })
  await probe.clock.advance(0)
  assert.equal(probe.children[0].frames.at(-1).conversation_id, "root")
  assert.deepEqual(probe.children[0].frames.at(-1).pending_requests, [{ id: "q", kind: "question" }])
  await probe.dispose()
})

test("inconsistent status and duplicate input responses cannot establish Done", async () => {
  const recovery = new ObservationRecovery(new Clock()), subject = fixture()
  subject.finish()
  subject.api.client.session.status = async () => ({ data: { root: { type: "unrecognized" } } })
  await assert.rejects(recovery.collect(subject.api), /Invalid activity/)
  subject.api.client.session.status = async () => ({ data: {} })
  subject.api.client.permission.list = async () => ({ data: [{ id: "p", sessionID: "root" }, { id: "p", sessionID: "root" }] })
  await assert.rejects(recovery.collect(subject.api), /Input exceeds bound/)
})

test("stale local Working metadata cannot overwrite an explicitly observed newer root turn", async () => {
  const probe = bridgeFixture()
  probe.subject.activity("busy")
  probe.subject.api.state.session = { messages: () => [{ id: "old-user", sessionID: "root", role: "user" }] }
  await probe.start(); await probe.clock.advance(1500)
  probe.subject.listeners.get("message.updated")({ properties: {
    info: { id: "new-user", sessionID: "root", role: "user" },
  } })
  await probe.clock.advance(1500)
  assert.equal(probe.children[0].frames.at(-1).turn_id, "root/new-user")
  assert.equal(probe.children[0].frames.at(-1).completed_at_ms, undefined)
  await probe.dispose()
})
