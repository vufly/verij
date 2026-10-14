// Count bridge API invocations and owned runtime events without retaining payloads.
import { writeFileSync, renameSync } from "node:fs"
import bridge from "../../integrations/opencode/bridge.mjs"
import { events } from "../../integrations/opencode/core.mjs"
export default {
  // Keep production ID so H3's real plugin disable/enable controls this wrapper.
  id: "verij.monitoring.v1",
  async tui(api) {
    const path = `${process.env.VERIJ_RESOURCE_ROOT}/metrics/${process.env.VERIJ_RESOURCE_PER_PROCESS === "1" ? `api-${process.pid}` : "api"}.json`
    try {
    const endpoints = Object.fromEntries(["session.status", "session.get", "session.children",
      "session.messages", "permission.list", "question.list"].map(name => [name, 0]))
    let incoming = 0
    const transport = Object.fromEntries(Object.keys(endpoints).map(name => [name, { bytes: 0, reads: 0, cancellations: 0 }]))
    const failures = {}
    let maxMessageLimit = 0
    const unsub = events.map(type => api.event.on(type, event => {
      const p = event.properties ?? {}
      const sid = p.sessionID ?? p.info?.sessionID ?? p.part?.sessionID
      if (sid === api.route.current.params?.sessionID) incoming++
    }))
    const client = {}
    for (const namespace of ["session", "permission", "question"]) {
      client[namespace] = {}
      for (const name of Object.keys(endpoints).filter(name => name.startsWith(namespace + "."))) {
        const method = name.split(".")[1]
        client[namespace][method] = (parameters, options = {}) => {
          endpoints[name]++
          if (name === "session.messages") maxMessageLimit = Math.max(maxMessageLimit, parameters?.limit ?? Infinity)
          if (process.env.VERIJ_RESOURCE_TRANSPORT_METRICS !== "1") return api.client[namespace][method](parameters, options)
          return api.client[namespace][method](parameters, { ...options, requestValidator: async resolved => {
            const fetch = resolved.fetch
            resolved.fetch = async request => {
              const response = await fetch(request)
              if (!response.body) return response
              const reader = response.body.getReader()
              return new Response(new ReadableStream({
                async pull(controller) {
                  const { done, value } = await reader.read()
                  if (done) { controller.close(); reader.releaseLock(); return }
                  transport[name].bytes += value.byteLength; transport[name].reads++
                  controller.enqueue(value)
                },
                async cancel() {
                  transport[name].cancellations++
                  await reader.cancel(); reader.releaseLock()
                },
              }, { highWaterMark: 0 }), { status: response.status, headers: response.headers })
            }
            // Wrap raw transport before the production byte/prefix guards run.
            await options.requestValidator?.(resolved)
          } }).catch(error => {
            failures[name] = error instanceof Error ? error.message.slice(0, 256) : "Observation failed"
            throw error
          })
        }
      }
    }
    if (process.env.VERIJ_RESOURCE_MONITORING === "enabled") {
      const counted = Object.create(api)
      Object.defineProperty(counted, "client", { value: client })
      await bridge.tui(counted, { reporter: process.env.VERIJ_RESOURCE_REPORTER })
    }
    const publish = () => {
      writeFileSync(path + ".tmp", JSON.stringify({ pid: process.pid, events: incoming,
         endpoints, ready: api.state.ready, selected: api.route.current.params?.sessionID,
         version: api.app.version, ...(process.env.VERIJ_RESOURCE_TRANSPORT_METRICS === "1"
           ? { transport, max_message_limit: maxMessageLimit, failures,
             producer_paths: api.state.path, cwd: process.cwd(), runtime: process.versions.node, bun: process.versions.bun } : {}) }))
      renameSync(path + ".tmp", path)
    }
    const interval = setInterval(publish, 1000)
    publish()
    api.lifecycle.onDispose(() => { clearInterval(interval); unsub.forEach(dispose => dispose()); publish() })
    } catch (error) {
      writeFileSync(path, JSON.stringify({ error: String(error).slice(0, 512) }))
    }
  },
}
