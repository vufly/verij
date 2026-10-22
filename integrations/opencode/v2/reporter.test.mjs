import test from "node:test"
import assert from "node:assert/strict"
import { EventEmitter } from "node:events"
import { LiveReporter } from "./reporter.mjs"
import { unknown } from "./core.mjs"
import { createPlugin } from "./tui.js"

function fixture() {
  let now = 0, next = 0, resets = 0
  const timers = new Map(), intervals = new Map(), children = []
  const clock = { now: () => now,
    setTimeout: (fn, ms) => { timers.set(++next, { fn, at: now + ms }); return next },
    clearTimeout: id => timers.delete(id),
    setInterval: fn => { intervals.set(++next, fn); return next }, clearInterval: id => intervals.delete(id) }
  const spawn = (executable, args, options) => {
    const child = new EventEmitter()
    child.stdin = new EventEmitter(); child.stdout = new EventEmitter(); child.frames = []; child.signals = []
    child.stdin.write = (value, cb) => { child.frames.push(JSON.parse(value)); cb?.() }
    child.stdin.end = () => child.emit("exit")
    child.kill = signal => { child.signals.push(signal); child.emit("exit") }
    child.reply = value => child.stdout.emit("data", Buffer.from(JSON.stringify(value) + "\n"))
    child.executable = executable; child.args = args; child.options = options
    children.push(child); return child
  }
  const reporter = new LiveReporter({ executable: "/verij", args: ["agent", "opencode"], env: {},
    unknown: unknown("2.1.2"), reset: () => resets++, spawn, clock })
  return { reporter, children, clock, spawn, timers, intervals, resets: () => resets,
    advance(ms) { now += ms; for (const [id, timer] of [...timers]) if (timer.at <= now && timers.delete(id)) timer.fn() },
    interval() { for (const fn of intervals.values()) fn() } }
}
const working = id => ({ ...unknown("2.1.2"), conversation_id: id, activity: "working" })

