// P0 observation only. No SDK reads, navigation, replies, sync, or reporter.
import { Plugin } from "@opencode/plugin/tui"
import { appendFileSync } from "node:fs"
import { resolve } from "node:path"
import { createHash } from "node:crypto"

export default Plugin.define({
  id: "verij.contract.v2",
  setup(context) {
    const root = process.env.VJ_V2_PROBE_ROOT
    if (!root || resolve(root, "work") !== process.cwd()) throw new Error("Private fixture required")
    const output = resolve(root, "probe.ndjson")
    let frames = 0
    let lastRoute
    let disposed = false
    const emit = (kind, fields = {}) => {
      if (frames >= 64) return
      appendFileSync(output, JSON.stringify({ kind, index: frames++, pid: process.pid, ...fields }) + "\n", { mode: 0o600 })
    }
    const shape = {
      session_get: typeof context.client.session?.get === "function",
      session_active: typeof context.client.session?.active === "function",
      session_list: typeof context.client.session?.list === "function",
      message_list: typeof context.client.message?.list === "function",
      permission_requests: typeof context.client.permission?.request?.list === "function",
      session_forms: typeof context.client.session?.form?.list === "function",
      session_inbox: typeof context.client.session?.inbox?.list === "function",
      typed_events: typeof context.data.on === "function",
      enveloped_events: typeof context.data.listen === "function",
      family_cache: typeof context.data.session?.family === "function",
    }
    emit("setup", { version_matches: context.app.version === "2.0.32",
      options_match: context.options.fixture === "verij-v2-p0", shape })
    const observeRoute = () => {
      if (disposed) return
      const current = context.ui.router.current()
      const route = ["home", "session", "plugin"].includes(current?.type) ? current.type : "unknown"
      const selected = route === "session" && typeof current.sessionID === "string" && current.sessionID.length <= 256
        ? createHash("sha256").update(current.sessionID).digest("hex").slice(0, 20) : null
      const key = `${route}/${selected}`
      if (lastRoute === key) return
      lastRoute = key
      emit("route", { route, selected })
    }
    // Exercise subscription/cleanup shapes without retaining raw event envelopes.
    const unsubscribeTyped = context.data.on("session.execution.started", () => observeRoute())
    const unsubscribeEnvelope = context.data.listen(() => observeRoute())
    observeRoute()
    const timer = setInterval(observeRoute, 250)
    return () => {
      if (disposed) return
      disposed = true
      clearInterval(timer)
      unsubscribeTyped()
      unsubscribeEnvelope()
      emit("cleanup")
    }
  },
})
