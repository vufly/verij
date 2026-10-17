import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { resolve } from "node:path"
import { EventEmitter } from "node:events"
import test from "node:test"

import {
  supportsVersion,
  isControlChar,
  hasLoneSurrogate,
  isValidU32,
  sanitizeId,
  sanitizeTitle,
  classifyError,
  mapPromptKind,
  unknown,
  validateStrictSnapshot,
  snapshotsSemanticallyEqual,
  SnapshotQueue,
  StateProjector,
  createBridge,
  observationBudget,
  PROTOCOL_VERSION,
  MINIMUM_VERSION,
  SUPPORTED_VERSIONS,
  ERROR_KINDS,
  REQUEST_KINDS,
  PI_ACTIVITIES,
  ALLOWED_STATUSES,
  ALLOWED_READY_KEYS,
  ALLOWED_RECEIPT_KEYS,
  ALLOWED_SNAPSHOT_KEYS,
} from "./core.mjs"

test("Pi minor/patch upgrades retain strict snapshot validation", () => {
  assert.equal(MINIMUM_VERSION, "1.2.0")
  assert.equal(SUPPORTED_VERSIONS, ">=1.2.0, <2.0.0")
  for (const version of ["1.2.0", "1.2.1", "1.3.0", "1.99.42", "1.3.0-beta.1", "1.3.0+build.7"]) {
    assert.equal(supportsVersion(version), true, version)
    const snapshot = unknown(version)
    assert.equal(validateStrictSnapshot(snapshot), true, version)
    assert.equal(validateStrictSnapshot({ ...snapshot, schema_version: 2 }), false, version)
    assert.equal(validateStrictSnapshot({ ...snapshot, prompt: "private" }), false, version)
    assert.equal(validateStrictSnapshot({ ...snapshot, activity: "new-unknown-state" }), false, version)
  }
  for (const version of ["1.1.999", "2.0.0", "0.9.0", "1.2", "1.x.0", "1.3.0-", "1.3.0\n", "1.3.0+bad suffix", "1.18446744073709551616.0", undefined, null, ""]) {
    assert.equal(supportsVersion(version), false, String(version))
    assert.equal(validateStrictSnapshot({ ...unknown(), version }), false, String(version))
  }
})

test("isValidU32 validates bounded 32-bit unsigned integers", () => {
  assert.equal(isValidU32(0), true)
  assert.equal(isValidU32(42), true)
  assert.equal(isValidU32("42"), true)
  assert.equal(isValidU32(0xffffffff), true)
  assert.equal(isValidU32("4294967295"), true)
  assert.equal(isValidU32(-1), false)
  assert.equal(isValidU32("-1"), false)
  assert.equal(isValidU32(0x100000000), false)
  assert.equal(isValidU32("4294967296"), false)
  assert.equal(isValidU32(1.5), false)
  assert.equal(isValidU32("1.5"), false)
  assert.equal(isValidU32("abc"), false)
  assert.equal(isValidU32(""), false)
  assert.equal(isValidU32(null), false)
})

test("ALLOWED_STATUSES matches exact native AgentStatus serialized values", () => {
  assert.deepEqual(ALLOWED_STATUSES, [
    "idle",
    "needs_input",
    "working",
    "done",
    "error",
    "unknown",
  ])
})

test("sanitizeId enforces byte limits, rejects collisions, controls and lone surrogates", () => {
  assert.equal(sanitizeId("clean-id-123"), "clean-id-123")
  assert.equal(sanitizeId("bad\x00id"), undefined)
  assert.equal(sanitizeId("bad\nid"), undefined)
  assert.equal(sanitizeId("bad\rid"), undefined)
  assert.equal(sanitizeId("bad\x1fid"), undefined)
  assert.equal(sanitizeId("bad\x7fid"), undefined)
  assert.equal(sanitizeId("bad\u0080id"), undefined)
  assert.equal(sanitizeId("bad\u009fid"), undefined)
  assert.equal(sanitizeId(""), undefined)
  assert.equal(sanitizeId(null), undefined)
  assert.equal(sanitizeId(123), undefined)

  assert.equal(sanitizeId("lone-\uD800-surrogate"), undefined)
  assert.equal(sanitizeId("lone-\uDC00-surrogate"), undefined)

  const valid256 = "a".repeat(256)
  assert.equal(sanitizeId(valid256), valid256)
  const invalid257 = "a".repeat(257)
  assert.equal(sanitizeId(invalid257), undefined)

  const validEuro = "€".repeat(85)
  assert.equal(Buffer.byteLength(validEuro, "utf8"), 255)
  assert.equal(sanitizeId(validEuro), validEuro)

  const invalidEuro = "€".repeat(86)
  assert.equal(sanitizeId(invalidEuro), undefined)
})

test("sanitizeTitle removes controls, strips lone surrogates, and bounds UTF-8 bytes", () => {
  assert.equal(sanitizeTitle("Normal Title"), "Normal Title")
  assert.equal(sanitizeTitle("A\x00B\x1fC\x7fD\u0080E\u009fF"), "ABCDEF")
  assert.equal(sanitizeTitle("\x00\x1f\x7f\u0085"), undefined)
  assert.equal(sanitizeTitle(""), undefined)

  assert.equal(sanitizeTitle("Title\uD800WithSurrogate"), "TitleWithSurrogate")

  const rockets65 = "🚀".repeat(65)
  const sanitized = sanitizeTitle(rockets65)
  assert(sanitized !== undefined)
  assert.equal(Buffer.byteLength(sanitized, "utf8"), 256)
  assert.equal(sanitized, "🚀".repeat(64))
})

