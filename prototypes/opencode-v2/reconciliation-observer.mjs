// Private P0 identity experiment. No SDK/cache access or mutation.
import { createHash } from "node:crypto"
import { terminalMetadata } from "./cli-observer-core.mjs"

export function reconciliationMetadata(value) {
  const metadata = terminalMetadata(value)
  if (value.type === "session.execution.started") return { ...metadata, marker_from_event: null }
  // Tagged SessionMessage.ID.fromEvent, not receipt order or a new identity.
  if (typeof value.id !== "string" || value.id.length > 256 || !/^evt_[A-Za-z0-9]+$/.test(value.id))
    throw new Error("event_identity")
  return { ...metadata, marker_from_event: createHash("sha256")
    .update(value.id.replace(/^evt_/, "msg_")).digest("hex").slice(0, 20) }
}
