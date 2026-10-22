import test from "node:test"
import assert from "node:assert/strict"
import { EVENTS, observe, terminalMetadata } from "./cli-observer-core.mjs"

function fixture({ emitFailure = false, subscribeFailure = false, terminal = false } = {}) {
  const frames = [], typed = new Map(), listeners = new Set(), timers = new Set()
  let current = { type: "home" }, unsubscribed = 0
  const context = {
    options: { fixture: "verij-v2-cli", generation: 1 }, app: { version: "2.0.32" },
    get client() { throw new Error("SDK reads forbidden") },
    ui: { router: { current: () => current, navigate() { throw new Error("Navigation forbidden") } } },
    data: {
      get session() { throw new Error("Cache reads forbidden") },
      on(type, handler) {
        if (subscribeFailure && typed.size === 2) throw new Error("PRIVATE_SUBSCRIPTION_FAILURE")
        typed.set(type, handler)
        return () => { typed.delete(type); unsubscribed++ }
      },
      listen(handler) { listeners.add(handler); return () => { listeners.delete(handler); unsubscribed++ } },
    },
  }
  const clock = { setInterval(fn) { timers.add(fn); return fn }, clearInterval(fn) { timers.delete(fn) } }
  const dispose = observe(context, frame => { if (emitFailure) throw new Error("PRIVATE_DISK_ERROR"); frames.push(frame) }, { clock, ...(terminal ? { projectEvent: terminalMetadata } : {}) })
  const event = (type = EVENTS[0], changes = {}) => ({ type, id: "evt_fixture", data: { sessionID: "ses_private", text: "PRIVATE_PROMPT" },
    durable: { aggregateID: "ses_private", seq: 3, version: 1 }, ...changes })
  return { frames, typed, listeners, timers, dispose, event, context,
    tick() { for (const fn of timers) fn() }, select(value) { current = value },
    send(value) { typed.get(value.type)?.(value); for (const handler of listeners) handler({ details: value }) },
    get unsubscribed() { return unsubscribed },
  }
}

test("Home and plugin routes never carry session authority; session IDs are hashed", () => {
  const f = fixture()
  f.select({ type: "session", sessionID: "ses_private" }); f.tick()
  f.select({ type: "plugin", data: { sessionID: "ses_private", body: "PRIVATE_BODY" } }); f.tick()
  f.select({ type: "home", sessionID: "ses_private" }); f.tick()
  const routes = f.frames.filter(x => x.kind === "route")
  assert.deepEqual(routes.map(x => x.route), ["home", "session", "plugin", "home"])
  assert.match(routes[1].selected, /^[0-9a-f]{20}$/)
  assert.deepEqual(routes.map(x => x.selected === null), [true, false, true, true])
  assert.doesNotMatch(JSON.stringify(f.frames), /ses_private|PRIVATE_BODY/)
  f.dispose()
})

test("typed event.data and envelope.details project identical durable facts without private payloads", () => {
  const f = fixture()
  f.select({ type: "session", sessionID: "ses_private" })
  f.send(f.event(EVENTS[2], { data: { sessionID: "ses_private", error: { type: "api", message: "PRIVATE_ERROR", response: { body: "PRIVATE_PROVIDER" } } } }))
  const events = f.frames.filter(x => x.kind === "event")
  assert.equal(events.length, 2)
  const normalize = ({ channel, index, ...rest }) => rest
  assert.deepEqual(normalize(events[0]), normalize(events[1]))
  assert.doesNotMatch(JSON.stringify(f.frames), /PRIVATE|ses_private|evt_fixture/)
  assert.equal(events[0].seq, 3)
  f.dispose()
})

test("foreign events cannot replace selected route or claim family membership", () => {
  const f = fixture()
  f.select({ type: "session", sessionID: "selected" }); f.tick()
  f.send(f.event())
  const event = f.frames.find(x => x.kind === "event")
  assert.notEqual(event.session, event.selected)
  assert.equal(event.selected, f.frames.filter(x => x.kind === "route").at(-1).selected)
  f.dispose()
})

test("aggregate sequences are not interpreted as a global ordering clock", () => {
  const f = fixture()
  f.send(f.event(EVENTS[0], { durable: { aggregateID: "ses_private", seq: 100, version: 1 } }))
  f.send(f.event(EVENTS[0], { id: "evt_foreign", data: { sessionID: "ses_foreign" }, durable: { aggregateID: "ses_foreign", seq: 1, version: 1 } }))
  assert.deepEqual(f.frames.filter(x => x.kind === "event" && x.channel === "typed").map(x => x.seq), [100, 1])
  f.dispose()
})

test("unknown event types are ignored and unchanged routes coalesce", () => {
  const f = fixture(), before = f.frames.length
  f.send(f.event("session.text.delta")); f.tick(); f.tick()
  assert.equal(f.frames.length, before)
  f.dispose()
})

test("malformed known-event metadata fails closed without throwing into normal execution", () => {
  for (const changes of [{ data: { sessionID: "x".repeat(257) } }, { id: null }, { durable: null },
    { durable: { seq: -1, version: 1, aggregateID: "ses_private" } },
    { durable: { seq: 2**53, version: 1, aggregateID: "ses_private" } },
    { durable: { seq: 1, version: 2, aggregateID: "ses_private" } }]) {
    const f = fixture()
    assert.doesNotThrow(() => f.send(f.event(EVENTS[0], changes)))
    assert.equal(f.frames.filter(x => x.kind === "fault").length, 1)
    assert.equal(f.frames.filter(x => x.kind === "event").length, 0)
    f.send(f.event()); f.dispose()
    assert.equal(f.frames.at(-1).kind, "cleanup")
  }
})