test("classifyError maps failure types to closed enum without retaining raw error bodies", () => {
  assert.equal(classifyError("401 Unauthorized apiKey missing"), "provider_auth")
  assert.equal(classifyError("Insufficient quota billing error"), "provider_quota")
  assert.equal(classifyError("Rate limit exceeded 429"), "provider_rate_limit")
  assert.equal(classifyError("503 Service Unavailable overloaded"), "provider_unavailable")
  assert.equal(classifyError("Corrupt JSON parse error"), "session_corrupt")
  assert.equal(classifyError("Extension failed to load"), "extension_error")
  assert.equal(classifyError("Uncaught runtime exception"), "terminal_failure")
  assert.equal(classifyError({ message: "401 token invalid" }), "provider_auth")
  assert.equal(classifyError(undefined), "terminal_failure")
})

test("mapPromptKind maps Pi kinds to generic requests", () => {
  assert.equal(mapPromptKind("confirm"), "confirmation")
  assert.equal(mapPromptKind("select"), "question")
  assert.equal(mapPromptKind("input"), "question")
  assert.equal(mapPromptKind("editor"), "ui")
  assert.equal(mapPromptKind("custom"), "ui")
  assert.equal(mapPromptKind("unknown"), "ui")
})

test("unknown snapshot conforms to protocol schema", () => {
  const u = unknown()
  assert.equal(validateStrictSnapshot(u), true)
  assert.equal(u.schema_version, 1)
  assert.equal(u.version, "1.2.0")
  assert.deepEqual(u.pending_requests, [])
  assert.equal(u.background_task_count, 0)
  assert.equal("activity" in u, false)
  assert.equal("outcome" in u, false)
})

test("validateStrictSnapshot enforces native Rust constraints and unmodified title rules", () => {
  const validIdle = {
    schema_version: 1,
    version: "1.2.0",
    conversation_id: "conv-1",
    conversation_title: "Valid Title",
    activity: "idle",
    pending_requests: [],
    background_task_count: 0,
  }
  assert.equal(validateStrictSnapshot(validIdle), true)

  // Empty string title is permitted by native
  assert.equal(validateStrictSnapshot({ ...validIdle, conversation_title: "" }), true)
  // Null title is permitted (Option::None)
  assert.equal(validateStrictSnapshot({ ...validIdle, conversation_title: null }), true)

  // Rejects raw unmodified title with controls
  assert.equal(validateStrictSnapshot({ ...validIdle, conversation_title: "Bad\x00Title" }), false)
  assert.equal(validateStrictSnapshot({ ...validIdle, conversation_title: "Bad\nTitle" }), false)
  assert.equal(validateStrictSnapshot({ ...validIdle, conversation_title: "Bad\x7fTitle" }), false)
  assert.equal(validateStrictSnapshot({ ...validIdle, conversation_title: "Bad\u0080Title" }), false)
  // Rejects raw unmodified title > 256 bytes
  assert.equal(validateStrictSnapshot({ ...validIdle, conversation_title: "a".repeat(257) }), false)

  // Valid completed snapshot
  const validDone = {
    schema_version: 1,
    version: "1.2.0",
    conversation_id: "conv-1",
    run_id: "run-1",
    activity: "idle",
    pending_requests: [],
    background_task_count: 0,
    outcome: { kind: "completed", completed_at_ms: 1700000000000 },
  }
  assert.equal(validateStrictSnapshot(validDone), true)

  assert.equal(validateStrictSnapshot({ ...validDone, activity: "working" }), false)
  assert.equal(validateStrictSnapshot({ ...validDone, pending_requests: [{ id: "req-1", kind: "confirmation" }] }), false)
  assert.equal(validateStrictSnapshot({ ...validDone, background_task_count: 1 }), false)
  assert.equal(validateStrictSnapshot({ ...validIdle, extra_field: "prohibited" }), false)
})

test("SnapshotQueue enforces 128-item total capacity, coalescing, and overflow degradation", () => {
  let overflowCalled = false
  const queue = new SnapshotQueue(4, () => {
    overflowCalled = true
  })
  const snap1 = { schema_version: 1, version: "1.2.0", activity: "working" }
  const snap2 = { schema_version: 1, version: "1.2.0", activity: "working" }
  const snap3 = { schema_version: 1, version: "1.2.0", activity: "idle" }

  queue.push(snap1)
  assert.equal(queue.items.length, 1)

  queue.push(snap2)
  assert.equal(queue.items.length, 1)

  queue.push(snap2, true)
  assert.equal(queue.items.length, 2)

  const item = queue.shift()
  assert.equal(item, JSON.stringify(snap1))
  queue.acknowledge()

  queue.push(snap3)
  assert.equal(queue.items.length, 2)

  queue.push({ schema_version: 1, version: "1.2.0", activity: "thinking" })
  queue.push({ schema_version: 1, version: "1.2.0", activity: "retrying" })
  queue.push({ schema_version: 1, version: "1.2.0", activity: "working" })

  assert.equal(overflowCalled, true)
  assert.equal(queue.overflow, true)
  assert.equal(queue.items.length, 1)
  const degraded = JSON.parse(queue.items[0])
  assert.equal(validateStrictSnapshot(degraded), true)
  assert.equal("activity" in degraded, false)

  queue.reset()
  assert.equal(queue.items.length, 0)
  assert.equal(queue.overflow, false)
})

