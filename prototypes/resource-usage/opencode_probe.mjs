#!/usr/bin/env node
// Synthetic API scheduling only; real sidebar/native-store cost lives in verify.py.
import { parseArgs } from "node:util"
import { writeFileSync, readFileSync } from "node:fs"
import { createHash } from "node:crypto"
import bridge from "../../integrations/opencode/bridge.mjs"
import { ObservationRecovery, RequestBudget, budgetedAPI, observationBudget } from "../../integrations/opencode/core.mjs"
import { fixture } from "./fixture.mjs"

const { values } = parseArgs({ options: {
  seconds: { type: "string", default: "3" }, warmup: { type: "string", default: "1" },
  output: { type: "string" },
} })
const seconds = Number(values.seconds), warmup = Number(values.warmup)
if (!(seconds > 0 && warmup >= 0 && Number.isFinite(seconds + warmup))) throw Error("invalid durations")
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms))
const hashes = Object.fromEntries(["bridge.mjs", "core.mjs"].map(name => [name,
  createHash("sha256").update(readFileSync(new URL(`../../integrations/opencode/${name}`, import.meta.url))).digest("hex")]))
const familyCosts = []
for (const members of [1, 16, 64]) {
  const subject = fixture(members)
  const recovery = new ObservationRecovery(), budget = new RequestBudget(), api = budgetedAPI(subject.api, budget)
  let result, slices = 0, peakSliceCalls = 0
  const start = performance.now()
  try {
    while (!result && slices < 12) {
      const before = subject.count()
      result = await recovery.collect(api, AbortSignal.timeout(3000))
      slices++; peakSliceCalls = Math.max(peakSliceCalls, subject.count() - before)
      if (!result) await sleep(1000)
    }
    if (result?.members.length !== members) throw Error("Family recovery failed")
    const first = subject.count()
    await recovery.collect(api, AbortSignal.timeout(3000))
    familyCosts.push({ members, requests_per_reconciliation: first, slices, peak_slice_calls: peakSliceCalls,
      elapsed_seconds: (performance.now() - start) / 1000,
      cached_recovery_calls: subject.count() - first, endpoints: { ...subject.calls } })
  } finally { budget.dispose() }
}

process.env.ZELLIJ_PANE_ID = "0"
process.env.ZELLIJ_SESSION_NAME = "synthetic-resource-probe"
const measurements = []
for (const monitoring of [false, true]) {
  const subject = fixture()
  subject.activity("busy")
  if (monitoring) await bridge.tui(subject.api, { reporter: "/bin/false" })
  await sleep(warmup * 1000)
  try {
    for (const rate of [0, 20, 100]) {
      const first = { ...subject.calls }, rss = process.memoryUsage().rss
      let events = 0, peakRss = rss
      const start = performance.now()
      const samples = setInterval(() => { peakRss = Math.max(peakRss, process.memoryUsage().rss) }, 100)
      const interval = rate ? setInterval(() => {
        events++
        subject.listeners.get("message.part.updated")?.({ properties: { part: { sessionID: "root" } } })
      }, 1000 / rate) : null
      await sleep(seconds * 1000)
      if (interval) clearInterval(interval)
      clearInterval(samples)
      const elapsed = (performance.now() - start) / 1000
      const endpoints = Object.fromEntries(Object.entries(subject.calls).map(([k, v]) => [k, v - first[k]]))
      const requests = Object.values(endpoints).reduce((a, b) => a + b, 0)
      measurements.push({ monitoring, owned_events_per_second: rate, events, elapsed_seconds: elapsed,
        endpoints, requests, requests_per_second: requests / elapsed,
        rss_mib: process.memoryUsage().rss / 2**20, peak_rss_mib: peakRss / 2**20 })
    }
  } finally { await subject.api.dispose?.() }
}
const report = { status: "PASS", measurement_kind: "synthetic API scheduling; exiting reporter stub; no native writes",
  node_version: process.version, asset_sha256: hashes, warmup_seconds: warmup, workload_seconds: seconds,
  observation_budget: observationBudget,
  familyCosts, measurements }
const text = JSON.stringify(report, null, 2) + "\n"
if (values.output) writeFileSync(values.output, text)
console.log(text)
