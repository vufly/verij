import test from "node:test"
import assert from "node:assert/strict"
import { ObservationRecovery, OutcomeRecovery, RequestBudget, budgetedAPI,
  readMessagePrefix, readBoundedJSON, observationBudget, normalize, WorkerMetadata } from "./core.mjs"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import { DatabaseSync } from "node:sqlite"
import { fixture } from "../../prototypes/resource-usage/fixture.mjs"

const user = (id = "u", created = 1) => ({ id, sessionID: "root", role: "user", time: { created } })
const assistant = (id = "a", parentID = "u", created = 2) => ({ id, sessionID: "root", role: "assistant",
  parentID, finish: "stop", time: { created, completed: created + 1 } })
const headers = cursor => new Headers({ "content-type": "application/json", ...(cursor ? { "x-next-cursor": cursor } : {}) })
function pages(infos) {
  const subject = fixture()
  subject.api.client.session.messages = async ({ limit, before }) => {
    assert.equal(limit, 1)
    subject.calls["session.messages"]++
    const end = before ? Number(before) : infos.length
    return { data: end ? [{ info: infos[end - 1] }] : [], response: { headers: headers(end > 1 ? String(end - 1) : undefined) } }
  }
  return subject
}

test("prefix recovery cancels before an unread large tool body and preserves pagination headers", async () => {
  let pulled = 0, cancelled = false
  const response = new Response(new ReadableStream({
    pull(controller) {
      pulled++
      controller.enqueue(Buffer.from(pulled === 1 ? `[{"info":${JSON.stringify(assistant())}` : ",\"parts\":[]}]"))
    }, cancel() { cancelled = true },
  }, { highWaterMark: 0 }), { headers: headers("cursor") })
  const result = await readMessagePrefix(response, true)
  assert.equal(result[0].info.id, "a")
  assert.equal(pulled, 1)
  assert.equal(cancelled, true)
  assert.equal(response.headers.get("x-next-cursor"), "cursor")
})

test("metadata prefix ignores tool bytes already delivered in the same transport-owned chunk", async () => {
  const buffer = Buffer.concat([Buffer.from(`[{"info":${JSON.stringify(assistant())},"parts":[{"output":"`),
    Buffer.alloc(8 * 1024 * 1024, 120), Buffer.from('"}]}]')])
  let cancelled = false
  const result = await readMessagePrefix(new Response(new ReadableStream({
    start(controller) { controller.enqueue(buffer) }, cancel() { cancelled = true },
  }), { headers: headers() }), true)
  assert.ok(JSON.stringify(result).length < 512)
  assert.equal(cancelled, true)
})

test("metadata parsing survives byte boundaries, UTF-8 and escaped delimiters", async () => {
  const info = { ...assistant(), structured: { ignored: 'é \\ \" } [' } }
  const bytes = Buffer.from(` [ { "info" : ${JSON.stringify(info)} , "parts" : [] } ] `)
  let offset = 0
  const response = new Response(new ReadableStream({ pull(controller) {
    if (offset === bytes.length) { controller.close(); return }
    controller.enqueue(bytes.subarray(offset, ++offset))
  } }), { headers: headers() })
  const result = await readMessagePrefix(response, true)
  assert.equal(result[0].info.id, "a")
  assert.equal(result[0].info.structured, undefined)
})

test("unexpected serialization, partial metadata, oversized metadata and excessive nesting refuse authority", async () => {
  for (const body of ['[{"parts":[],"info":{}}]', '[{"i nfo":{}}]', '[{"info":',
    " ".repeat(observationBudget.metadataBytes + 1),
    `[{"info":${JSON.stringify({ ...assistant(), structured: "x".repeat(observationBudget.metadataBytes) })}}]`,
    `[{"info":${"{".repeat(observationBudget.jsonDepth + 1)}}`]) {
    await assert.rejects(readMessagePrefix(new Response(body, { headers: headers() }), true))
  }
  assert.deepEqual(await readMessagePrefix(new Response("[]", { headers: headers() }), true), [])
})