test("SnapshotQueue: requeueInFlight accounts for capacity and never exceeds limit", () => {
  const queue = new SnapshotQueue(2)
  queue.push({ schema_version: 1, version: "1.2.0", activity: "working" })
  queue.push({ schema_version: 1, version: "1.2.0", activity: "idle" })

  const first = queue.shift()
  assert(first !== undefined)
  assert.equal(queue.items.length, 1)

  queue.requeueInFlight()
  assert.equal(queue.items.length, 2)
  assert.equal(queue.items[0], first)

  const queueFull = new SnapshotQueue(2)
  queueFull.push({ schema_version: 1, version: "1.2.0", activity: "working" })
  queueFull.push({ schema_version: 1, version: "1.2.0", activity: "idle" })
  queueFull.inFlight = JSON.stringify({ schema_version: 1, version: "1.2.0", activity: "thinking" })
  queueFull.requeueInFlight()
  assert.equal(queueFull.items.length, 1)
  assert.equal(queueFull.overflow, true)
})

test("StateProjector: Item A regressions - require event.aborted===false, background tasks delay Done", () => {
  const projector = new StateProjector()
  const ctx = {
    sessionManager: { getSessionId: () => "conv-a" },
    hasPendingMessages: () => false,
  }

  projector.handleEvent("session_start", { reason: "startup" }, ctx)
  projector.handleEvent("before_agent_start", {}, ctx)
  projector.handleEvent("agent_before_settle", { outcome: "completed" }, ctx)
  const missingAborted = projector.handleEvent("agent_settled", {}, ctx)
  assert.equal(validateStrictSnapshot(missingAborted), true)
  assert.equal("outcome" in missingAborted, false, "missing aborted:false must degrade to Unknown")
  assert.equal("activity" in missingAborted, false)

  projector.handleEvent("session_start", { reason: "startup" }, ctx)
  projector.handleEvent("before_agent_start", {}, ctx)
  projector.handleEvent("agent_before_settle", { outcome: "completed" }, ctx)
  const conflict1 = projector.handleEvent("agent_settled", { aborted: true }, ctx)
  assert.equal(validateStrictSnapshot(conflict1), true)
  assert.equal("outcome" in conflict1, false)
  assert.equal("activity" in conflict1, false)

  projector.handleEvent("session_start", { reason: "startup" }, ctx)
  projector.handleEvent("before_agent_start", {}, ctx)
  projector.handleEvent("agent_before_settle", { outcome: "aborted" }, ctx)
  const conflict2 = projector.handleEvent("agent_settled", { aborted: false }, ctx)
  assert.equal(validateStrictSnapshot(conflict2), true)
  assert.equal("outcome" in conflict2, false)
  assert.equal("activity" in conflict2, false)

  projector.handleEvent("session_start", { reason: "startup" }, ctx)
  projector.handleEvent("before_agent_start", {}, ctx)
  projector.handleEvent("agent_before_settle", { outcome: "completed" }, ctx)
  const bgSettled = projector.handleEvent("agent_settled", { aborted: false, backgroundTaskCount: 1 }, ctx)
  assert.equal(validateStrictSnapshot(bgSettled), true)
  assert.equal(bgSettled.background_task_count, 1)
  assert.equal("outcome" in bgSettled, false, "background tasks must prevent terminal completion")
})

test("StateProjector: Item B regressions - manual compact suppresses outcome, tools clear old outcome", () => {
  const projector = new StateProjector()
  const ctx = {
    sessionManager: { getSessionId: () => "conv-b" },
    hasPendingMessages: () => false,
  }

  projector.handleEvent("session_start", { reason: "startup" }, ctx)
  projector.handleEvent("before_agent_start", {}, ctx)
  projector.handleEvent("agent_before_settle", { outcome: "completed" }, ctx)
  const done = projector.handleEvent("agent_settled", { aborted: false }, ctx)
  assert.equal(validateStrictSnapshot(done), true)
  assert.equal(done.activity, "idle")
  assert.equal(done.outcome?.kind, "completed")

  const compacting = projector.handleEvent("session_before_compact", { reason: "manual" }, ctx)
  assert.equal(validateStrictSnapshot(compacting), true)
  assert.equal(compacting.activity, "working")
  assert.equal("outcome" in compacting, false, "compaction MUST suppress terminal outcome")

  const compacted = projector.handleEvent("session_compact", {}, ctx)
  assert.equal(validateStrictSnapshot(compacted), true)
  assert.equal(compacted.activity, "idle")
  assert.equal(compacted.outcome?.kind, "completed")

  const toolStart = projector.handleEvent("tool_execution_start", {}, ctx)
  assert.equal(validateStrictSnapshot(toolStart), true)
  assert.equal(toolStart.activity, "working")
  assert.equal("outcome" in toolStart, false, "resumed tool work must clear old outcome")
  assert.equal(projector.outcome, null)
})

test("StateProjector: session_info_changed clearing title when getSessionName undefined", () => {
  const projector = new StateProjector({ conversationTitle: "Initial Title" })
  assert.equal(projector.conversationTitle, "Initial Title")

  projector.handleEvent("session_info_changed", {}, { sessionManager: { getSessionName: () => undefined } })
  assert.equal(projector.conversationTitle, undefined)
  assert.equal("conversation_title" in projector.getSnapshot(), false)
})

test("StateProjector: standalone idle tools restore prior idle activity at end, not working forever", () => {
  const projector = new StateProjector()
  const ctx = { sessionManager: { getSessionId: () => "conv-tool" } }
  projector.handleEvent("session_start", { reason: "startup" }, ctx)
  assert.equal(projector.activity, "idle")

  projector.handleEvent("tool_execution_start", {}, ctx)
  assert.equal(projector.activity, "working")

  projector.handleEvent("tool_execution_end", {}, ctx)
  assert.equal(projector.activity, "idle")
  assert.equal("outcome" in projector.getSnapshot(), false)
})

