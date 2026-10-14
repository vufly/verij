// Real pane-local producer and native reporter; synthetic API responses/events.
import { readFileSync, writeFileSync, renameSync } from "node:fs"
import bridge from "../../integrations/opencode/bridge.mjs"
import { fixture } from "./fixture.mjs"
const [root, cli, monitoring] = process.argv.slice(2)
const subject = fixture()
if (monitoring === "enabled") await bridge.tui(subject.api, { reporter: cli })
let rate = 0, complete = false, timer, events = 0
const publish = () => {
  const path = `${root}/metrics/api.json`
  writeFileSync(path + ".tmp", JSON.stringify({ pid: process.pid, events, endpoints: subject.calls }))
  renameSync(path + ".tmp", path)
}
const control = setInterval(() => {
  let requested
  let done
  try {
    const workload = JSON.parse(readFileSync(`${root}/workload.json`))
    requested = workload.events_per_second
    done = Boolean(workload.complete)
  } catch { return }
  if (requested === rate && done === complete) return
  clearInterval(timer)
  rate = requested
  complete = done
  subject.activity(rate ? "busy" : "idle")
  if (complete) {
    subject.finish()
    subject.listeners.get("session.idle")?.({ properties: { sessionID: "root" } })
  }
  if (rate) timer = setInterval(() => {
    events++
    subject.listeners.get("message.part.updated")?.({ properties: { part: { sessionID: "root" } } })
  }, 1000 / rate)
}, 50)
const summaries = setInterval(publish, 1000)
publish()
console.log("RESOURCE AGENT READY (synthetic API; real foreground process)")
const close = async () => {
  clearInterval(timer); clearInterval(control); clearInterval(summaries)
  await subject.api.dispose?.()
  publish()
  process.exit(0)
}
process.on("SIGTERM", close)
process.on("SIGINT", close)
