// Private controller supplies endpoint/auth via bounded stdin, never argv/files.
// This qualifies one explicitly provisioned headless connection, not CLI provenance.
import { createHash } from "node:crypto"
import { createReadOnlyTransport } from "./bounded-transport.mjs"

const key = value => createHash("sha256").update(value).digest("hex").slice(0, 20)
async function main() {
  let input = ""
  for await (const chunk of process.stdin) {
    input += chunk.toString("utf8")
    if (Buffer.byteLength(input) > 8192) throw Error("control_bound")
  }
  const config = JSON.parse(input)
  if (typeof config.sessionID !== "string" || !/^[a-zA-Z0-9_-]{1,256}$/.test(config.sessionID) ||
      typeof config.marker !== "string" || !/^[0-9a-f]{20}$/.test(config.marker)) throw Error("control_schema")
  const transport = createReadOnlyTransport({ baseUrl: config.baseUrl, headers: config.headers, fetch: globalThis.fetch })
  try {
    const path = `api/session/${encodeURIComponent(config.sessionID)}`
    const info = JSON.parse(await transport.get(path, { outcome: true }))
    const page = JSON.parse(await transport.get(path + "/message", { outcome: true, query: { limit: 1, order: "desc" } }))
    if (info.data?.id !== config.sessionID || info.data.outcome !== "succeeded" || page.data?.length !== 1 ||
        page.data[0].type !== "idle" || page.data[0].outcome !== "succeeded" || key(page.data[0].id) !== config.marker) throw Error("observation_schema")
    await Promise.resolve()
    const metrics = transport.metrics()
    if (metrics.calls !== 2 || metrics.errors !== 0 || metrics.active !== 0 || metrics.pending !== 0) throw Error("request_accounting")
    console.log(JSON.stringify({ explicit_private_transport_pass: true, calls: metrics.calls,
      connected_cli_transport_qualified: false }))
  } finally { transport.dispose() }
}
main().catch(() => { console.error("Private transport check failed"); process.exitCode = 1 })