test("StateProjector: Item C regressions - reload Unknown UI close stays null, no cross-run outcome restore", () => {
  const projector = new StateProjector()
  const ctx = {
    sessionManager: { getSessionId: () => "conv-c" },
    hasPendingMessages: () => false,
  }

  projector.handleEvent("session_start", { reason: "reload" }, ctx)
  const snap1 = projector.getSnapshot()
  assert.equal("activity" in snap1, false)

  projector.handleEvent("ui_prompt_start", { kind: "confirm" }, ctx)
  const closedReloadUi = projector.handleEvent("ui_prompt_end", {}, ctx)
  assert.equal(validateStrictSnapshot(closedReloadUi), true)
  assert.equal("activity" in closedReloadUi, false, "null execution activity must not coalesce to idle")

  const projector2 = new StateProjector()
  projector2.handleEvent("session_start", { reason: "startup" }, ctx)
  projector2.handleEvent("before_agent_start", {}, ctx)
  projector2.handleEvent("agent_before_settle", { outcome: "completed" }, ctx)
  const run1Done = projector2.handleEvent("agent_settled", { aborted: false }, ctx)
  const run1Id = run1Done.run_id
  assert.equal(run1Done.outcome?.kind, "completed")

  projector2.handleEvent("ui_prompt_start", { kind: "confirm" }, ctx)

  projector2.handleEvent("before_agent_start", {}, ctx)
  const run2Id = projector2.aggregateRunId
  assert.notEqual(run1Id, run2Id)

  projector2.handleEvent("agent_before_settle", { outcome: "error", error: "network 503" }, ctx)
  projector2.handleEvent("agent_settled", { aborted: false }, ctx)

  const uiClosedRun2 = projector2.handleEvent("ui_prompt_end", {}, ctx)
  assert.equal(validateStrictSnapshot(uiClosedRun2), true)
  assert.equal("outcome" in uiClosedRun2, false, "old outcome must not be restored across runs")
})

test("StateProjector: Item I regressions - tool tracking >64 degrades to Unknown, session_tree preserves compaction/Unknown", () => {
  const projector = new StateProjector()
  const ctx = {
    sessionManager: { getSessionId: () => "conv-i" },
  }
  projector.handleEvent("session_start", { reason: "startup" }, ctx)
  projector.handleEvent("before_agent_start", {}, ctx)

  for (let i = 0; i < 65; i++) {
    projector.handleEvent("tool_execution_start", {}, ctx)
  }
  const toolOverflow = projector.getSnapshot()
  assert.equal(validateStrictSnapshot(toolOverflow), true)
  assert.equal("activity" in toolOverflow, false, "tools >64 must degrade to Unknown")

  for (let i = 0; i < 65; i++) {
    projector.handleEvent("tool_execution_end", {}, ctx)
  }
  const afterDecrements = projector.getSnapshot()
  assert.equal("activity" in afterDecrements, false, "decrements after overflow must not invent quiescence")

  const projectorTree = new StateProjector()
  projectorTree.handleEvent("session_start", { reason: "startup" }, ctx)
  projectorTree.handleEvent("session_before_compact", {}, ctx)
  assert.equal(projectorTree.activity, "working")
  const treeCompacting = projectorTree.handleEvent("session_tree", {}, ctx)
  assert.equal(validateStrictSnapshot(treeCompacting), true)
  assert.equal(treeCompacting.activity, "working", "session_tree must preserve active compaction")
})

test("P0 Trace Replay: sequential replay across each run and validation of all 112 cases", () => {
  const resultsPath = resolve("prototypes/pi-runtime/results.json")
  const results = JSON.parse(readFileSync(resultsPath, "utf8"))

  assert.equal(results.runs.length, 4)

  for (const run of results.runs) {
    const projector = new StateProjector()

    for (const testCase of run.cases) {
      let lastSnap = null
      let sawBeforeStart = false
      let sawSettled = false

      for (const step of testCase.trace) {
        if (step.source !== "observer") continue

        if (step.event === "before_agent_start") sawBeforeStart = true
        if (step.event === "agent_settled") sawSettled = true

        const ctx = {
          sessionManager: {
            getSessionId: () => step.session ? "conv-p0-" + step.session : "conv-p0",
            getSessionName: () => step.name_present ? "P0 Session Name" : undefined,
          },
          isIdle: () => step.idle ?? false,
          hasPendingMessages: () => step.pending ?? false,
          now: () => step.at_ms ? step.at_ms + 1000 : 1000,
        }

        lastSnap = projector.handleEvent(step.event, step, ctx)

        assert.equal(validateStrictSnapshot(lastSnap), true, `step ${step.event} in ${testCase.case} must be valid native snapshot`)

        if (!sawSettled && sawBeforeStart) {
          assert.equal(lastSnap.outcome?.kind !== "completed", true, `interim premature Done prohibited in ${testCase.case}`)
        }
      }

      switch (testCase.case) {
        case "success":
        case "parallel":
        case "nested":
        case "tool_error":
        case "retry_success":
        case "auto_compact":
        case "queued":
        case "late_queued":
        case "continuation":
          assert.equal(lastSnap.activity, "idle", `case ${testCase.case} must settle idle`)
          assert.equal(lastSnap.outcome?.kind, "completed", `case ${testCase.case} must complete successfully`)
          assert(lastSnap.outcome.completed_at_ms > 0)
          break

        case "cancel":
          assert.equal(lastSnap.activity, "idle")
          assert.equal(lastSnap.outcome?.kind, "aborted")
          break

        case "provider_error":
        case "retry_exhausted":
          assert.equal(lastSnap.activity, "idle")
          assert.equal(lastSnap.outcome?.kind, "error")
          break

        case "postrun_error":
        case "continuation_error":
        case "manual_compact":
          assert.equal("outcome" in lastSnap, false)
          break
      }
    }
  }
})

