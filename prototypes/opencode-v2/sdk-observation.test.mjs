import test from "node:test"
import assert from "node:assert/strict"
import { observationFetch } from "./sdk-observation.mjs"

const baseUrl = "http://fixture.invalid/private-prefix"
const headers = { authorization: "Basic fixture-only-secret" }
const input = "http://fixture.invalid/private-prefix/api/session/fixture"
const options = signal => ({ method: "GET", headers, ...(signal ? { signal } : {}) })
const deferred = () => { let resolve; const promise = new Promise(r => { resolve = r }); return { promise, resolve } }
const tick = () => new Promise(resolve => setImmediate(resolve))

test("independent SDK fetch preserves prefix, bracketed location query and exact credentials", async () => {
  const original = globalThis.fetch
  const calls = []
  const observation = observationFetch({ baseUrl, headers, fetch: async (url, init) => {
    calls.push({ url: String(url), init })
    return Response.json({ data: { id: "fixture" } })
  } })
  try {
    const response = await observation.fetch(input + "?location%5Bdirectory%5D=%2Fprivate&limit=1", options())
    assert.deepEqual(await response.json(), { data: { id: "fixture" } })
    assert.equal(calls.length, 1)
    assert.equal(calls[0].url, input + "?location%5Bdirectory%5D=%2Fprivate&limit=1")
    assert.equal(calls[0].init.method, "GET")
    assert.equal(calls[0].init.redirect, "error")
    assert.equal(new Headers(calls[0].init.headers).get("authorization"), headers.authorization)
    assert.equal(globalThis.fetch, original)
    assert.deepEqual(observation.metrics(), { invocations: 1, calls: 1, errors: 0, active: 0, pending: 0 })
    assert.doesNotMatch(JSON.stringify(observation.metrics()), /secret|authorization|private-prefix/)
  } finally { observation.dispose() }
})

test("method, body, header, authority and prefix changes fail before physical fetch", async () => {
  for (const [url, init] of [
    [input, { ...options(), method: "POST" }], [input, { ...options(), body: "PRIVATE_BODY" }],
    [input, { ...options(), headers: { authorization: "changed" } }],
    [input, { ...options(), headers: { ...headers, extra: "added" } }],
    ["http://foreign.invalid/private-prefix/api/session/fixture", options()],
    ["http://fixture.invalid/api/session/fixture", options()],
    ["http://fixture.invalid/private-prefix-other/api/session/fixture", options()],
    [input + "#private", options()], ["http://user:private@fixture.invalid/private-prefix/api/session/fixture", options()],
  ]) {
    let physical = 0
    const observation = observationFetch({ baseUrl, headers, fetch: async () => { physical++; return Response.json({}) } })
    try {
      await assert.rejects(observation.fetch(url, init), { message: "observation_request" })
      assert.equal(physical, 0)
      assert.equal(observation.metrics().calls, 0)
    } finally { observation.dispose() }
  }
})

test("success byte/depth/node guards run before a Response reaches SDK parsing", async () => {
  for (const text of [JSON.stringify({ data: "x".repeat(65537) }), "[".repeat(33) + "0" + "]".repeat(33),
    "[" + "0,".repeat(16385) + "0]"]) {
    let handedToParser = false
    const observation = observationFetch({ baseUrl, headers, fetch: async () => new Response(text, { headers: { "content-type": "application/json" } }) })
    try {
      await assert.rejects(observation.fetch(input, options()).then(response => { handedToParser = true; return response.text() }), { code: "bound" })
      assert.equal(handedToParser, false)
    } finally { observation.dispose() }
  }
})

test("upstream errors cancel without parsing or retaining private error bodies", async () => {
  assert.throws(() => observationFetch({ baseUrl, headers }), { message: "observation_fetch_required" })
  for (const status of [201, 202, 204, 206]) {
    let cancelled = false
    const altered = observationFetch({ baseUrl, headers, fetch: async () => ({
      status, ok: true, body: { async cancel() { cancelled = true } },
    }) })
    try {
      await assert.rejects(altered.fetch(input, options()), { code: "schema" })
      assert.equal(cancelled, true)
    } finally { altered.dispose() }
  }
  let cancelled = false, parsed = false
  const observation = observationFetch({ baseUrl, headers, fetch: async () => ({
    status: 400, ok: false,
    body: { async cancel() { cancelled = true } },
    async text() { parsed = true; return "PRIVATE_PROVIDER_ERROR" },
  }) })
  try {
    await assert.rejects(observation.fetch(input, options()), error => error.code === "http" && !/PRIVATE|secret|private-prefix/.test(error.message))
    assert.equal(cancelled, true)
    assert.equal(parsed, false)
  } finally { observation.dispose() }
})

test("preaborted SDK invocation charges no physical request", async () => {
  const controller = new AbortController(); controller.abort()
  const observation = observationFetch({ baseUrl, headers, fetch: async () => { throw Error("must not call") } })
  try {
    await assert.rejects(observation.fetch(input, options(controller.signal)), { code: "abort" })
    assert.deepEqual(observation.metrics(), { invocations: 1, calls: 0, errors: 0, active: 0, pending: 0 })
  } finally { observation.dispose() }
})

test("in-flight caller abort retains physical SDK slot until real body cancellation settles", async () => {
  const entered = deferred(), cancelStarted = deferred(), release = deferred()
  const observation = observationFetch({ baseUrl, headers, fetch: async () => {
    entered.resolve()
    return new Response(new ReadableStream({ cancel() { cancelStarted.resolve(); return release.promise } }),
      { headers: { "content-type": "application/json" } })
  } })
  const controller = new AbortController()
  const request = observation.fetch(input, options(controller.signal))
  const rejected = assert.rejects(request, { code: "abort" })
  try {
    await entered.promise; await tick()
    controller.abort()
    await rejected; await cancelStarted.promise
    assert.equal(observation.metrics().active, 1)
    release.resolve()
    await tick(); await tick()
    assert.equal(observation.metrics().active, 0)
  } finally { release.resolve(); observation.dispose() }
})

test("disposal and deadline cancel the independent client without mutating global fetch", async () => {
  const entered = deferred(), original = globalThis.fetch
  const observation = observationFetch({ baseUrl, headers, fetch: async (_url, init) => {
    entered.resolve()
    return new Promise((_, reject) => init.signal.addEventListener("abort", () => reject(Error("PRIVATE_ABORT")), { once: true }))
  } })
  const request = observation.fetch(input, options())
  const rejected = assert.rejects(request, { code: "cancelled" })
  await entered.promise
  observation.dispose(); observation.dispose()
  await rejected
  assert.equal(globalThis.fetch, original)
  await assert.rejects(observation.fetch(input, options()), { code: "cancelled" })

  const short = observationFetch({ baseUrl, headers, deadlineMs: 5, fetch: async (_url, init) => new Promise((_, reject) => {
    init.signal.addEventListener("abort", () => reject(Error("PRIVATE_STALL")), { once: true })
  }) })
  try { await assert.rejects(short.fetch(input, options()), { code: "deadline" }) } finally { short.dispose() }
})
