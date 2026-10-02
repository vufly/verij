// Probe actual local TUI state/events; persist metadata only, never text/tool payloads.
import { appendFileSync, readFileSync, existsSync, unlinkSync } from "node:fs"
import { createHash } from "node:crypto"

const key = value => value ? createHash("sha256").update(value).digest("hex").slice(0, 20) : null
const identity = () => {
  const stat = readFileSync(`/proc/${process.pid}/stat`, "utf8")
  const fields = stat.slice(stat.lastIndexOf(")") + 2).split(/\s+/)
  return { pid: process.pid, pgrp: Number(fields[2]), tty_nr: Number(fields[4]),
    tpgid: Number(fields[5]), start_jiffies: Number(fields[19]),
    boot_id: readFileSync("/proc/sys/kernel/random/boot_id", "utf8").trim() }
}
const message = info => ({ id: key(info.id), session: key(info.sessionID), role: info.role,
  parent_message: key(info.parentID), finish: info.finish ?? null,
  completed: Boolean(info.time?.completed), error_name: info.error?.name ?? null })

export default {
  id: "verij-runtime-metadata",
  async tui(api) {
    const pid = process.pid
    const pane = process.env.ZELLIJ_PANE_ID
    const parents = new Map()
    const rec = record => appendFileSync(process.env.VJ_RUNTIME_EVENTS,
      JSON.stringify({ ts: Date.now(), pid, pane, ...record }) + "\n", { mode: 0o600 })
    const active = () => api.route.current.name === "session" ? api.route.current.params.sessionID : null
    const inFamily = sid => {
      const selected = active()
      if (!selected || !sid) return false
      const seen = new Set()
      while (sid && !seen.has(sid)) {
        if (sid === selected) return true
        seen.add(sid)
        sid = parents.get(sid) ?? api.state.session.get(sid)?.parentID
      }
      return false
    }
    const subscriptions = ["session.created", "session.updated", "session.status", "session.idle",
      "session.error", "message.updated", "message.part.updated", "permission.asked", "question.asked"]
    const unsub = subscriptions.map(type => api.event.on(type, event => {
      const properties = event.properties ?? {}
      const info = properties.info
      if (type.startsWith("session.") && info?.id) parents.set(info.id, info.parentID ?? null)
      const sid = properties.sessionID ?? info?.sessionID ?? info?.id ?? properties.part?.sessionID
      const item = { kind: "event", type, event: key(event.id), session: key(sid),
        route: key(active()), in_family: inFamily(sid), parent_session: key(type.startsWith("session.") ? info?.parentID : null),
        status: properties.status?.type ?? null, retry_attempt: properties.status?.attempt ?? null,
        error_name: properties.error?.name ?? null }
      if (type === "message.updated") item.message = message(info)
      if (type === "message.part.updated") item.part = {
        type: properties.part.type, tool: properties.part.tool ?? null,
        tool_status: properties.part.state?.status ?? null, reason: properties.part.reason ?? null }
      rec(item)
    }))
    rec({ kind: "init", birth: identity(), route: key(active()) })
    let ready = false
    const commands = `${process.env.VJ_RUNTIME_COMMANDS}/${pid}.json`
    const interval = setInterval(() => {
      if (!ready && api.state.ready) {
        ready = true
        rec({ kind: "ready", birth: identity(), route: key(active()) })
      }
      if (!ready || !existsSync(commands)) return
      try {
        const command = JSON.parse(readFileSync(commands, "utf8"))
        unlinkSync(commands)
        if (command.action === "navigate") api.route.navigate(command.route, command.params ?? {})
        const snapshots = (command.sessions ?? []).map(sid => ({
          session: key(sid), parent_session: key(api.state.session.get(sid)?.parentID ?? parents.get(sid)),
          in_family: inFamily(sid), status: api.state.session.status(sid)?.type ?? null,
          messages: api.state.session.messages(sid).map(message),
        }))
        rec({ kind: "snapshot", correlation: command.id, route: key(active()), sessions: snapshots })
      } catch (error) {
        rec({ kind: "probe_error", error_type: error.name })
      }
    }, 100)
    api.lifecycle.onDispose(() => {
      clearInterval(interval)
      unsub.forEach(dispose => dispose())
      rec({ kind: "dispose" })
    })
  },
}