// Mock Child Process for Bounded Transport Bridge Tests
class MockStdin extends EventEmitter {
  constructor() {
    super()
    this.lines = []
    this.closed = false
  }
  write(data, cb) {
    this.lines.push(data)
    if (cb) process.nextTick(cb)
    return true
  }
  end() {
    this.closed = true
    this.emit("close")
  }
}

class MockProcess extends EventEmitter {
  constructor() {
    super()
    this.pid = 99999
    this.stdin = new MockStdin()
    this.stdout = new EventEmitter()
    this.killed = false
    this.killSignal = null
    this.signals = []
    this.autoCloseOnKill = true
  }

  get written() {
    return this.stdin.lines.join("")
  }

  get stdinClosed() {
    return this.stdin.closed
  }

  kill(signal = "SIGTERM") {
    this.killed = true
    this.killSignal = signal
    this.signals.push(signal)
    if (this.autoCloseOnKill) {
      this.emit("exit", 0)
      this.emit("close", 0)
    }
  }

  pushStdout(line) {
    this.stdout.emit("data", Buffer.from(line + "\n"))
  }
}

function createTestClock() {
  let currentTime = 1000
  let timerId = 0
  const activeTimers = new Map()

  const clock = {
    now: () => currentTime,
    setTimeout: (fn, ms) => {
      const id = ++timerId
      activeTimers.set(id, { fn, triggerAt: currentTime + ms })
      return id
    },
    clearTimeout: id => activeTimers.delete(id),
  }

  const advanceTime = ms => {
    currentTime += ms
    for (const [id, timer] of [...activeTimers.entries()]) {
      if (timer.triggerAt <= currentTime) {
        activeTimers.delete(id)
        timer.fn()
      }
    }
  }

  return { clock, advanceTime, activeTimers }
}

test("createBridge: Pi 1.3.0 connects and accepts native NeedsInput status on ready greeting and receipts", () => {
  const { clock } = createTestClock()
  let proc = null
  const fakePi = new EventEmitter()
  const bridge = createBridge(
    { pi: fakePi, version: "1.3.0", pane: "1", session: "s", conversationId: "conv-needs-input" },
    { clock, spawn: () => { proc = new MockProcess(); return proc } }
  )

  bridge.install()
  fakePi.emit("session_start", { reason: "startup" }, { mode: "tui" })
  assert(proc !== null)

  proc.pushStdout(JSON.stringify({
    ready: true,
    agent_instance_id: "inst-1",
    source_epoch: 1,
    source_revision: 1,
    status: "needs_input",
    restored: false,
  }))

  assert.equal(bridge.isConnected(), true)
  assert.equal(proc.killed, false)

  proc.pushStdout(JSON.stringify({
    source_epoch: 1,
    source_revision: 2,
    record_revision: 1,
    completion_revision: 0,
    status: "needs_input",
  }))

  assert.equal(proc.killed, false)
  bridge.teardown()
})

test("createBridge: Item D rate limiter - 100 alternating prompts respect <=10 writes/s and preserve order", () => {
  const { clock, advanceTime } = createTestClock()
  let proc = null
  const fakePi = new EventEmitter()
  const bridge = createBridge(
    { pi: fakePi, version: "1.2.0", pane: "1", session: "s", conversationId: "conv-rate" },
    { clock, spawn: () => { proc = new MockProcess(); return proc } }
  )

  bridge.install()
  fakePi.emit("session_start", { reason: "startup" }, { mode: "tui" })

  assert(proc !== null)

  proc.pushStdout(JSON.stringify({
    ready: true,
    agent_instance_id: "inst",
    source_epoch: 1,
    source_revision: 1,
    status: "idle",
    restored: false,
  }))

  const writeTimestamps = []
  let rev = 2

  for (let i = 0; i < 50; i++) {
    fakePi.emit("ui_prompt_start", { kind: "confirm" }, { mode: "tui" })
    fakePi.emit("ui_prompt_end", {}, { mode: "tui" })
  }

  while (writeTimestamps.length < 50) {
    if (proc.stdin.lines.length > writeTimestamps.length + 1) {
      writeTimestamps.push(clock.now())
      proc.pushStdout(JSON.stringify({
        source_epoch: 1,
        source_revision: rev++,
        record_revision: 1,
        completion_revision: 0,
        status: "idle",
      }))
    }
    advanceTime(100)
  }

  for (let i = 1; i < writeTimestamps.length; i++) {
    const delta = writeTimestamps[i] - writeTimestamps[i - 1]
    assert(delta >= 100, `write interval ${delta}ms must be >= 100ms (<= 10 writes/s)`)
  }

  bridge.teardown()
})

test("createBridge: Item E - queue overflow replaces lastSnapshot with Unknown/NeedsInput, heartbeat preserves Unknown", () => {
  const { clock, advanceTime } = createTestClock()
  let proc = null
  const fakePi = new EventEmitter()
  const bridge = createBridge(
    { pi: fakePi, version: "1.2.0", pane: "1", session: "s", conversationId: "conv-over" },
    { clock, spawn: () => { proc = new MockProcess(); return proc } }
  )

  bridge.install()
  fakePi.emit("session_start", { reason: "startup" }, { mode: "tui" })
  assert(proc !== null)

  proc.pushStdout(JSON.stringify({
    ready: true,
    agent_instance_id: "inst",
    source_epoch: 1,
    source_revision: 1,
    status: "idle",
    restored: false,
  }))

  fakePi.emit("before_agent_start", {}, { mode: "tui" })
  fakePi.emit("agent_before_settle", { outcome: "completed" }, { mode: "tui" })
  fakePi.emit("agent_settled", { aborted: false }, { mode: "tui" })

  const queue = bridge.getQueue()
  for (let i = 0; i < 150; i++) {
    queue.push({ schema_version: 1, version: "1.2.0", activity: "working", i })
  }

  assert.equal(queue.overflow, true)
  const projector = bridge.getProjector()
  assert.equal("outcome" in projector.getSnapshot(), false)

  advanceTime(5000)
  for (const line of proc.stdin.lines.slice(2)) {
    const snap = JSON.parse(line)
    assert.equal("outcome" in snap, false, "overflowed state must never replay completion on heartbeat")
  }

  bridge.teardown()
})

