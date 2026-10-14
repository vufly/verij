// Count bridge API invocations and owned runtime events without retaining payloads.
import { writeFileSync, renameSync } from "node:fs"
import bridge from "../../integrations/opencode/bridge.mjs"
import { events } from "../../integrations/opencode/core.mjs"
export default {
  id: "verij-resource-baseline",
  async tui(api) {
    try {
    const endpoints = Object.fromEntries(["session.status", "session.get", "session.children",
      "session.messages", "permission.list", "question.list"].map(name => [name, 0]))
    let incoming = 0
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
        client[namespace][method] = (...args) => {
          endpoints[name]++
          return api.client[namespace][method](...args)
        }
      }
    }
    if (process.env.VERIJ_RESOURCE_MONITORING === "enabled") {
      const counted = Object.create(api)
      Object.defineProperty(counted, "client", { value: client })
      await bridge.tui(counted, { reporter: process.env.VERIJ_RESOURCE_REPORTER })
    }
    const publish = () => {
      const path = `${process.env.VERIJ_RESOURCE_ROOT}/metrics/api.json`
      writeFileSync(path + ".tmp", JSON.stringify({ pid: process.pid, events: incoming,
        endpoints, ready: api.state.ready, selected: api.route.current.params?.sessionID,
        version: api.app.version }))
      renameSync(path + ".tmp", path)
    }
    const interval = setInterval(publish, 1000)
    publish()
    api.lifecycle.onDispose(() => { clearInterval(interval); unsub.forEach(dispose => dispose()); publish() })
    } catch (error) {
      writeFileSync(`${process.env.VERIJ_RESOURCE_ROOT}/metrics/api.json`, JSON.stringify({ error: String(error).slice(0, 512) }))
    }
  },
}
