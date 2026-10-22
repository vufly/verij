import test from "node:test"
import assert from "node:assert/strict"
import { mkdtemp, writeFile, readFile, rm, symlink } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { createHash } from "node:crypto"
import { spawnSync } from "node:child_process"
import { PIN, CASES, verifySources, characterize, validateReport, parseReport } from "./sdk-probe.mjs"

const HERE = new URL("./sdk-probe.mjs", import.meta.url)
const digest = () => readFile(HERE).then(raw => createHash("sha256").update(raw).digest("hex"))
const report = async () => ({ schema: 1, target: { version: PIN.version, commit: PIN.commit }, sources: { ...PIN.sources },
  artifact_sha256: await digest(), status: "CHARACTERIZED", evidence_kind: "synthetic-tagged-sdk",
  production_admission: false, connected_transport_qualified: false, cases: CASES.map(name => ({ name, observed: true })) })

// Test the characterization driver separately from the actual pinned TS import.
const fakeMake = options => ({ session: { get: async input => {
  const response = await options.fetch(`${options.baseUrl}/api/session/${input.sessionID}`,
    { method: "GET", headers: options.headers })
  const body = JSON.parse(await response.text())
  if (!response.ok) throw Object.assign(new Error("fixture"), body)
  return body.data
} } })

test("driver records only fixed case names and flags without replacing global fetch", async () => {
  const original = globalThis.fetch
  const cases = await characterize(fakeMake)
  assert.equal(globalThis.fetch, original)
  assert.deepEqual(cases, CASES.map(name => ({ name, observed: true })))
  assert.equal(JSON.stringify(cases).includes("fixture-only-not-a-credential"), false)
})

test("a bounded fake SDK cannot be misreported as accepting oversized bodies", async () => {
  const bounded = options => ({ session: { get: async () => {
    const response = await options.fetch(`${options.baseUrl}/api/session/fixture`, { method: "GET" })
    if ((await response.text()).length > 65536) throw new Error("bound")
    return {}
  } } })
  await assert.rejects(characterize(bounded), /bound/)
})

test("source hash and absolute-path checks fail closed before import", async () => {
  const directory = await mkdtemp(join(tmpdir(), "vj-v2-sdk-test-"))
  try {
    await writeFile(join(directory, "client.ts"), 'throw new Error("MUST_NOT_EXECUTE")')
    await assert.rejects(verifySources(directory), /source_pin/)
    await assert.rejects(verifySources("relative"), /source_path/)
    const link = directory + "-link"
    await symlink(directory, link)
    try { await assert.rejects(verifySources(link), /source_path/) } finally { await rm(link) }
  } finally { await rm(directory, { recursive: true, force: true }) }
})

test("retained characterization matches pinned source and current probe", async () => {
  const retained = parseReport(await readFile(new URL("./sdk-results.json", import.meta.url)))
  assert.equal((await validateReport(retained)).status, "CHARACTERIZED")
})

test("characterization is explicitly synthetic and cannot promote production admission", async () => {
  const item = await report()
  assert.deepEqual(await validateReport(item), { status: "CHARACTERIZED", cases: 7,
    production_admission: false, connected_transport_qualified: false })
  for (const field of ["production_admission", "connected_transport_qualified"]) {
    await assert.rejects(validateReport({ ...item, [field]: true }), /report_contract/)
  }
})

test("unapproved body fields, strings, sources and missing cases are rejected", async () => {
  const mutations = [r => { r.prompt = "PRIVATE" }, r => { r.target.prompt = "PRIVATE" },
    r => { r.sources["extra.ts"] = "PRIVATE" }, r => { r.cases[0].body = "PRIVATE" },
    r => { r.cases[0].name = "PRIVATE" }, r => { r.cases[0].observed = false },
    r => { r.cases.pop() }, r => { r.artifact_sha256 = "0".repeat(64) },
    r => { r.sources["client.ts"] = "0".repeat(64) }]
  for (const mutate of mutations) {
    const item = await report()
    mutate(item)
    await assert.rejects(validateReport(item))
  }
})

test("evidence bounds precede JSON parsing", () => {
  for (const raw of [Buffer.alloc(16385), Buffer.from("[".repeat(33) + "0" + "]".repeat(33))]) {
    assert.throws(() => parseReport(raw), /report_bound/)
  }
  assert.deepEqual(parseReport(Buffer.from('{"ok":true}')), { ok: true })
})

test("invalid CLI arguments cannot trigger a fetch or print private values", () => {
  const result = spawnSync(process.execPath, [HERE.pathname, "--unexpected", "PRIVATE_SECRET_SENTINEL"],
    { encoding: "utf8", env: { PATH: "/usr/bin:/bin" }, timeout: 3000 })
  assert.equal(result.status, 1)
  assert.equal(result.stdout, "")
  assert.equal(result.stderr.trim(), "SDK probe failed; no support admission")
})