test("createBridge: Item F - no duplicate child processes, spontaneous crash clears timers and requeues inFlight", () => {
  const { clock, advanceTime } = createTestClock()
  let spawnCount = 0
  let activeProcesses = []

  class TrackedProcess extends MockProcess {
    constructor() {
      super()
      spawnCount++
      activeProcesses.push(this)
    }
    kill(signal) {
      super.kill(signal)
      activeProcesses = activeProcesses.filter(p => p !== this)
    }
  }

  const fakePi = new EventEmitter()
  const bridge = createBridge(
    { pi: fakePi, version: "1.2.0", pane: "1", session: "s", conversationId: "conv-leak" },
    { clock, spawn: () => new TrackedProcess() }
  )

  bridge.install()
  fakePi.emit("session_start", { reason: "startup" }, { mode: "tui" })
  assert.equal(spawnCount, 1)

  const firstChild = activeProcesses[0]
  assert(firstChild !== undefined)
  firstChild.stdin.emit("error", new Error("stdin broken"))

  advanceTime(200)
  assert.equal(spawnCount, 1)

  bridge.teardown()
})

test("createBridge: Item G - rejects unknown keys, invalid status enums, corrupted restored state", () => {
  const { clock } = createTestClock()

  {
    let proc = null
    const bridge = createBridge(
      { pi: new EventEmitter(), version: "1.2.0", pane: "1", session: "s", conversationId: "c1" },
      { clock, spawn: () => { proc = new MockProcess(); return proc } }
    )
    bridge.startReporter()
    proc.pushStdout(JSON.stringify({
      ready: true,
      agent_instance_id: "inst",
      source_epoch: 1,
      source_revision: 1,
      status: "idle",
      restored: false,
      unknown_extra_key: "forbidden",
    }))
    assert.equal(proc.killed, true)
    bridge.teardown()
  }

  {
    let proc = null
    const bridge = createBridge(
      { pi: new EventEmitter(), version: "1.2.0", pane: "1", session: "s", conversationId: "c1" },
      { clock, spawn: () => { proc = new MockProcess(); return proc } }
    )
    bridge.startReporter()
    proc.pushStdout(JSON.stringify({
      ready: true,
      agent_instance_id: "inst",
      source_epoch: 1,
      source_revision: 1,
      status: "INVALID_STATUS",
      restored: false,
    }))
    assert.equal(proc.killed, true)
    bridge.teardown()
  }

  {
    let proc = null
    const bridge = createBridge(
      { pi: new EventEmitter(), version: "1.2.0", pane: "1", session: "s", conversationId: "c1" },
      { clock, spawn: () => { proc = new MockProcess(); return proc } }
    )
    bridge.startReporter()
    proc.pushStdout(JSON.stringify({
      ready: true,
      agent_instance_id: "inst",
      source_epoch: 1,
      source_revision: 1,
      status: "idle",
      restored: true,
      aggregate_run_id: "run-1",
      latest_snapshot: {
        schema_version: 1,
        version: "1.2.0",
        conversation_id: "c1",
        run_id: "run-1",
        activity: "working",
        pending_requests: [],
        background_task_count: 0,
        outcome: { kind: "completed", completed_at_ms: 1000 },
      },
    }))
    assert.equal(proc.killed, true)
    bridge.teardown()
  }
})

test("createBridge: framing handles split UTF-8 chunks across multiple data events", () => {
  const { clock } = createTestClock()
  let proc = null
  const fakePi = new EventEmitter()
  const bridge = createBridge(
    { pi: fakePi, version: "1.2.0", pane: "1", session: "s", conversationId: "conv-split" },
    { clock, spawn: () => { proc = new MockProcess(); return proc } }
  )

  bridge.install()
  fakePi.emit("session_start", { reason: "startup" }, { mode: "tui" })
  assert(proc !== null)

  const jsonLine = JSON.stringify({
    ready: true,
    agent_instance_id: "inst-split-€",
    source_epoch: 1,
    source_revision: 1,
    status: "idle",
    restored: false,
  }) + "\n"

  const buf = Buffer.from(jsonLine, "utf8")
  const part1 = buf.subarray(0, 40)
  const part2 = buf.subarray(40)

  proc.stdout.emit("data", part1)
  assert.equal(bridge.isConnected(), false)

  proc.stdout.emit("data", part2)
  assert.equal(bridge.isConnected(), true)
  bridge.teardown()
})

test("createBridge: oversized frames (> 256 KiB) trigger immediate SIGKILL", () => {
  const { clock } = createTestClock()
  let proc = null
  const fakePi = new EventEmitter()
  const bridge = createBridge(
    { pi: fakePi, version: "1.2.0", pane: "1", session: "s", conversationId: "conv-oversized" },
    { clock, spawn: () => { proc = new MockProcess(); return proc } }
  )

  bridge.install()
  fakePi.emit("session_start", { reason: "startup" }, { mode: "tui" })
  assert(proc !== null)

  const bigChunk = Buffer.alloc(270000, 65)
  proc.stdout.emit("data", bigChunk)

  assert.equal(proc.killed, true)
  assert.equal(proc.killSignal, "SIGKILL")
  bridge.teardown()
})

