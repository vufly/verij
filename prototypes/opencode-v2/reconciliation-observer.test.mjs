import test from "node:test"
import assert from "node:assert/strict"
import { createHash } from "node:crypto"
import { reconciliationMetadata } from "./reconciliation-observer.mjs"

const hash = value => createHash("sha256").update(value).digest("hex").slice(0, 20)
const event = (type = "failed") => ({ type: `session.execution.${type}`, id: "evt_0123ABC",
  data: { error: { type: "provider.invalid-request" }, reason: "user" } })

test("terminal event derives exact tagged message ID, never event hash", () => {
  for (const type of ["succeeded", "failed", "interrupted"]) {
    const result = reconciliationMetadata(event(type))
    assert.equal(result.marker_from_event, hash("msg_0123ABC"))
    assert.notEqual(result.marker_from_event, hash("evt_0123ABC"))
  }
})

test("started activity has no terminal marker authority", () => {
  const value = event("started")
  Object.defineProperty(value, "id", { get() { throw new Error("must not read") } })
  assert.deepEqual(reconciliationMetadata(value), { error_type: null, interruption_reason: null, marker_from_event: null })
})

test("different terminals and repeated observation have stable distinct identity", () => {
  const first = event()
  const second = { ...first, id: "evt_0123ABD" }
  assert.deepEqual(reconciliationMetadata(first), reconciliationMetadata(first))
  assert.notEqual(reconciliationMetadata(first).marker_from_event, reconciliationMetadata(second).marker_from_event)
})

test("malformed, raw message, and oversized event identities fail closed", () => {
  for (const id of [null, 5, {}, "", "evt_", "msg_0123ABC", "evt_private/body", "evt_" + "a".repeat(253)]) {
    assert.throws(() => reconciliationMetadata({ ...event(), id }), /event_identity/)
  }
})

test("body, error text, and message metadata are never accessed or retained", () => {
  const value = event()
  for (const field of ["message", "response", "body", "metadata"]) {
    Object.defineProperty(value.data.error, field, { get() { throw new Error("PRIVATE_SENTINEL") } })
    Object.defineProperty(value.data, field, { get() { throw new Error("PRIVATE_SENTINEL") } })
  }
  assert.deepEqual(reconciliationMetadata(value), { error_type: "provider.invalid-request", interruption_reason: null,
    marker_from_event: hash("msg_0123ABC") })
})

test("unknown error types collapse without leaking text", () => {
  const value = event()
  value.data.error.type = "PRIVATE_SENTINEL"
  const result = reconciliationMetadata(value)
  assert.equal(result.error_type, "other")
  assert.equal(JSON.stringify(result).includes("PRIVATE_SENTINEL"), false)
})

test("interruption reasons stay distinct and unknown reason fails", () => {
  for (const reason of ["user", "shutdown", "superseded", "inactivity"]) {
    const value = event("interrupted")
    value.data.reason = reason
    assert.equal(reconciliationMetadata(value).interruption_reason, reason)
  }
  const value = event("interrupted")
  value.data.reason = "PRIVATE_SENTINEL"
  assert.throws(() => reconciliationMetadata(value), /reason_schema/)
})
