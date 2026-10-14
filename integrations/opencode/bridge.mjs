import { spawn as spawnReporter } from "node:child_process"
import { events, normalize, unknown, reconcile, supportsVersion, SnapshotQueue,
  ReconcileScheduler, RequestBudget, budgetedAPI, observationBudget } from "./core.mjs"

export function createBridge(runtime = {}) {
  const clock = runtime.clock ?? {
    now: () => performance.now(), setTimeout: (fn, ms) => globalThis.setTimeout(fn, ms),
    clearTimeout: timer => globalThis.clearTimeout(timer),
  }
  const spawn = runtime.spawn ?? spawnReporter
  const setTimeout = clock.setTimeout, clearTimeout = clock.clearTimeout
  const setInterval = runtime.setInterval ?? globalThis.setInterval
  const clearInterval = runtime.clearInterval ?? globalThis.clearInterval
  return {
  id: "verij.monitoring.v1",
  async tui(api, options = {}) {
    const version = api.app?.version
    // Minor and patch upgrades within OpenCode 1.x remain compatible.
    if (!supportsVersion(version)) {
      api.ui?.toast?.({ title: "Verij monitoring", message: `Unsupported OpenCode ${version}`, variant: "warning" })
      return
    }
    const pane = process.env.ZELLIJ_PANE_ID
    const session = process.env.ZELLIJ_SESSION_NAME
    if (!/^\d+$/.test(pane ?? "") || !session || !options.reporter?.startsWith("/")) return
    const queue = new SnapshotQueue()
    let child, connected = false, stopped = false, writing = false
    let generation = 0, last = unknown(version), lastRoute = "", lastWork = "", retryAfter = 0, reporterFailures = 0
    let startupTimeout, renewalTimeout
    let ownedFamily = new Set()
    let requestController, receiptTimeout
    const requestBudget = new RequestBudget(clock)
    const observedAPI = budgetedAPI(api, requestBudget)
    let scheduler
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
        if (error) recipient.kill("SIGTERM")
      })
      // Next frame waits for native transaction receipt, not merely pipe drain.
    }
    const publish = value => { last = value; queue.push(value); flush() }
    const renewLease = () => {
      clearTimeout(renewalTimeout)
      renewalTimeout = setTimeout(() => {
        if (connected && !stopped && !writing && !queue.items.length) {
          queue.push(last, true); flush() // Receipt schedules the next five-second renewal.
        }
      }, observationBudget.leaseMs)
    }
    const syncRoute = () => {
      const current = api.route.current
      const route = `${current.name}/${current.params?.sessionID ?? ""}`
      if (route === lastRoute) return
      lastRoute = route; lastWork = ""; generation++
      ownedFamily.clear()
      queue.reset() // Old family observations are locally superseded.
      publish(unknown(version))
      retryAfter = 0; reporterFailures = 0
      scheduler?.invalidate({ resetBackoff: true })
    }
    const start = () => {
      if (child || stopped || clock.now() < retryAfter) return
      const candidate = spawn(options.reporter, ["agent", "opencode", "--session", session,
        "--pane", pane, "--pid", String(process.pid)], { env, stdio: ["pipe", "pipe", "ignore"] })
      child = candidate
      let output = ""
      const startup = setTimeout(() => candidate.kill("SIGKILL"), 4000)
      startupTimeout = startup
      candidate.stdin.on("error", () => {})
      candidate.stdout.on("data", bytes => {
        if (child !== candidate || stopped) return
        output += bytes.toString()
        if (output.length > 262144) { candidate.kill("SIGKILL"); return }
        while (output.includes("\n")) {
          const end = output.indexOf("\n")
          let reply
          try { reply = JSON.parse(output.slice(0, end)) } catch { candidate.kill("SIGTERM"); return }
          output = output.slice(end + 1)
          if (!reply || typeof reply !== "object") { candidate.kill("SIGTERM"); return }
          if (reply.ready) {
            if (connected) { candidate.kill("SIGTERM"); return }
            clearTimeout(startup)
            connected = true
            // A new native producer epoch must start conservatively, then sample
            // the current route again before publishing semantic authority.
            generation++; ownedFamily.clear(); queue.reset()
            publish(unknown(version))
            scheduler.invalidate({ resetBackoff: true })
          } else {
            if (!writing || !Number.isSafeInteger(reply.record_revision) || reply.record_revision <= 0) { candidate.kill("SIGTERM"); return }
            clearTimeout(receiptTimeout); writing = false
            queue.acknowledge(); reporterFailures = 0
            renewLease()
          }
          flush()
        }
      })
      const failed = () => {
        clearTimeout(startup)
        clearTimeout(receiptTimeout)
        clearTimeout(renewalTimeout)
        if (child !== candidate) return
        child = null; connected = false; writing = false
        queue.reset()
        if (stopped) return
        reporterFailures = Math.min(reporterFailures + 1, 6)
        retryAfter = clock.now() + Math.min(observationBudget.maxBackoffMs, 2000 * 2 ** (reporterFailures - 1))
        ownedFamily.clear(); last = unknown(version)
        queue.push(unknown(version))
        generation++
        scheduler.invalidate()
      }
      candidate.on("error", failed)
      candidate.on("exit", failed)
    }
    const collect = async () => {
      if (stopped) return false
      syncRoute()
      const sampledGeneration = generation
      requestController = new AbortController()
      const timeout = setTimeout(() => requestController.abort(), 3000)
      try {
        const family = await reconcile(observedAPI, requestController.signal)
        if (sampledGeneration === generation && !stopped) {
          ownedFamily = new Set(family?.members.map(s => s.id) ?? [])
          publish(normalize(version, family))
          return true
        }
        return false
      } catch (error) {
        if (sampledGeneration === generation && !stopped) publish(unknown(version))
        throw error
      } finally {
        clearTimeout(timeout)
        requestController.abort() // Cancel sibling endpoint work after failures.
      }
    }
    scheduler = new ReconcileScheduler(collect, () => last.activity !== "idle" || last.pending_requests.length > 0, clock)
    const unsubscribe = events.map(type => api.event.on(type, event => {
      syncRoute()
      const p = event.properties ?? {}
      const sid = p.sessionID ?? (type.startsWith("session.") ? p.info?.id : p.info?.sessionID) ?? p.part?.sessionID
      if (p.info?.parentID && ownedFamily.has(p.info.parentID) && type.startsWith("session.")) {
        if (ownedFamily.size < 64) ownedFamily.add(p.info.id)
        else if (!ownedFamily.has(p.info.id)) {
          generation++; ownedFamily.clear(); publish(unknown(version)); scheduler.invalidate()
          return
        }
      }
      const selected = api.route.current
      if (!ownedFamily.has(sid) && sid !== selected.params?.sessionID && type !== "server.instance.disposed") return
      generation++ // Never commit a snapshot sampled across a newer owned event.
      const newWork = type === "message.updated" && p.info?.role === "user"
        && typeof p.info.id === "string" && lastWork !== `${sid}/${p.info.id}` && last.turn_id !== `${sid}/${p.info.id}`
      if (newWork) lastWork = `${sid}/${p.info.id}`
      let meaningful = newWork
      if (sid === last.conversation_id && newWork) {
        publish({ ...last, turn_id: `${sid}/${p.info.id}`, activity: "working",
          completed_at_ms: undefined, error_name: undefined })
      }
      if (ownedFamily.has(sid) && type === "session.status" && ["busy", "retry"].includes(p.status?.type)) {
        meaningful ||= last.activity !== (p.status.type === "retry" ? "retrying" : "working")
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
          meaningful = true
          pending.push({ id, kind: type.startsWith("permission") ? "permission" : "question" })
        } else if (!type.endsWith(".asked")) {
          const index = pending.findIndex(r => r.id === id)
          if (index >= 0) { pending.splice(index, 1); meaningful = true }
        }
        if (pending.length > 100) publish(unknown(version))
        else publish({ ...last, pending_requests: pending, completed_at_ms: undefined, error_name: undefined,
          activity: "working" })
      }
      // Repeated stream events only advance generation and set one dirty bit.
      if (newWork) { retryAfter = 0; reporterFailures = 0 }
      scheduler.invalidate({ resetBackoff: meaningful })
    }))
    const interval = setInterval(() => {
      start()
      syncRoute()
      if (queue.overflow) { queue.overflow = false; generation++; scheduler.invalidate() }
    }, 1000)
    queue.push(unknown(version))
    syncRoute()
    start()
    scheduler.invalidate({ resetBackoff: true })
    api.lifecycle.onDispose(async () => {
      stopped = true
      clearTimeout(receiptTimeout)
      clearTimeout(startupTimeout)
      clearTimeout(renewalTimeout)
      clearInterval(interval)
      scheduler.dispose()
      requestController?.abort()
      requestBudget.dispose()
      unsubscribe.forEach(dispose => dispose())
      queue.reset()
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
}

export default createBridge()