test("createBridge: synchronous write throw is safely contained", () => {
  const { clock } = createTestClock()
  let proc = null
  class ThrowingWriteProcess extends MockProcess {
    constructor() {
      super()
      this.stdin.write = () => {
        throw new Error("EPIPE synchronous write error")
      }
    }
  }

  const fakePi = new EventEmitter()
  const bridge = createBridge(
    { pi: fakePi, version: "1.2.0", pane: "1", session: "s", conversationId: "conv-throw" },
    { clock, spawn: () => { proc = new ThrowingWriteProcess(); return proc } }
  )

  bridge.install()
  assert.doesNotThrow(() => {
    fakePi.emit("session_start", { reason: "startup" }, { mode: "tui" })
  })

  assert.equal(proc.killed, true)
  bridge.teardown()
})

test("createBridge: synchronous spawn throw retries with exponential backoff", () => {
  const { clock, advanceTime } = createTestClock()
  let attempts = 0
  const mockSpawn = () => {
    attempts++
    if (attempts === 1) {
      throw new Error("ENOENT: verij not found")
    }
    return new MockProcess()
  }

  const fakePi = new EventEmitter()
  const bridge = createBridge(
    { pi: fakePi, version: "1.2.0", pane: "1", session: "s", conversationId: "conv-spawn-retry" },
    { clock, spawn: mockSpawn }
  )

  bridge.install()
  fakePi.emit("session_start", { reason: "startup" }, { mode: "tui" })
  assert.equal(attempts, 1)

  advanceTime(2001)
  assert.equal(attempts, 2)
  bridge.teardown()
})

test("createBridge: Item H - mode guard rejects non-TUI and stops subsequent events from queueing", () => {
  const { clock } = createTestClock()
  let spawned = false
  const fakePi = new EventEmitter()
  const bridge = createBridge(
    { pi: fakePi, version: "1.2.0", pane: "1", session: "s", conversationId: "c1" },
    { clock, spawn: () => { spawned = true; return new MockProcess() } }
  )

  bridge.install()

  fakePi.emit("session_start", { reason: "startup" }, { mode: "print" })
  assert.equal(spawned, false)

  fakePi.emit("before_agent_start", {}, { mode: "print" })
  assert.equal(spawned, false)
  assert.equal(bridge.getQueue().items.length, 0)
  bridge.teardown()
})

test("createBridge: Item H - unsupported version spawns zero processes and sets zero timers", () => {
  let spawned = false
  let timersSet = 0
  const fakePi = new EventEmitter()
  const bridge = createBridge(
    { pi: fakePi, version: "2.0.0", pane: "1", session: "s", conversationId: "c1" },
    {
      spawn: () => { spawned = true; return new MockProcess() },
      clock: { now: () => 1000, setTimeout: () => { timersSet++; return 1 }, clearTimeout: () => {} },
    }
  )

  bridge.install()
  fakePi.emit("session_start", { reason: "startup" }, { mode: "tui" })
  assert.equal(spawned, false)
  assert.equal(timersSet, 0)
})

test("createBridge: shutdown flush delivers queued terminal edge when child responsive, zero leaked timers", () => {
  const { clock, advanceTime, activeTimers } = createTestClock()
  let proc = null
  const fakePi = new EventEmitter()
  const bridge = createBridge(
    { pi: fakePi, version: "1.2.0", pane: "1", session: "s", conversationId: "c1" },
    { clock, spawn: () => { proc = new MockProcess(); return proc } }
  )

  bridge.install()
  fakePi.emit("session_start", { reason: "startup" }, { mode: "tui" })
  proc.pushStdout(JSON.stringify({
    ready: true,
    agent_instance_id: "inst-1",
    source_epoch: 1,
    source_revision: 1,
    status: "idle",
    restored: false,
  }))
  proc.pushStdout(JSON.stringify({
    source_epoch: 1,
    source_revision: 2,
    record_revision: 1,
    completion_revision: 0,
    status: "idle",
  }))

  advanceTime(101)

  fakePi.emit("before_agent_start", {}, { mode: "tui" })
  fakePi.emit("agent_before_settle", { outcome: "completed" }, { mode: "tui" })
  fakePi.emit("agent_settled", { aborted: false }, { mode: "tui", now: () => 12345 })

  // Terminal frame is queued while working frame was in-flight. Immediately teardown
  bridge.teardown()

  // Child stdin is NOT yet closed (flush is active)
  assert.equal(proc.stdinClosed, false)

  // Receipt arrives for in-flight working frame
  proc.pushStdout(JSON.stringify({
    source_epoch: 1,
    source_revision: 3,
    record_revision: 2,
    completion_revision: 0,
    status: "working",
  }))

  // Rate limiter interval (100ms) elapses to flush queued terminal frame
  advanceTime(101)

  const writtenLines = proc.written.trim().split("\n")
  const lastWritten = JSON.parse(writtenLines.at(-1))
  assert.equal(lastWritten.outcome?.kind, "completed")

  // Receipt arrives for terminal frame
  proc.pushStdout(JSON.stringify({
    source_epoch: 1,
    source_revision: 4,
    record_revision: 3,
    completion_revision: 1,
    status: "done",
  }))

  // All frames delivered, stdin closed
  assert.equal(proc.stdinClosed, true)
  proc.emit("close")

  assert.equal(activeTimers.size, 0)
  assert.equal(bridge.isStopped(), true)
})

