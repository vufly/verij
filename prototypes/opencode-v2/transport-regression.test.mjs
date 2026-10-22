// Independent follow-up regressions for transport review findings.
import test from "node:test"
import assert from "node:assert/strict"
import http from "node:http"
import { createReadOnlyTransport } from "./bounded-transport.mjs"

const tick = () => new Promise(resolve => setImmediate(resolve))
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done }); return { promise, resolve } }
const make = fetch => createReadOnlyTransport({ baseUrl: "http://fixture.invalid/prefix", headers: {}, fetch, deadlineMs: 25 })

test("every early response cancellation retains its physical slot until cancellation settles", async () => {
  for (const kind of ["http", "redirect", "content_type", "advertised_bound", "late_response"]) {
    const cancellation = deferred(), responseReady = deferred()
    let cancelled = false
    const body = new ReadableStream({ cancel() { cancelled = true; return cancellation.promise } })
    const response = new Response(body, { status: kind === "http" ? 400 : kind === "redirect" ? 302 : 200,
      headers: { "content-type": kind === "content_type" ? "text/plain" : "application/json",
        ...(kind === "advertised_bound" ? { "content-length": "1048577" } : {}) } })
    const transport = make(() => kind === "late_response" ? responseReady.promise : Promise.resolve(response))
    const request = transport.get("api/info")
    await assert.rejects(request, error => error.code === "deadline")
    if (kind === "late_response") responseReady.resolve(response)
    await tick()
    assert.equal(cancelled, true, kind)
    assert.equal(transport.metrics().active, 1, kind)
    cancellation.resolve()
    await tick()
    assert.equal(transport.metrics().active, 0, kind)
    transport.dispose()
  }
})

test("aborted reader reuses the original pending cancellation promise", async () => {
  const cancellation = deferred()
  const transport = make(() => Promise.resolve(new Response(new ReadableStream({ cancel() { return cancellation.promise } }),
    { headers: { "content-type": "application/json" } })))
  await assert.rejects(transport.get("api/info"), error => error.code === "deadline")
  await tick()
  assert.equal(transport.metrics().active, 1)
  cancellation.resolve()
  await tick()
  assert.equal(transport.metrics().active, 0)
  transport.dispose()
})

test("input bounds and accessors fail closed without invoking fetch or getters", async () => {
  let calls = 0, getters = 0
  const fetch = () => { calls++; return Promise.resolve(new Response("{}")) }
  const dangerous = Object.defineProperty({}, "authorization", { enumerable: true, get() { getters++; throw Error("PRIVATE") } })
  assert.throws(() => createReadOnlyTransport({ baseUrl: "http://fixture.invalid/", headers: dangerous, fetch }), /accessors/)
  assert.throws(() => createReadOnlyTransport({ baseUrl: "http://fixture.invalid/", headers: new Date(), fetch }), /plain/)
  const transport = make(fetch)
  for (const path of ["x".repeat(4097), "api/%252e%252e", "api/%2f..", "api/\t../info"]) {
    await assert.rejects(transport.get(path), error => error.code === "schema")
  }
  await assert.rejects(transport.get("api/info", { query: { cursor: "x".repeat(4097) } }), error => error.code === "schema")
  await assert.rejects(transport.get("api/info", { query: dangerous }), error => error.code === "schema")
  assert.equal(calls, 0)
  assert.equal(getters, 0)
  transport.dispose()
})

test("real HTTP redirect:error never forwards auth to the redirect target", async () => {
  let targetHits = 0
  const target = http.createServer((_request, response) => { targetHits++; response.end("{}") })
  await new Promise(resolve => target.listen(0, "127.0.0.1", resolve))
  const source = http.createServer((_request, response) => {
    response.writeHead(302, { location: `http://127.0.0.1:${target.address().port}/private` })
    response.end()
  })
  await new Promise(resolve => source.listen(0, "127.0.0.1", resolve))
  const transport = createReadOnlyTransport({ baseUrl: `http://127.0.0.1:${source.address().port}/prefix`,
    headers: { authorization: "Basic fixture-only" }, fetch: globalThis.fetch })
  try {
    await assert.rejects(transport.get("api/info"))
    assert.equal(targetHits, 0)
  } finally {
    transport.dispose()
    source.closeAllConnections(); target.closeAllConnections()
    await Promise.all([new Promise(resolve => source.close(resolve)), new Promise(resolve => target.close(resolve))])
  }
})
