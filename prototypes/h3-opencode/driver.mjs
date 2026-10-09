// Test control only: route navigation and plugin enable/disable, never semantics.
import probe from "../opencode-runtime/runtime-tui.mjs"
import { readFileSync, existsSync, unlinkSync, appendFileSync } from "node:fs"
import { createHash } from "node:crypto"

export default {
  id: "verij.h3.driver",
  async tui(api) {
    await probe.tui(api)
    const hash = value => value ? createHash("sha256").update(value).digest("hex").slice(0, 20) : null
    const requests = ["permission.asked", "permission.replied", "question.asked", "question.replied", "question.rejected"]
      .map(type => api.event.on(type, event => appendFileSync(process.env.VJ_RUNTIME_EVENTS, JSON.stringify({
        kind:"h3-request",ts:Date.now(),pid:process.pid,type,session:hash(event.properties.sessionID),
        request:hash(event.properties.id ?? event.properties.requestID),
      }) + "\n")))
    const file = `${process.env.VJ_H3_COMMANDS}/${process.pid}.json`
    let active = false
    const timer = setInterval(async () => {
      if (active || !existsSync(file)) return
      active = true
      try {
        const command = JSON.parse(readFileSync(file, "utf8"))
        unlinkSync(file)
        if (command.action === "disable") await api.plugins.deactivate("verij.monitoring.v1")
        if (command.action === "enable") await api.plugins.activate("verij.monitoring.v1")
        appendFileSync(process.env.VJ_RUNTIME_EVENTS, JSON.stringify({kind:"h3-control",pid:process.pid,
          correlation:command.id,action:command.action}) + "\n")
      } finally { active = false }
    }, 100)
    api.lifecycle.onDispose(() => { clearInterval(timer); requests.forEach(dispose => dispose()) })
  },
}