test("response byte limit applies before decoding even with absent or misleading Content-Length", async () => {
  for (const length of [undefined, "1", String(observationBudget.responseBytes + 1)]) {
    let cancelled = false
    const response = new Response(new ReadableStream({ start(controller) {
      controller.enqueue(new Uint8Array(observationBudget.responseBytes + 1))
    }, cancel() { cancelled = true } }), { headers: { "content-type": "application/json",
      ...(length ? { "content-length": length } : {}) } })
    await assert.rejects(readBoundedJSON(response), /byte bound/)
    assert.equal(cancelled, true)
  }
  const body = '"' + "x".repeat(observationBudget.responseBytes - 2) + '"'
  assert.equal((await readBoundedJSON(new Response(body))).length, observationBudget.responseBytes)
})

test("response depth and node limits prevent compact JSON from multiplying parsed allocations", async () => {
  await assert.rejects(readBoundedJSON(new Response("[".repeat(33) + "]".repeat(33))), /depth bound/)
  await assert.rejects(readBoundedJSON(new Response("[" + "{},".repeat(observationBudget.jsonNodes) + "{}]")), /node bound/)
})

test("cancellation wakes a stalled reader and disposes its subscription", async () => {
  const controller = new AbortController()
  let cancelled = false
  const response = new Response(new ReadableStream({ cancel() { cancelled = true } }))
  const reading = readBoundedJSON(response, controller.signal)
  controller.abort()
  await assert.rejects(reading, /cancelled/)
  assert.equal(cancelled, true)
})

test("SDK validator wraps only the resolved request transport, including error paths", async () => {
  const subject = fixture(), budget = new RequestBudget()
  let transportCalls = 0, cancelled = 0, status = 200
  const transport = async request => {
    transportCalls++
    assert.equal(request.headers.get("authorization"), "fixture-token")
    return new Response(new ReadableStream({ pull(controller) {
      controller.enqueue(Buffer.from(status === 200 ? `[{"info":${JSON.stringify(user())},"parts":["private"]}]` : "private-error"))
      controller.close()
    }, cancel() { cancelled++ } }, { highWaterMark: 0 }), { status, headers: headers("cursor") })
  }
  // Same beforeRequest/resolved-fetch/JSON parse ordering as the inspected SDK.
  subject.api.client.session.messages = async (_args, options) => {
    const resolved = { ...options, fetch: transport }
    await resolved.requestValidator(resolved)
    const response = await resolved.fetch(new Request("http://fixture/session/root/message", {
      headers: { authorization: "fixture-token" }, signal: options.signal,
    }))
    return { data: await response.json(), response }
  }
  const api = budgetedAPI(subject.api, budget)
  const result = await api.client.session.messages({ sessionID: "root", limit: 1 })
  assert.equal(result.data[0].info.id, "u")
  assert.ok(!JSON.stringify(result.data).includes("private"))
  assert.equal(result.response.headers.get("x-next-cursor"), "cursor")
  assert.equal(transportCalls, 1)
  assert.equal(budget.calls, 1)
  status = 500
  await assert.rejects(api.client.session.messages({ sessionID: "root", limit: 1 }), /HTTP 500/)
  assert.equal(budget.calls, 2)
  assert.equal(cancelled, 1) // Error response cancelled without reading its body.
  budget.dispose()
})

test("stale/empty local caches cannot establish idle or old Done for a newer authoritative user", async () => {
  const subject = pages([user(), assistant(), user("u2", 3)])
  subject.api.state.session = { messages: () => [user(), assistant()] }
  const recovery = new ObservationRecovery()
  let snapshot = normalize("1.19.0", await recovery.collect(subject.api))
  assert.equal(snapshot.turn_id, "root/u2")
  assert.equal(snapshot.activity, "unknown")
  assert.equal(snapshot.completed_at_ms, undefined)
  subject.api.state.session.messages = () => []
  recovery.invalidateMessages()
  snapshot = normalize("1.19.0", await recovery.collect(subject.api))
  assert.equal(snapshot.completed_at_ms, undefined)
})