test("invalid session route records only fixed fault reason", () => {
  const f = fixture()
  f.select({ type: "session", sessionID: "" })
  assert.doesNotThrow(f.tick)
  assert.deepEqual(f.frames.at(-1), { kind: "fault", generation: 1, index: 2, reason: "schema" })
  f.dispose()
})

test("event storms reserve bounded fault and cleanup slots without breaking callbacks", () => {
  const f = fixture()
  for (let i = 0; i < 10000; i++) assert.doesNotThrow(() => f.send(f.event()))
  f.dispose()
  assert.equal(f.frames.length, 96)
  assert.deepEqual(f.frames.slice(-2).map(x => x.kind), ["fault", "cleanup"])
  assert.equal(f.frames.at(-2).reason, "frame_bound")
})

test("cleanup unsubscribes every callback, stops timer, and ignores stale callback invocations", () => {
  const f = fixture(), stale = [...f.typed.values(), ...f.listeners]
  f.dispose(); f.dispose()
  const before = f.frames.length
  for (const callback of stale) callback({ ...f.event(), details: f.event() })
  f.tick()
  assert.equal(f.frames.length, before)
  assert.equal(f.unsubscribed, 5)
  assert.equal(f.timers.size, 0)
  assert.equal(f.typed.size + f.listeners.size, 0)
})

test("partial subscription failure rolls back attached handlers", () => {
  const f = fixture({ subscribeFailure: true })
  assert.equal(f.typed.size, 0)
  assert.equal(f.unsubscribed, 2)
  assert.equal(f.frames.at(-1).reason, "subscription")
  f.dispose()
  assert.doesNotMatch(JSON.stringify(f.frames), /PRIVATE_SUBSCRIPTION/)
})

test("evidence write failure cannot throw into the CLI or prevent cleanup", () => {
  const f = fixture({ emitFailure: true })
  assert.doesNotThrow(() => { f.send(f.event()); f.tick(); f.dispose() })
  assert.equal(f.unsubscribed, 5)
  assert.equal(f.timers.size, 0)
})

test("terminal projection reads only error type, never error text, status, response, or other payloads", () => {
  const error = { type: "provider.invalid-request" }
  for (const key of ["message", "status", "response"]) Object.defineProperty(error, key, {
    get() { throw new Error("PRIVATE_ERROR_BODY") },
  })
  const f = fixture({ terminal: true })
  f.send(f.event(EVENTS[2], { data: { sessionID: "ses_private", error } }))
  const events = f.frames.filter(x => x.kind === "event")
  assert.equal(events.length, 2)
  for (const event of events) {
    assert.equal(event.error_type, "provider.invalid-request")
    assert.equal(event.interruption_reason, null)
  }
  assert.doesNotMatch(JSON.stringify(f.frames), /PRIVATE_ERROR|response|status|message/)
  f.dispose()
})

test("arbitrary bounded upstream error types collapse to fixed other rather than retaining private text", () => {
  const f = fixture({ terminal: true })
  f.send(f.event(EVENTS[2], { data: { sessionID: "ses_private", error: { type: "PRIVATE_TYPE", message: "PRIVATE_BODY" } } }))
  assert.equal(f.frames.find(x => x.kind === "event").error_type, "other")
  assert.doesNotMatch(JSON.stringify(f.frames), /PRIVATE/)
  f.dispose()
})

test("every declared interruption reason stays distinct; started/succeeded carry no terminal metadata", () => {
  for (const reason of ["user", "shutdown", "superseded", "inactivity"]) {
    const f = fixture({ terminal: true })
    f.send(f.event(EVENTS[3], { data: { sessionID: "ses_private", reason } }))
    const events = f.frames.filter(x => x.kind === "event")
    assert.equal(events.length, 2)
    assert.ok(events.every(x => x.interruption_reason === reason && x.error_type === null))
    f.dispose()
  }
  for (const type of EVENTS.slice(0, 2)) assert.deepEqual(terminalMetadata({ type, data: {
    get error() { throw new Error("PRIVATE") }, get reason() { throw new Error("PRIVATE") },
  } }), { error_type: null, interruption_reason: null })
})

test("malformed terminal payloads fault once without throwing or retaining secret values", () => {
  for (const [type, data] of [
    [EVENTS[2], {}], [EVENTS[2], { error: null }], [EVENTS[2], { error: { type: "" } }],
    [EVENTS[2], { error: { type: "x".repeat(129) } }], [EVENTS[2], { error: { type: 1 } }],
    [EVENTS[3], {}], [EVENTS[3], { reason: "PRIVATE_REASON" }], [EVENTS[3], { reason: {} }],
  ]) {
    const f = fixture({ terminal: true })
    assert.doesNotThrow(() => f.send(f.event(type, { data: { sessionID: "ses_private", ...data } })))
    assert.equal(f.frames.filter(x => x.kind === "event").length, 0)
    assert.equal(f.frames.filter(x => x.kind === "fault").length, 1)
    f.dispose()
    assert.equal(f.frames.at(-1).kind, "cleanup")
    assert.doesNotMatch(JSON.stringify(f.frames), /PRIVATE/)
  }
})

test("terminal event floods retain frame bound and cleanup; stale terminal callbacks stay fenced", () => {
  const f = fixture({ terminal: true }), stale = f.typed.get(EVENTS[3])
  const value = f.event(EVENTS[3], { data: { sessionID: "ses_private", reason: "user" } })
  for (let i = 0; i < 10000; i++) f.send(value)
  f.dispose()
  assert.equal(f.frames.length, 96)
  assert.deepEqual(f.frames.slice(-2).map(x => x.kind), ["fault", "cleanup"])
  stale(value)
  assert.equal(f.frames.length, 96)
})