test("ready starts with Unknown; native receipts serialize and dedupe snapshots", () => {
  const f = fixture(); f.reporter.tick(0); const child = f.children[0]
  child.reply({ ready: true }); assert.deepEqual(child.frames, [unknown("2.1.2")]); assert.equal(f.resets(), 1)
  f.reporter.publish(working("root")); f.reporter.publish(working("root")); assert.equal(child.frames.length, 1)
  child.reply({ record_revision: 1 }); assert.deepEqual(child.frames[1], working("root"))
  child.reply({ record_revision: 2 }); f.reporter.publish(working("root")); assert.equal(child.frames.length, 2)
})
test("event storms retain one pending full snapshot and preserve latest selection", () => {
  const f = fixture(); f.reporter.tick(0); const child = f.children[0]; child.reply({ ready: true })
  for (let i = 0; i < 10000; i++) f.reporter.publish(working(`root${i}`))
  assert.equal(child.frames.length, 1); assert.equal(typeof f.reporter.pending, "string")
  child.reply({ record_revision: 1 }); assert.equal(child.frames[1].conversation_id, "root9999")
})
test("five-second lease renewal uses no API reads and no duplicate process", () => {
  const f = fixture(); f.reporter.tick(0); const child = f.children[0]; child.reply({ ready: true }); child.reply({ record_revision: 1 })
  f.advance(4999); f.reporter.tick(4999); assert.equal(child.frames.length, 1)
  f.advance(1); f.reporter.tick(5000); assert.equal(child.frames.length, 2); assert.equal(f.children.length, 1)
})
test("reporter exit fences pending hints; replacement begins Unknown after backoff", () => {
  const f = fixture(); f.reporter.tick(0); const first = f.children[0]; first.reply({ ready: true }); f.reporter.publish(working("old"))
  first.emit("exit"); assert.equal(f.reporter.ready, false); assert.equal(f.reporter.pending, undefined)
  f.reporter.tick(1999); assert.equal(f.children.length, 1)
  f.advance(2000); f.reporter.tick(2000); const second = f.children[1]; second.reply({ ready: true })
  assert.deepEqual(second.frames, [unknown("2.1.2")])
  first.reply({ record_revision: 1 }); assert.equal(second.frames.length, 1)
})
test("startup/receipt deadlines and malformed/oversized replies terminate child", () => {
  for (const action of [
    (f, child) => f.advance(4000),
    (f, child) => { child.reply({ ready: true }); f.advance(4000) },
    (f, child) => child.reply({ record_revision: 1 }),
    (f, child) => child.stdout.emit("data", Buffer.alloc(16385)),
    (f, child) => child.stdout.emit("data", Buffer.from("bad\n")),
    (f, child) => { child.reply({ ready: true }); child.reply({ ready: true }) },
  ]) {
    const f = fixture(); f.reporter.tick(0); const child = f.children[0]; action(f, child)
    assert.deepEqual(child.signals, ["SIGKILL"]); assert.equal(f.reporter.ready, false)
  }
})
test("spawn failure backs off and never throws into OpenCode", () => {
  const f = fixture(); f.reporter.spawn = () => { throw new Error("no binary") }
  assert.doesNotThrow(() => f.reporter.tick(0)); assert.equal(f.reporter.retryAt, 2000)
})
test("dispose clears timers, closes owned pipe, and prevents restarts", async () => {
  const f = fixture(); f.reporter.tick(0); f.children[0].reply({ ready: true })
  await f.reporter.dispose(); await f.reporter.dispose()
  assert.equal(f.timers.size, 0); f.reporter.tick(99999); assert.equal(f.children.length, 1)
})
test("plugin requires opt-in, V2 runtime shape, pane locator, and absolute reporter", () => {
  const f = fixture(); const plugin = createPlugin({ clock: f.clock, spawn: f.spawn, env: {} })
  for (const context of [ {}, { app: { version: "1.19.0" }, options: { experimentalLiveStatus: true } },
    { app: { version: "2.1.2" }, options: { reporter: "/verij" } } ]) assert.equal(plugin.setup(context), undefined)
  assert.equal(f.children.length, 0)
})
test("plugin reads cache on one-second ticks only, survives events, and cleans up", async () => {
  const f = fixture(); let listener, unsubscribed = 0, reads = 0
  const forbidden = new Proxy({}, { get() { throw new Error("forbidden") } })
  const context = { app: { version: "2.1.2" }, options: { experimentalLiveStatus: true, reporter: "/verij", runtimeDir: "/installed", statesDir: "/states" },
    client: forbidden, ui: { router: { current: () => ({ type: "session", sessionID: "root" }) } },
    data: { listen: cb => { listener = cb; return () => unsubscribed++ }, session: {
      get: id => { reads++; return { id } }, family: () => ["root"], status: () => "idle",
      permission: { list: () => undefined }, form: { list: () => undefined }, message: forbidden, sync: forbidden } } }
  const plugin = createPlugin({ clock: f.clock, spawn: f.spawn,
    env: { ZELLIJ_PANE_ID: "3", ZELLIJ_SESSION_NAME: "inner", VERIJ_AGENT_STATE_DIR: "/override" } })
  const cleanup = plugin.setup(context), child = f.children[0]
  assert.equal(child.options.env.VERIJ_AGENT_STATE_DIR, "/override"); assert.equal(child.options.env.VERIJ_STATES_DIR, "/states")
  assert.deepEqual(child.args.slice(0, 6), ["agent", "opencode", "--session", "inner", "--pane", "3"])
  child.reply({ ready: true }); child.reply({ record_revision: 1 }); f.interval()
  const before = reads
  for (let i = 0; i < 10000; i++) listener({ details: { type: "message.updated", data: { sessionID: "root", body: "SECRET" } } })
  assert.equal(reads, before)
  listener({ details: { type: "session.execution.started", data: { sessionID: "root" } } }); f.interval()
  child.reply({ record_revision: 2 }); assert.equal(child.frames.at(-1).activity, "working")
  assert.ok(!JSON.stringify(child.frames).includes("SECRET"))
  await cleanup(); assert.equal(unsubscribed, 1); assert.equal(f.intervals.size, 0); assert.equal(f.timers.size, 0)
})
