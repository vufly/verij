import { spawn as nativeSpawn } from "node:child_process"

// Live snapshots have no durable outcomes: replacing the pending full snapshot
// is safe. One in-flight receipt + one pending value bounds event-storm work.
export class LiveReporter {
  constructor({ executable, args, env, unknown, reset, spawn = nativeSpawn, clock = globalThis }) {
    Object.assign(this, { executable, args, env, unknown, reset, spawn, clock })
    this.child = undefined; this.closed = false; this.ready = false
    this.pending = undefined; this.inFlight = undefined; this.delivered = undefined
    this.last = unknown; this.failures = 0; this.retryAt = 0; this.renewAt = Infinity
  }
  publish(snapshot) {
    if (this.closed) return
    const value = JSON.stringify(snapshot)
    if (Buffer.byteLength(value) > 65536) return this.publish(this.unknown)
    this.last = snapshot
    if (value === (this.pending ?? this.inFlight ?? this.delivered)) return
    this.pending = value
    this.flush()
  }
  flush() {
    if (!this.ready || !this.child || this.inFlight || !this.pending || this.closed) return
    const child = this.child
    this.inFlight = this.pending; this.pending = undefined
    this.receiptTimer = this.clock.setTimeout(() => child.kill("SIGKILL"), 4000)
    try { child.stdin.write(this.inFlight + "\n", error => { if (error) child.kill("SIGKILL") }) }
    catch { child.kill("SIGKILL") }
  }
  tick(now) {
    if (this.closed) return
    if (!this.child && now >= this.retryAt) this.start(now)
    if (this.ready && !this.inFlight && !this.pending && now >= this.renewAt) {
      this.pending = JSON.stringify(this.last); this.flush()
    }
  }
  start(now) {
    let child
    try { child = this.spawn(this.executable, this.args, { env: this.env, stdio: ["pipe", "pipe", "ignore"] }) }
    catch { this.failed(now); return }
    this.child = child
    let output = ""
    this.startTimer = this.clock.setTimeout(() => child.kill("SIGKILL"), 4000)
    child.stdin.on("error", () => {})
    child.stdout.on("data", bytes => {
      if (this.closed || child !== this.child) return
      if (bytes.length > 16384 || output.length + bytes.length > 16384) { child.kill("SIGKILL"); return }
      output += bytes.toString()
      while (output.includes("\n")) {
        const end = output.indexOf("\n")
        let reply
        try { reply = JSON.parse(output.slice(0, end)) } catch { child.kill("SIGKILL"); return }
        output = output.slice(end + 1)
        if (!reply || typeof reply !== "object") { child.kill("SIGKILL"); return }
        if (reply.ready === true && !this.ready) {
          this.clock.clearTimeout(this.startTimer)
          this.ready = true
          this.pending = undefined; this.inFlight = undefined; this.delivered = undefined
          this.reset()
          this.publish(this.unknown) // Every native producer starts without semantic authority.
        } else if (this.ready && this.inFlight && Number.isSafeInteger(reply.record_revision) && reply.record_revision > 0) {
          this.clock.clearTimeout(this.receiptTimer)
          this.delivered = this.inFlight; this.inFlight = undefined
          this.failures = 0; this.renewAt = this.now() + 5000
          this.flush()
        } else { child.kill("SIGKILL"); return }
      }
    })
    const failed = () => {
      if (child !== this.child) return
      this.child = undefined
      this.failed(this.now())
    }
    child.on("error", failed)
    child.on("exit", failed)
  }
  now() { return this.clock.now ? this.clock.now() : performance.now() }
  failed(now) {
    this.clock.clearTimeout(this.startTimer); this.clock.clearTimeout(this.receiptTimer)
    this.ready = false; this.pending = undefined; this.inFlight = undefined; this.delivered = undefined
    this.last = this.unknown; this.reset()
    this.failures = Math.min(this.failures + 1, 5)
    this.retryAt = now + Math.min(30000, 2000 * 2 ** (this.failures - 1))
  }
  async dispose() {
    if (this.closed) return
    this.closed = true
    this.clock.clearTimeout(this.startTimer); this.clock.clearTimeout(this.receiptTimer)
    this.pending = undefined; this.inFlight = undefined
    const child = this.child
    if (!child) return
    await new Promise(resolve => {
      const timer = this.clock.setTimeout(() => { child.kill("SIGKILL"); resolve() }, 1500)
      child.once("exit", () => { this.clock.clearTimeout(timer); resolve() })
      try { child.stdin.end() } catch { child.kill("SIGKILL") }
    })
  }
}
