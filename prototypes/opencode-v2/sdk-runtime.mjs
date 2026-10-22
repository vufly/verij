// Tagged SDK on an explicitly owned headless connection; never CLI admission.
import { pathToFileURL, fileURLToPath } from "node:url"
import { resolve } from "node:path"
import { createHash } from "node:crypto"
import { parseReport, verifySources } from "./sdk-probe.mjs"
import { observationFetch } from "./sdk-observation.mjs"

export const CHECKS = Object.freeze([
  "session_get_unwrapped", "active_unwrapped", "parent_pagination", "message_cursor_order",
  "permission_location_scoping", "permission_session_pending", "form_location_scoping",
  "form_session_unwrapped", "pending_inbox_unwrapped", "preaborted_cancellation",
  "physical_request_accounting", "global_fetch_unchanged",
])
const key = value => {
  if (typeof value !== "string" || !value.length || value.length > 4096) throw new Error("identity_bound")
  return createHash("sha256").update(value).digest("hex").slice(0, 20)
}
const require = condition => { if (!condition) throw new Error("runtime_contract") }
const ids = values => {
  require(Array.isArray(values) && values.length <= 8)
  return values.map(value => key(value.id))
}

export async function observeShapes(client, config, observation) {
  const checks = []
  const check = (name, condition) => { require(condition); checks.push(name) }
  const root = await client.session.get({ sessionID: config.root })
  check(CHECKS[0], root.id === config.root && root.data === undefined && root.outcome === "succeeded")
  const active = await client.session.active()
  check(CHECKS[1], active && typeof active === "object" && !Array.isArray(active) &&
    Object.hasOwn(active, config.active) && !Object.hasOwn(active, config.root) && active.data === undefined)
  const paginate = async (method, initial) => {
    let cursor, pages = 0
    const data = [], seen = new Set(), cursors = new Set()
    for (; pages < 8; pages++) {
      const page = await method(cursor ? { cursor, limit: 1 } : { ...initial, limit: 1 })
      require(Array.isArray(page.data) && page.data.length <= 1)
      if (!page.data.length) return { data, pages: pages + 1 }
      const item = page.data[0]
      require(typeof item.id === "string" && item.id.length <= 256 && !seen.has(item.id))
      seen.add(item.id)
      data.push(item)
      cursor = page.cursor?.next
      require(typeof cursor === "string" && cursor.length > 0 && cursor.length <= 4096 && !cursors.has(cursor))
      cursors.add(cursor)
    }
    throw new Error("page_bound")
  }
  const children = await paginate(client.session.list, { parentID: config.root, order: "desc" })
  check(CHECKS[2], children.data.length === 3 && children.data.every(item => item.parentID === config.root)
    && [...children.data.map(item => item.id)].sort().join() === [...config.children].sort().join())
  const listMessages = input => client.message.list({ sessionID: config.root, ...input })
  const descending = await paginate(listMessages, { order: "desc" })
  const ascending = await paginate(listMessages, { order: "asc" })
  check(CHECKS[3], descending.data.length === 3 && descending.data[0].type === "idle"
    && descending.data[0].outcome === "succeeded" && ids(descending.data).join() === ids(ascending.data).reverse().join())

  const permissionsA = await client.permission.request.list({ location: { directory: config.locationA } })
  const permissionsB = await client.permission.request.list({ location: { directory: config.locationB } })
  check(CHECKS[4], permissionsA.location?.directory === config.locationA && permissionsB.location?.directory === config.locationB
    && ids(permissionsA.data).join() === key(config.permissionA) && ids(permissionsB.data).join() === key(config.permissionB))
  // Tagged permission.list lists pending session requests, NOT permission rules.
  const perSession = await client.permission.list({ sessionID: config.root })
  check(CHECKS[5], ids(perSession).join() === key(config.permissionA))
  const formsA = await client.form.list({ location: { directory: config.locationA } })
  const formsB = await client.form.list({ location: { directory: config.locationB } })
  check(CHECKS[6], formsA.location?.directory === config.locationA && formsB.location?.directory === config.locationB
    && ids(formsA.data).join() === key(config.formA) && ids(formsB.data).join() === key(config.formB))
  const forms = await client.session.form.list({ sessionID: config.root })
  check(CHECKS[7], ids(forms).join() === key(config.formA))
  const inbox = await client.session.inbox.list({ sessionID: config.root })
  check(CHECKS[8], ids(inbox).join() === key(config.pending))

  const before = observation.metrics()
  const controller = new AbortController()
  controller.abort()
  let rejected = false
  try { await client.session.get({ sessionID: config.root }, { signal: controller.signal }) } catch { rejected = true }
  const after = observation.metrics()
  check(CHECKS[9], rejected && after.calls === before.calls && after.invocations === before.invocations + 1)
  await Promise.resolve()
  const metrics = observation.metrics()
  check(CHECKS[10], metrics.invocations === 22 && metrics.calls === 21 && metrics.errors === 0 && metrics.active === 0 && metrics.pending === 0)
  return { checks, root: key(root.id), root_outcome: root.outcome, marker_outcome: descending.data[0].outcome,
    active: Object.keys(active).map(key),
    children: { ids: ids(children.data), pages: children.pages },
    messages: { ids: ids(descending.data), types: descending.data.map(item => item.type), ascending_ids: ids(ascending.data),
      descending_pages: descending.pages, ascending_pages: ascending.pages },
    permissions: { location_a: key(permissionsA.location.directory), location_b: key(permissionsB.location.directory),
      a: ids(permissionsA.data), b: ids(permissionsB.data), session: ids(perSession) },
    forms: { location_a: key(formsA.location.directory), location_b: key(formsB.location.directory),
      a: ids(formsA.data), b: ids(formsB.data), session: ids(forms) },
    inbox: ids(inbox), metrics, preaborted_rejected: rejected }
}

async function main(args) {
  require(args.length === 2 && args[0] === "--source-dir" && globalThis.Bun)
  const source = await verifySources(args[1])
  let size = 0, chunks = []
  for await (const chunk of process.stdin) {
    size += chunk.length
    require(size <= 8192)
    chunks.push(chunk)
  }
  const config = parseReport(Buffer.concat(chunks))
  const originalFetch = globalThis.fetch
  const observation = observationFetch({ baseUrl: config.baseUrl, headers: config.headers, fetch: originalFetch })
  try {
    const { make } = await import(pathToFileURL(source).href)
    const client = make({ baseUrl: config.baseUrl, headers: config.headers, fetch: observation.fetch })
    const report = await observeShapes(client, config, observation)
    require(globalThis.fetch === originalFetch)
    report.checks.push(CHECKS[11])
    report.global_fetch_unchanged = true
    report.production_admission = false
    report.connected_transport_qualified = false
    await verifySources(args[1])
    const output = JSON.stringify(report)
    require(Buffer.byteLength(output) + 1 <= 8192)
    process.stdout.write(output + "\n")
  } finally { observation.dispose() }
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main(process.argv.slice(2)).catch(() => {
  console.error("Private tagged-SDK runtime check failed; no support admission")
  process.exitCode = 1
})
