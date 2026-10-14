import assert from "node:assert/strict"
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import test from "node:test"
import { contextMetadata, eventMetadata, opaque, project, recorder } from "./trace.mjs"

const runtime = opaque("test-runtime")

test("opaque IDs never retain source identity", () => {
  assert.match(opaque("PRIVATE_SESSION_SENTINEL"), /^[a-f0-9]{20}$/)
  assert.equal(opaque("PRIVATE_SESSION_SENTINEL"), opaque("PRIVATE_SESSION_SENTINEL"))
  assert.equal(opaque(""), undefined)
  assert.equal(opaque({ id: "PRIVATE" }), undefined)
})

test("event projection drops all raw content and arbitrary field names", () => {
  const event = {
    type: "turn_end", outcome: "completed", turnIndex: 0,
    toolCallId: "PRIVATE_TOOL_ID_SENTINEL", parentToolCallId: "PRIVATE_PARENT_SENTINEL",
    messageEntryId: "PRIVATE_ENTRY_SENTINEL", errorMessage: "PRIVATE_PROVIDER_BODY_SENTINEL",
    context: { canContinue: true, llmMessages: ["PRIVATE_CONTEXT_SENTINEL"] },
    message: { role: "assistant", stopReason: "stop", content: [{ type: "text", text: "PRIVATE_ASSISTANT_SENTINEL" },
      { type: "toolCall", arguments: { secret: "PRIVATE_ARGUMENT_SENTINEL" } }] },
    result: "PRIVATE_TOOL_RESULT_SENTINEL", title: "PRIVATE_DIALOG_SENTINEL",
  }
  const record = project("turn_end", "observer", runtime, {
    ...eventMetadata(event), prompt: "PRIVATE_PROMPT_SENTINEL", arbitrary: "PRIVATE", title: event.title,
  })
  assert.equal(record.outcome, "completed")
  assert.equal(record.tool_count, 1)
  assert.equal(record.error_present, true)
  assert.equal(record.can_continue, true)
  assert.match(record.call, /^[a-f0-9]{20}$/)
  assert.doesNotMatch(JSON.stringify(record), /PRIVATE|SENTINEL|secret|arguments|prompt|title/)
})

test("context metadata never enumerates or reads transcript entries", () => {
  const ctx = { mode: "tui", signal: { aborted: false }, isIdle: () => false, hasPendingMessages: () => true,
    sessionManager: { getSessionId: () => "PRIVATE_SESSION_SENTINEL", getSessionName: () => "PRIVATE_NAME_SENTINEL",
      getBranch: () => { throw Error("must not inspect transcript") }, getEntries: () => { throw Error("must not inspect transcript") } } }
  const record = project("agent_start", "observer", runtime, contextMetadata(ctx))
  assert.equal(record.pending, true)
  assert.equal(record.name_present, true)
  assert.doesNotMatch(JSON.stringify(record), /PRIVATE|SENTINEL/)
})

test("dialog titles and arbitrary error text are never forwarded", () => {
  const record = project("ui_prompt_start", "observer", runtime,
    eventMetadata({ kind: "confirm", title: "PRIVATE_SECRET_SENTINEL", errorMessage: "PRIVATE_ERROR_SENTINEL" }))
  assert.equal(record.kind, "confirm")
  assert.equal(record.error_present, true)
  assert.equal("title" in record, false)
})

test("invalid enums and unbounded/invalid numeric or hash values are dropped", () => {
  const record = project("agent_end", "observer", runtime, { outcome: "PRIVATE", case: "PRIVATE", call: "raw-id",
    pid: -1, tool_count: Infinity, turn_index: 1.5, version: "2.0.0", engine_version: "PRIVATE_ENGINE" })
  assert.deepEqual(record, { schema_version: 1, source: "observer", runtime, event: "agent_end" })
})

test("bad event, source, and runtime identities are rejected", () => {
  assert.throws(() => project("message_update", "observer", runtime))
  assert.throws(() => project("agent_start", "unknown", runtime))
  assert.throws(() => project("agent_start", "observer", "raw-runtime"))
})

test("recorder failure stays non-gating and does not write to stdout", () => {
  const record = recorder("/not/an/existing/p0-parent/trace", "observer", runtime, "1.2.0")
  assert.doesNotThrow(() => record("agent_start"))
  assert.doesNotThrow(() => record("agent_end"))
})

test("recorder enforces one-megabyte cap and permissions", () => {
  const root = mkdtempSync(join(tmpdir(), "vj-pi-trace-test-"))
  try {
    const path = join(root, "trace")
    const record = recorder(path, "observer", runtime, "1.2.0")
    record("agent_start", { prompt: "PRIVATE_SECRET_SENTINEL" })
    assert.equal(statSync(path).mode & 0o777, 0o600)
    assert.doesNotMatch(readFileSync(path, "utf8"), /PRIVATE|prompt/)
    writeFileSync(path, "x".repeat(1024 * 1024))
    record("agent_end")
    assert.equal(readFileSync(path).length, 1024 * 1024)
    writeFileSync(path, "")
    record("agent_end") // Once disabled, the writer does not re-enable itself.
    assert.equal(readFileSync(path).length, 0)
  } finally { rmSync(root, { recursive: true, force: true }) }
})
