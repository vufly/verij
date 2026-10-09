import { spawn } from "node:child_process"
import { events, normalize, unknown, reconcile, versions, SnapshotQueue } from "./core.mjs"

export default {
  id: "verij.monitoring.v1",
  async tui(api, options = {}) {
    const version = api.app?.version
    // File plugins have no engines guard. Select only explicitly verified APIs.
    if (!versions.includes(version)) {
      api.ui?.toast?.({ title: "Verij monitoring", message: `Unsupported OpenCode ${version}`, variant: "warning" })
      return
    }
    const pane = process.env.ZELLIJ_PANE_ID
    const session = process.env.ZELLIJ_SESSION_NAME
    if (!/^\d+$/.test(pane ?? "") || !session || !options.reporter?.startsWith("/")) return
    const queue = new SnapshotQueue()
    let child, connected = false, stopped = false, collecting = false, writing = false
    let generation = 0, last = unknown(version), lastRoute = "", retryAfter = 0
    let ownedFamily = new Set()
    let requestController, receiptTimeout
    const env = { ...process.env }
    // Explicit launch-time overrides win. Installed defaults survive profile HOME.
    env.VERIJ_AGENT_STATE_DIR ??= options.runtimeDir
    env.VERIJ_STATES_DIR ??= options.statesDir
    const flush = () => {
      if (!connected || writing || stopped) return
      const item = queue.shift()
      if (!item) return
      writing = true
      const recipient = child
      receiptTimeout = setTimeout(() => recipient.kill("SIGKILL"), 4000)
      child.stdin.write(item + "\n", error => {
        if (error) child?.kill("SIGTERM")
      })
      // Next frame waits for native transaction receipt, not merely pipe drain.
    }
    const publish = value => { last = value; queue.push(value); flush() }
    const syncRoute = () => {
      const current = api.route.current
      const route = `${current.name}/${current.params?.sessionID ?? ""}`
      if (route === lastRoute) return
      lastRoute = route; generation++
      ownedFamily.clear()
      queue.items.length = 0 // Old family observations are locally superseded.
      publish(unknown(version))
    }
    const start = () => {
      if (child || stopped || Date.now() < retryAfter) return
      const candidate = spawn(options.reporter, ["agent", "opencode", "--session", session,
        "--pane", pane, "--pid", String(process.pid)], { env, stdio: ["pipe", "pipe", "ignore"] })
      child = candidate
      let output = ""
      const startup = setTimeout(() => candidate.kill("SIGKILL"), 4000)
      candidate.stdin.on("error", () => {})
      candidate.stdout.on("data", bytes => {
        output += bytes.toString()
        if (output.length > 262144) { candidate.kill("SIGKILL"); return }
        while (output.includes("\n")) {
          const end = output.indexOf("\n")
          let reply
          try { reply = JSON.parse(output.slice(0, end)) } catch { candidate.kill("SIGTERM"); return }
          output = output.slice(end + 1)
          if (reply.ready) {
            clearTimeout(startup)
            connected = true
          } else { clearTimeout(receiptTimeout); writing = false }
          flush()
        }
      })
      const failed = () => {
        clearTimeout(startup)
        clearTimeout(receiptTimeout)
        if (child !== candidate) return
        child = null; connected = false; writing = false
        retryAfter = Date.now() + 2000
        queue.items.length = 0
        queue.push(unknown(version))
        generation++
      }
      candidate.on("error", failed)
      candidate.on("exit", failed)
    }
    const collect = async () => {
      if (collecting || stopped) return
      syncRoute()
      collecting = true
      const sampledGeneration = generation
      requestController = new AbortController()
      const timeout = setTimeout(() => requestController.abort(), 3000)
      try {
        const family = await reconcile(api, requestController.signal)
        if (sampledGeneration === generation && !stopped) {
          ownedFamily = new Set(family?.members.map(s => s.id) ?? [])
          publish(normalize(version, family))
        }
      } catch {
        if (sampledGeneration === generation && !stopped) publish(unknown(version))
      } finally {
        clearTimeout(timeout)
        collecting = false
      }
    }
    const unsubscribe = events.map(type => api.event.on(type, event => {
      syncRoute()
      const p = event.properties ?? {}
      const sid = p.sessionID ?? (type.startsWith("session.") ? p.info?.id : p.info?.sessionID) ?? p.part?.sessionID
      if (p.info?.parentID && ownedFamily.has(p.info.parentID) && type.startsWith("session.")) {
        ownedFamily.add(p.info.id)
      }
      const selected = api.route.current
      if (!ownedFamily.has(sid) && sid !== selected.params?.sessionID && type !== "server.instance.disposed") return
      generation++ // Never commit a snapshot sampled across a newer owned event.
      if (sid === last.conversation_id && type === "message.updated" && p.info?.role === "user") {
        publish({ ...last, turn_id: `${sid}/${p.info.id}`, activity: "working",
          pending_requests: [], completed_at_ms: undefined, error_name: undefined })
      }
      if (ownedFamily.has(sid) && type === "session.status" && ["busy", "retry"].includes(p.status?.type)) {
        publish({ ...last, completed_at_ms: undefined, error_name: undefined,
          activity: p.status.type === "retry" ? "retrying" : "working" })
      }
      // Preserve input edges immediately for the owned root; descendant input
      // is reconciled through its validated parent chain. No event broadcasts
      // are allowed to assign another TUI's conversation to this pane.
      if (ownedFamily.has(sid) && (type.startsWith("permission.") || type.startsWith("question."))) {
        const pending = [...last.pending_requests]
        const id = p.id ?? p.requestID
        if (type.endsWith(".asked") && id && !pending.some(r => r.id === id)) {
          pending.push({ id, kind: type.startsWith("permission") ? "permission" : "question" })
        } else if (!type.endsWith(".asked")) {
          const index = pending.findIndex(r => r.id === id)
          if (index >= 0) pending.splice(index, 1)
        }
        publish({ ...last, pending_requests: pending, completed_at_ms: undefined, error_name: undefined,
          activity: "working" })
      }
      // Let the native TUI cache subscription run first; reconciliation is
      // authoritative even when callbacks precede hydration.
      queueMicrotask(collect)
    }))
    const interval = setInterval(() => {
      start()
      syncRoute()
      if (queue.overflow) { queue.overflow = false; generation++ }
      void collect()
    }, 1000)
    queue.push(unknown(version))
    start()
    void collect()
    api.lifecycle.onDispose(async () => {
      stopped = true
      clearTimeout(receiptTimeout)
      clearInterval(interval)
      requestController?.abort()
      unsubscribe.forEach(dispose => dispose())
      queue.items.length = 0
      const closing = child
      if (!closing) return
      closing.stdin.end()
      await new Promise(resolve => {
        const timeout = setTimeout(() => { closing.kill("SIGKILL"); resolve() }, 1500)
        closing.once("exit", () => { clearTimeout(timeout); resolve() })
      })
    })
  },
}
