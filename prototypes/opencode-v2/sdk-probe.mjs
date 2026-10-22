// P0 characterization of the exact tagged SDK, with synthetic fetch only.
// No CLI connection, provider, shared client, global fetch mutation, or secrets.
import { readFile, realpath, lstat } from "node:fs/promises"
import { resolve, isAbsolute, join } from "node:path"
import { pathToFileURL, fileURLToPath } from "node:url"
import { createHash } from "node:crypto"

export const PIN = Object.freeze({
  version: "2.0.32",
  commit: "843a6c514e606b436a7fe713422ace0a93ad3d61",
  sources: Object.freeze({
    "client.ts": "9e4da23f8bcd598b45c6fe589ecaf434e3a8561bca9913cf1e4ec332d80aff36",
    "client-error.ts": "1c02bcfc01072693c43e71cc53c8dcca5d8d75327e09099cac9b37a0317ace2d",
  }),
})
export const CASES = Object.freeze([
  "metadata_over_64k", "success_over_1m", "declared_error_over_1m",
  "depth_over_32", "nodes_over_16384", "prefix_and_auth", "global_fetch_unchanged",
])
const HERE = fileURLToPath(import.meta.url)
const hash = value => createHash("sha256").update(value).digest("hex")
const fail = code => { throw new Error(code) }

export async function verifySources(directory) {
  if (typeof directory !== "string" || !isAbsolute(directory) || await realpath(directory) !== directory) fail("source_path")
  for (const [name, digest] of Object.entries(PIN.sources)) {
    const path = join(directory, name)
    const stat = await lstat(path)
    if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 128 * 1024 || hash(await readFile(path)) !== digest) fail("source_pin")
  }
  // Bun's .js -> .ts fallback must not resolve an unpinned sibling instead.
  for (const name of ["client-error.js", "client-error.jsx", "client-error.json", "client-error.node"]) {
    try { await lstat(join(directory, name)) } catch (error) {
      if (error.code === "ENOENT") continue
      fail("source_path")
    }
    fail("source_shadow")
  }
  return join(directory, "client.ts")
}

export async function characterize(make) {
  const originalFetch = globalThis.fetch
  const cases = []
  const run = async (name, data, check, errorStatus = false) => {
    let requested = false
    const client = make({
      baseUrl: "http://fixture.invalid/private-prefix",
      headers: { authorization: "Basic fixture-only-not-a-credential" },
      fetch: async (url, options) => {
        if (requested || options.method !== "GET" || String(url) !== "http://fixture.invalid/private-prefix/api/session/fixture") fail("synthetic_request")
        requested = true
        return new Response(JSON.stringify(data), { status: errorStatus ? 400 : 200,
          headers: { "content-type": "application/json" } })
      },
    })
    let accepted = false
    if (errorStatus) {
      try { await client.session.get({ sessionID: "fixture" }) } catch (error) {
        accepted = check(error)
      }
    } else accepted = check(await client.session.get({ sessionID: "fixture" }))
    if (!requested || accepted !== true) fail("characterization_failed")
    cases.push({ name, observed: true })
  }
  await run("metadata_over_64k", { data: { metadata: { value: "x".repeat(65537) } } },
    value => value.metadata.value.length === 65537)
  await run("success_over_1m", { data: { metadata: { value: "x".repeat(1048577) } } },
    value => value.metadata.value.length === 1048577)
  await run("declared_error_over_1m", { _tag: "FixtureError", data: { message: "x".repeat(1048577) } },
    error => error.data?.message?.length === 1048577, true)
  let nested = { end: true }
  for (let index = 0; index < 33; index++) nested = { nested }
  await run("depth_over_32", { data: nested }, value => {
    for (let index = 0; index < 33; index++) value = value?.nested
    return value?.end === true
  })
  await run("nodes_over_16384", { data: { nodes: Array.from({ length: 16385 }, () => ({})) } },
    value => value.nodes.length === 16385)
  let preserved = false
  const client = make({ baseUrl: "http://fixture.invalid/private-prefix", headers: { authorization: "Basic fixture-only-not-a-credential" },
    fetch: async (url, options) => {
      preserved = String(url) === "http://fixture.invalid/private-prefix/api/session/fixture" &&
        new Headers(options.headers).get("authorization") === "Basic fixture-only-not-a-credential" && options.method === "GET"
      return new Response('{"data":{}}', { headers: { "content-type": "application/json" } })
    } })
  await client.session.get({ sessionID: "fixture" })
  if (!preserved || globalThis.fetch !== originalFetch) fail("synthetic_isolation")
  cases.push({ name: "prefix_and_auth", observed: true }, { name: "global_fetch_unchanged", observed: true })
  return cases
}