test("unchanged head does not authorize cached completion after a backdated inserted user", async () => {
  const infos = [user(), assistant("a", "u", 10)], subject = pages(infos), recovery = new ObservationRecovery()
  assert.ok(normalize("1.19.0", await recovery.collect(subject.api)).completed_at_ms)
  infos.splice(1, 0, user("inserted", 5))
  recovery.invalidateMessages()
  const snapshot = normalize("1.19.0", await recovery.collect(subject.api))
  assert.equal(snapshot.turn_id, "root/inserted")
  assert.equal(snapshot.activity, "unknown")
  assert.equal(snapshot.completed_at_ms, undefined)
})

test("root suffix crossing many tool steps resumes without retaining history or increasing request rate", async () => {
  const infos = [user(), ...Array.from({ length: 30 }, (_, n) => ({ ...assistant(`a${n}`, "u", n + 2), finish: n === 29 ? "stop" : "tool-calls" }))]
  const subject = pages(infos), recovery = new OutcomeRecovery()
  let messages, slices = 0
  while (!messages && slices < 10) {
    const before = subject.count()
    messages = await recovery.sample(subject.api, "root", {})
    assert.ok(subject.count() - before <= observationBudget.historySliceCalls)
    if (recovery.scan) assert.ok(JSON.stringify(recovery.scan).length < 1024)
    slices++
  }
  assert.equal(messages.length, 2)
  assert.equal(messages[0].info.id, "u")
  assert.equal(messages[1].info.id, "a29")
  assert.equal(slices, 8)
  assert.equal(recovery.scan, undefined)
})

test("head change during outcome proof rejects the sample and resets cursor work", async () => {
  const subject = pages([user(), assistant()]), query = subject.api.client.session.messages
  let calls = 0
  subject.api.client.session.messages = async (...args) => ++calls === 3
    ? { data: [{ info: user("new", 3) }], response: { headers: headers() } } : query(...args)
  const recovery = new OutcomeRecovery()
  assert.equal(await recovery.sample(subject.api, "root", {}), undefined)
  assert.equal(recovery.scan, undefined)
})

test("foreign metadata, nondecreasing pages, missing cursor and oversized suffix stay conservative", async () => {
  const scenarios = [
    pages([{ ...user(), sessionID: "foreign" }]),
    pages([assistant("older", "u", 10), assistant("newer", "u", 2)]),
    pages([assistant()]),
    pages(Array.from({ length: 101 }, (_, n) => assistant(`a${n}`, "u", n + 1))),
  ]
  for (const subject of scenarios) {
    const recovery = new OutcomeRecovery()
    await assert.rejects(async () => {
      for (let n = 0; n < 30; n++) await recovery.sample(subject.api, "root", {})
    })
  }
})

test("working metadata can label a turn but cannot promote a stale local terminal assistant", async () => {
  const subject = fixture()
  subject.activity("busy")
  subject.api.state.session = { messages: () => [user(), assistant()] }
  const result = await new ObservationRecovery().collect(subject.api)
  const snapshot = normalize("1.19.0", result)
  assert.equal(snapshot.activity, "working")
  assert.equal(snapshot.turn_id, "root/u")
  assert.equal(snapshot.completed_at_ms, undefined)
  assert.equal(subject.calls["session.messages"], 0)
})

test("API latency cannot silently double quiet topology/history fallback intervals", async () => {
  let time = 0
  const subject = fixture(), recovery = new ObservationRecovery({ now: () => time })
  const session = subject.api.client.session
  for (const method of ["status", "get", "children", "messages"]) {
    const invoke = session[method]
    session[method] = async (...args) => { time += 5; return invoke(...args) }
  }
  await recovery.collect(subject.api)
  assert.ok(time > 0)
  const first = { ...subject.calls }
  time = 10000
  await recovery.collect(subject.api)
  assert.equal(subject.calls["session.messages"], first["session.messages"] + 1)
  assert.equal(subject.calls["session.children"], first["session.children"] + 2)
})

