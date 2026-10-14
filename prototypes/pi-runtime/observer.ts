/** Passive P0 observer. No transcript mutations, continuation, UI, or timers. */
import { randomUUID } from "node:crypto"
import { VERSION, type ExtensionAPI, type ExtensionContext } from "@earendil-works/pi-coding-agent"
import { EVENTS, contextMetadata, eventMetadata, opaque, recorder } from "./trace.mjs"

export default function (pi: ExtensionAPI) {
  const record = recorder(process.env.VJ_PI_TRACE, "observer", opaque(randomUUID()), VERSION)
  record("observer_loaded")
  // Dynamic registration only for this probe; event projection remains an explicit whitelist.
  const on = pi.on.bind(pi) as (type: string, handler: (event: any, ctx: ExtensionContext) => void) => void
  for (const type of EVENTS) on(type, (event, ctx) => {
    try { record(type, { ...contextMetadata(ctx), ...eventMetadata(event) }) } catch { /* Invalidated context is not an agent failure. */ }
  })
}