test("createBridge: shutdown flush deadline escalates and kills unresponsive child, zero leaked timers", () => {
  const { clock, advanceTime, activeTimers } = createTestClock()
  let proc = null
  const fakePi = new EventEmitter()
  const bridge = createBridge(
    { pi: fakePi, version: "1.2.0", pane: "1", session: "s", conversationId: "c1" },
    { clock, spawn: () => { proc = new MockProcess(); proc.autoCloseOnKill = false; return proc } }
  )

  bridge.install()
  fakePi.emit("session_start", { reason: "startup" }, { mode: "tui" })
  proc.pushStdout(JSON.stringify({
    ready: true,
    agent_instance_id: "inst-1",
    source_epoch: 1,
    source_revision: 1,
    status: "idle",
    restored: false,
  }))
  proc.pushStdout(JSON.stringify({
    source_epoch: 1,
    source_revision: 2,
    record_revision: 1,
    completion_revision: 0,
    status: "idle",
  }))

  fakePi.emit("before_agent_start", {}, { mode: "tui" })
  fakePi.emit("agent_before_settle", { outcome: "completed" }, { mode: "tui" })
  fakePi.emit("agent_settled", { aborted: false }, { mode: "tui", now: () => 12345 })

  bridge.teardown()
  assert.equal(proc.stdinClosed, false)

  // 500ms deadline expires -> stdin closed
  advanceTime(501)
  assert.equal(proc.stdinClosed, true)

  // 500ms grace -> SIGTERM
  advanceTime(501)
  assert.equal(proc.signals.includes("SIGTERM"), true)

  // 500ms grace -> SIGKILL
  advanceTime(501)
  assert.equal(proc.signals.includes("SIGKILL"), true)

  proc.emit("close")
  assert.equal(activeTimers.size, 0)
})

test("createBridge: teardown does not start reconnect even if child closes during teardown", () => {
  const { clock, advanceTime } = createTestClock()
  let spawnCount = 0
  let proc = null
  const fakePi = new EventEmitter()
  const bridge = createBridge(
    { pi: fakePi, version: "1.2.0", pane: "1", session: "s", conversationId: "c1" },
    { clock, spawn: () => { spawnCount++; proc = new MockProcess(); return proc } }
  )

  bridge.install()
  fakePi.emit("session_start", { reason: "startup" }, { mode: "tui" })
  assert.equal(spawnCount, 1)

  bridge.teardown()
  proc.emit("close")

  advanceTime(30000)
  assert.equal(spawnCount, 1)
  assert.equal(bridge.isStopped(), true)
})

test("StateProjector: unexpected context conversation change retires state to Unknown and reseeds runtime", () => {
  let uuidCounter = 1
  const randomUUID = () => `uuid-${uuidCounter++}`
  const projector = new StateProjector({ version: "1.2.0", randomUUID, conversationId: "conv-1" })

  projector.handleEvent("before_agent_start", {}, {})
  assert.equal(projector.runActive, true)
  assert.equal(projector.activity, "working")
  const initialRuntime = projector.runtimeId

  const ctxNew = {
    sessionManager: {
      getSessionId: () => "conv-2",
      getSessionName: () => "Conversation Two",
    },
  }

  const snapDuringRun = projector.handleEvent("agent_before_settle", { outcome: "completed" }, ctxNew)
  assert.equal(snapDuringRun.conversation_id, "conv-2")
  assert.equal(snapDuringRun.outcome, undefined)
  assert.equal(snapDuringRun.run_id, undefined)
  assert.equal(projector.runActive, false)
  assert.equal(projector.outcome, null)
  assert.notEqual(projector.runtimeId, initialRuntime)

  const settleSnap = projector.handleEvent("agent_settled", { aborted: false }, ctxNew)
  assert.equal(settleSnap.outcome, undefined)
})

test("StateProjector: conversation change after Done clears outcome and never publishes old completion under new ID", () => {
  let uuidCounter = 1
  const randomUUID = () => `uuid-${uuidCounter++}`
  const projector = new StateProjector({ version: "1.2.0", randomUUID, conversationId: "conv-1" })

  projector.handleEvent("session_start", { reason: "startup", session: "conv-1" }, {})
  projector.handleEvent("before_agent_start", {}, {})
  projector.handleEvent("agent_before_settle", { outcome: "completed" }, {})
  const doneSnap = projector.handleEvent("agent_settled", { aborted: false }, { now: () => 1000 })
  assert.equal(doneSnap.outcome?.kind, "completed")
  assert.equal(doneSnap.conversation_id, "conv-1")

  const ctxChanged = {
    sessionManager: {
      getSessionId: () => "conv-new",
    },
  }

  const snapAfterDone = projector.handleEvent("session_info_changed", {}, ctxChanged)
  assert.equal(snapAfterDone.conversation_id, "conv-new")
  assert.equal(snapAfterDone.outcome, undefined)
  assert.equal(snapAfterDone.run_id, undefined)
  assert.equal(projector.outcome, null)

  const freshSnap = projector.getSnapshot()
  assert.equal(freshSnap.outcome, undefined)
})

test("StateProjector: invalid conversation ID in context retires to Unknown without old ID retention", () => {
  const projector = new StateProjector({ version: "1.2.0", conversationId: "conv-valid" })

  const ctxInvalid = {
    sessionManager: {
      getSessionId: () => "invalid\u0000id",
    },
  }

  const snap = projector.handleEvent("turn_start", {}, ctxInvalid)
  assert.equal(snap.conversation_id, undefined)
  assert.equal(projector.conversationId, undefined)
  assert.equal(projector.degradedToUnknown, true)
})