test("standalone worker reads indexed metadata only, never invokes message-body RPC, and sees fresh WAL updates", async () => {
  const root = mkdtempSync(path.join(tmpdir(), "verij-worker-metadata-"))
  const filename = path.join(root, "metadata.db"), oldDB = process.env.OPENCODE_DB
  const oldChannel = process.env.OPENCODE_DISABLE_CHANNEL_DB
  process.env.OPENCODE_DB = filename
  const writer = new DatabaseSync(filename)
  writer.exec(`PRAGMA journal_mode = WAL;
    CREATE TABLE session (id TEXT PRIMARY KEY);
    CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
    CREATE TABLE part (message_id TEXT, data TEXT);
    CREATE INDEX message_session_time_created_id_idx ON message(session_id, time_created, id);
    INSERT INTO session VALUES ('root');`)
  const insert = writer.prepare("INSERT INTO message VALUES (?, 'root', ?, ?)")
  insert.run("u", 1, JSON.stringify(user()))
  insert.run("a", 2, JSON.stringify(assistant()))
  writer.prepare("INSERT INTO part VALUES ('a', ?)").run("private-output".repeat(100000))
  const subject = fixture(), budget = new RequestBudget()
  subject.api.state.path = {
    state: path.join(process.env.XDG_STATE_HOME ?? path.join(tmpdir(), "unmatched"), "opencode"),
    config: process.env.OPENCODE_CONFIG_DIR ?? path.join(process.env.XDG_CONFIG_HOME ?? path.join(tmpdir(), "unmatched"), "opencode"),
    directory: process.cwd(),
  }
  // Resolve defaults exactly as the installed worker storage contract does.
  const os = await import("node:os")
  if (!process.env.XDG_STATE_HOME) subject.api.state.path.state = path.join(os.homedir(), ".local/state/opencode")
  if (!process.env.XDG_CONFIG_HOME && !process.env.OPENCODE_CONFIG_DIR) subject.api.state.path.config = path.join(os.homedir(), ".config/opencode")
  let rpcCalls = 0
  subject.api.client.session.messages = async (parameters, options) => {
    const resolved = { ...options, fetch: async () => { rpcCalls++; throw Error("Message body RPC forbidden") } }
    await resolved.requestValidator(resolved)
    const query = new URLSearchParams({ limit: String(parameters.limit), ...(parameters.before ? { before: parameters.before } : {}) })
    const response = await resolved.fetch(new Request(`http://opencode.internal/session/root/message?${query}`, { signal: options.signal }))
    return { data: await response.json(), response }
  }
  const api = budgetedAPI(subject.api, budget)
  try {
    let messages = await new OutcomeRecovery().sample(api, "root", {})
    assert.equal(messages[1].info.id, "a")
    assert.equal(rpcCalls, 0)
    assert.equal(budget.calls, 3) // Local metadata invocations still consume observation budget.
    insert.run("u2", 3, JSON.stringify(user("u2", 3)))
    messages = await new OutcomeRecovery().sample(api, "root", {})
    assert.equal(messages[0].info.id, "u2")
    assert.equal(messages.length, 1)
    const readonly = new WorkerMetadata(subject.api)
    const request = new Request("http://opencode.internal/session/root/message?limit=1")
    await readonly.page(request)
    assert.throws(() => readonly.db.exec("DELETE FROM message"), /readonly/)
    insert.run("oversized", 4, JSON.stringify({ ...user("oversized", 4), system: "x".repeat(observationBudget.metadataBytes) }))
    await assert.rejects(readonly.page(request), /byte bound/)
    writer.prepare("DELETE FROM message WHERE id = 'oversized'").run()
    writer.exec("DROP INDEX message_session_time_created_id_idx")
    await assert.rejects(readonly.page(request), /schema changed/)
    await assert.rejects(readonly.page(request), /Unindexed/)
    readonly.dispose()
    subject.api.state.path.state = root
    const unverified = new WorkerMetadata(subject.api)
    await assert.rejects(unverified.page(new Request("http://opencode.internal/session/root/message?limit=1")), /scope/)
    unverified.dispose()
    const count = writer.prepare("SELECT count(*) AS n FROM message").get().n
    assert.equal(count, 3)
  } finally {
    budget.dispose(); writer.close()
    if (oldDB === undefined) delete process.env.OPENCODE_DB; else process.env.OPENCODE_DB = oldDB
    if (oldChannel === undefined) delete process.env.OPENCODE_DISABLE_CHANNEL_DB; else process.env.OPENCODE_DISABLE_CHANNEL_DB = oldChannel
    rmSync(root, { recursive: true, force: true })
  }
})
