import { spawn as spawnReporter } from "node:child_process"
import { events, normalize, unknown, ObservationRecovery, supportsVersion, SnapshotQueue,
  ReconcileScheduler, RequestBudget, budgetedAPI, observationBudget, validObservationID } from "./core.mjs"

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
    const ownedStatuses = new Map(), ownedRequests = new Map()
    let requestController, receiptTimeout
    const requestBudget = new RequestBudget(clock)
    const recovery = new ObservationRecovery(clock)
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
      ownedFamily.clear(); ownedStatuses.clear(); ownedRequests.clear(); recovery.reset()
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
            generation++; ownedFamily.clear(); ownedStatuses.clear(); ownedRequests.clear(); recovery.reset(); queue.reset()
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
        ownedFamily.clear(); ownedStatuses.clear(); ownedRequests.clear(); recovery.reset(); last = unknown(version)
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
        const family = await recovery.collect(observedAPI, requestController.signal)
        // Streaming can supersede semantic samples continuously. Independently
        // verified topology still qualifies later input/activity event projection.
        const current = api.route.current
        if (!stopped && recovery.family && current.name === "session" && current.params?.sessionID === recovery.route) {
          ownedFamily = new Set(recovery.family.members.map(s => s.id))
          if (last.conversation_id !== recovery.family.root.id) {
            publish(normalize(version, { ...recovery.family, statuses: {}, messages: [], messagesComplete: false,
              permissions: [], questions: [] }))
          }
        }
        if (sampledGeneration === generation && !stopped) {
          if (family === undefined) {
            // Retain bounded input/activity edges, but a partial family scan
            // cannot renew old terminal authority as if it had been verified.
            publish({ ...last, activity: "unknown", completed_at_ms: undefined, error_name: undefined })
            return false
          }
          ownedFamily = new Set(family?.members.map(s => s.id) ?? [])
          ownedStatuses.clear(); ownedRequests.clear()
          for (const member of family?.members ?? []) ownedStatuses.set(member.id, family.statuses[member.id]?.type ?? "idle")
          for (const [kind, requests] of [["permission", family?.permissions ?? []], ["question", family?.questions ?? []]]) {
            for (const request of requests) ownedRequests.set(`${kind}/${request.id}`, { ...request, kind })
          }
          const value = normalize(version, family)
          if (["working", "retrying"].includes(value.activity) && value.conversation_id === last.conversation_id) {
            if (family?.messagesComplete === false) value.turn_id = last.turn_id ?? value.turn_id
            else value.turn_id ??= last.turn_id
          }
          publish(value)
          return true
        }
        return false
      } catch (error) {
        if (!recovery.hint) { ownedFamily.clear(); ownedStatuses.clear(); ownedRequests.clear() }
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
      const previousFamily = recovery.hint, topologyEpoch = recovery.epoch
      const topology = ["session.created", "session.updated", "session.deleted"].includes(type)
        && recovery.topology(type, p.info)
      const selected = api.route.current
      const reconnect = ["server.instance.disposed", "server.connected"].includes(type)
      if (!ownedFamily.has(sid) && !recovery.tracks(sid) && sid !== selected.params?.sessionID && !topology && !reconnect) return
      generation++ // Never commit a snapshot sampled across a newer owned event.
      if (reconnect || (topology && recovery.epoch !== topologyEpoch)) {
        // Adding/removing a child invalidates family quiescence, not an unrelated
        // root's explicit input. Only retain previously verified, unaffected IDs.
        const removed = new Set([p.info?.id])
        for (const member of previousFamily?.members ?? []) {
          if (removed.has(member.parentID)) removed.add(member.id)
        }
        if (reconnect || !previousFamily || removed.has(previousFamily.root.id) || removed.has(selected.params?.sessionID)) {
          if (reconnect) recovery.reset()
          ownedFamily.clear(); ownedStatuses.clear(); ownedRequests.clear(); publish(unknown(version))
        } else {
          for (const id of removed) { ownedFamily.delete(id); ownedStatuses.delete(id) }
          for (const [key, request] of ownedRequests) if (removed.has(request.sessionID)) ownedRequests.delete(key)
          publish({ ...last, activity: "unknown", completed_at_ms: undefined, error_name: undefined,
            pending_requests: [...ownedRequests.values()].map(({ id, kind }) => ({ id, kind })) })
        }
        requestController?.abort()
        scheduler.invalidate({ resetBackoff: true })
        return
      }
      const validID = validObservationID
      if ((p.info?.sessionID && p.info.sessionID !== sid) || (p.part?.sessionID && p.part.sessionID !== sid)) {
        recovery.invalidateMessages(); publish(unknown(version)); scheduler.invalidate()
        return
      }
      if (sid === last.conversation_id && ["message.updated", "message.removed", "session.error"].includes(type)) {
        recovery.invalidateMessages()
      }
      const newWork = type === "message.updated" && p.info?.role === "user"
        && validID(p.info.id) && validID(`${sid}/${p.info.id}`)
        && lastWork !== `${sid}/${p.info.id}` && last.turn_id !== `${sid}/${p.info.id}`
      if (newWork) lastWork = `${sid}/${p.info.id}`
      let meaningful = newWork
      if (sid === last.conversation_id && newWork) {
        publish({ ...last, turn_id: `${sid}/${p.info.id}`, activity: "working",
          completed_at_ms: undefined, error_name: undefined })
      }
      if (ownedFamily.has(sid) && type === "session.status" && ["busy", "retry"].includes(p.status?.type)) {
        recovery.invalidateMessages()
        ownedStatuses.set(sid, p.status.type)
        const activity = ownedStatuses.get(last.conversation_id) === "retry" ? "retrying" : "working"
        meaningful ||= last.activity !== activity
        publish({ ...last, completed_at_ms: undefined, error_name: undefined,
          background_task_count: [...ownedStatuses].filter(([id, status]) => id !== last.conversation_id && ["busy", "retry"].includes(status)).length,
          activity })
      }
      if (ownedFamily.has(sid) && (type === "session.error" || type === "message.removed"
          || (type === "session.idle" && last.activity !== "idle")
          || (type === "session.status" && p.status?.type === "idle" && last.activity !== "idle")
          || (type === "message.updated" && p.info?.role === "assistant" && (last.completed_at_ms || last.error_name)))) {
        if (type === "session.idle" || (type === "session.status" && p.status?.type === "idle")) ownedStatuses.set(sid, "idle")
        const busy = [...ownedStatuses].some(([, status]) => ["busy", "retry"].includes(status))
        publish({ ...last, completed_at_ms: undefined, error_name: undefined,
          background_task_count: [...ownedStatuses].filter(([id, status]) => id !== last.conversation_id && ["busy", "retry"].includes(status)).length,
          activity: busy ? (ownedStatuses.get(last.conversation_id) === "retry" ? "retrying" : "working") : "unknown" })
      }
      // Preserve input edges immediately for root and verified descendants.
      // New descendants await topology validation. No event broadcasts
      // are allowed to assign another TUI's conversation to this pane.
      if (ownedFamily.has(sid) && (type.startsWith("permission.") || type.startsWith("question."))) {
        const id = p.id ?? p.requestID
        if (!validID(id)) { scheduler.invalidate(); return }
        const kind = type.startsWith("permission") ? "permission" : "question", key = `${kind}/${id}`
        if (type.endsWith(".asked") && !ownedRequests.has(key)) {
          if (ownedRequests.size >= 100) {
            ownedRequests.clear(); publish(unknown(version)); scheduler.invalidate()
            return
          }
          meaningful = true
          ownedRequests.set(key, { id, kind, sessionID: sid })
        } else if (!type.endsWith(".asked") && ownedRequests.get(key)?.sessionID === sid) {
          ownedRequests.delete(key); meaningful = true
        }
        if (meaningful) publish({ ...last, pending_requests: [...ownedRequests.values()].map(({ id, kind }) => ({ id, kind })),
          completed_at_ms: undefined, error_name: undefined, activity: "working" })
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
      recovery.reset()
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