export async function validateReport(report) {
  const keys = (value, expected) => {
    if (!value || typeof value !== "object" || Array.isArray(value) || Object.keys(value).sort().join(" ") !== expected.split(" ").sort().join(" ")) fail("report_fields")
  }
  keys(report, "schema target sources artifact_sha256 status evidence_kind production_admission connected_transport_qualified cases")
  keys(report.target, "version commit")
  keys(report.sources, "client.ts client-error.ts")
  if (report.schema !== 1 || report.target.version !== PIN.version || report.target.commit !== PIN.commit ||
      Object.entries(PIN.sources).some(([key, value]) => report.sources[key] !== value) ||
      report.artifact_sha256 !== hash(await readFile(HERE)) || report.status !== "CHARACTERIZED" ||
      report.evidence_kind !== "synthetic-tagged-sdk" || report.production_admission !== false ||
      report.connected_transport_qualified !== false || !Array.isArray(report.cases) || report.cases.length !== CASES.length) fail("report_contract")
  report.cases.forEach((item, index) => {
    keys(item, "name observed")
    if (item.name !== CASES[index] || item.observed !== true) fail("report_case")
  })
  return { status: "CHARACTERIZED", cases: CASES.length, production_admission: false, connected_transport_qualified: false }
}

export function parseReport(raw) {
  if (!Buffer.isBuffer(raw) || raw.length > 16384) fail("report_bound")
  let depth = 0, nodes = 1, quoted = false, escaped = false
  for (const byte of raw) {
    if (quoted) {
      if (escaped) escaped = false
      else if (byte === 92) escaped = true
      else if (byte === 34) quoted = false
    } else if (byte === 34) quoted = true
    else if (byte === 91 || byte === 123) { depth++; nodes++ }
    else if (byte === 93 || byte === 125) depth--
    else if (byte === 44 || byte === 58) nodes++
    if (depth > 32 || nodes > 16384) fail("report_bound")
  }
  return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(raw))
}

export async function probe(directory) {
  const source = await verifySources(directory)
  const { make } = await import(pathToFileURL(source).href)
  const cases = await characterize(make)
  await verifySources(directory)
  const report = { schema: 1, target: { version: PIN.version, commit: PIN.commit }, sources: { ...PIN.sources },
    artifact_sha256: hash(await readFile(HERE)), status: "CHARACTERIZED", evidence_kind: "synthetic-tagged-sdk",
    production_admission: false, connected_transport_qualified: false, cases }
  await validateReport(report)
  return report
}

async function main(args) {
  if (args.length === 2 && args[0] === "--source-dir") {
    if (!globalThis.Bun) fail("bun_required_for_tagged_typescript")
    console.log(JSON.stringify(await probe(args[1]), null, 2))
  } else if (args.length === 2 && args[0] === "--verify") {
    const stat = await lstat(args[1])
    if (stat.size > 16384) fail("report_bound")
    console.log(JSON.stringify(await validateReport(parseReport(await readFile(args[1]))), null, 2))
  } else fail("usage_source_dir_or_verify")
}
if (process.argv[1] && resolve(process.argv[1]) === HERE) main(process.argv.slice(2)).catch(() => {
  // Even an unexpected SDK exception must not expose its body/URL/credentials.
  console.error("SDK probe failed; no support admission")
  process.exitCode = 1
})
