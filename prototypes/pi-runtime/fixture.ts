/** Active TEST fixture, not a monitoring adapter. Runs only in a private harness. */
import { randomUUID } from "node:crypto"
import { readFileSync, writeFileSync } from "node:fs"
import { VERSION, type ExtensionAPI, type ExtensionContext } from "@earendil-works/pi-coding-agent"
import { Type } from "typebox"
import { CASES, contextMetadata, opaque, recorder } from "./trace.mjs"

const sleep = (ms: number) => new Promise(resolve => setTimeout(resolve, ms))
const result = () => ({ content: [{ type: "text" as const, text: "PRIVATE_TOOL_RESULT_SENTINEL" }], details: undefined })

export default function (pi: ExtensionAPI) {
  const record = recorder(process.env.VJ_PI_TRACE, "fixture", opaque(randomUUID()), VERSION)
  const mark = (type: string, ctx?: ExtensionContext, values = {}) => record(type, { ...contextMetadata(ctx), ...values })
  let scenario = "success", continued = false, queued = false
  record("fixture_loaded")
  pi.on("session_start", (event, ctx) => mark("fixture_ready", ctx, { reason: event.reason }))

  pi.registerTool({ name: "probe_leaf", label: "P0 leaf", description: "Private deterministic test tool",
    parameters: Type.Object({ fail: Type.Optional(Type.Boolean()), delayMs: Type.Optional(Type.Number()) }),
    executionMode: "parallel",
    async execute(_id, args) {
      await sleep(Math.min(Math.max(args.delayMs ?? 0, 0), 100))
      if (args.fail) throw new Error("PRIVATE_TOOL_ERROR_SENTINEL")
      return result()
    },
  })
  pi.registerTool({ name: "probe_nested", label: "P0 nested", description: "Private nested parallel tool fixture",
    parameters: Type.Object({}), executionMode: "parallel",
    async execute(_id, _args, _signal, _update, ctx) {
      await Promise.all([ctx.executeTool("probe_leaf", { delayMs: 60 }), ctx.executeTool("probe_leaf", { delayMs: 30 })])
      return result()
    },
  })
  pi.registerTool({ name: "probe_queue", label: "P0 queues", description: "Private steering and follow-up fixture",
    parameters: Type.Object({}),
    async execute(_id, _args, _signal, _update, ctx) {
      pi.sendUserMessage("PRIVATE_STEERING_SENTINEL", { deliverAs: "steer" })
      pi.sendUserMessage("PRIVATE_FOLLOWUP_SENTINEL", { deliverAs: "followUp" })
      mark("queue_submitted", ctx, { case: scenario })
      return result()
    },
  })
  pi.registerTool({ name: "probe_ui", label: "P0 dialogs", description: "Private blocking dialog fixture",
    parameters: Type.Object({}),
    async execute(_id, _args, _signal, _update, ctx) {
      // Real TUI dialogs, dismissed by the harness's PTY Escape key.
      await ctx.ui.select("PRIVATE_DIALOG_TITLE_SENTINEL", ["PRIVATE_OPTION_SENTINEL"])
      await ctx.ui.confirm("PRIVATE_DIALOG_TITLE_SENTINEL", "PRIVATE_DIALOG_BODY_SENTINEL")
      await ctx.ui.input("PRIVATE_DIALOG_TITLE_SENTINEL", "PRIVATE_PLACEHOLDER_SENTINEL")
      await ctx.ui.editor("PRIVATE_DIALOG_TITLE_SENTINEL", "PRIVATE_EDITOR_SENTINEL")
      await ctx.ui.custom((_tui, _theme, _keys, done) => ({
        render: (width: number) => ["PRIVATE_CUSTOM_SENTINEL".slice(0, width)],
        handleInput: () => done(undefined), invalidate: () => {},
      }))
      return result()
    },
  })
  pi.registerTool({ name: "probe_ui_nested", label: "P0 nested dialog", description: "Private nested UI fixture",
    parameters: Type.Object({}),
    async execute(_id, _args, _signal, _update, ctx) {
      await ctx.ui.custom((_tui, _theme, _keys, done) => {
        // Open a child prompt after the outer custom component is installed.
        const controller = new AbortController()
        const start = setTimeout(() => {
          mark("nested_child_start", ctx)
          void ctx.ui.confirm("PRIVATE_CHILD_SENTINEL", "PRIVATE_CHILD_BODY_SENTINEL", { signal: controller.signal })
            .then(() => { mark("nested_child_end", ctx); setTimeout(() => done(undefined), 30) })
        }, 40)
        const dismiss = setTimeout(() => controller.abort(), 140)
        return { render: (width: number) => ["PRIVATE_OUTER_SENTINEL".slice(0, width)],
          invalidate: () => {}, dispose: () => { clearTimeout(start); clearTimeout(dismiss) } }
      })
      return result()
    },
  })

  pi.on("agent_end", async (_event, ctx) => {
    if (scenario === "postrun_error" || (scenario === "continuation_error" && continued)) {
      // Throw from Pi post-run compaction settings, after a successful turn.
      const model = ctx.modelRegistry.find("p0-fixture", "bad_compaction")
      if (!model || !await pi.setModel(model)) throw new Error("private invalid-config model unavailable")
      mark("postrun_failure_injected", ctx)
    }
    if (scenario === "late_queued" && !queued) {
      queued = true
      pi.sendUserMessage("PRIVATE_LATE_FOLLOWUP_SENTINEL", { deliverAs: "followUp" })
      mark("queue_submitted", ctx, { case: scenario })
    }
  })
  pi.on("agent_before_settle", (_event, ctx) => {
    if ((scenario === "continuation" || scenario === "continuation_error") && !continued) {
      continued = true
      mark("continuation_requested", ctx, { case: scenario })
      return { continue: true, entries: [{ type: "custom_message" as const, customType: "p0.fixture",
        content: "PRIVATE_CONTINUATION_SENTINEL", display: false }] }
    }
  })
  pi.registerCommand("probe", { description: "Private P0 harness control",
    handler: async (args, ctx) => {
      const [operation, name] = args.trim().split(/\s+/)
      mark("command_start", ctx, { operation, case: name ?? operation })
      if (operation === "run") {
        if (!CASES.includes(name)) throw new Error("unknown probe case")
        const model = ctx.modelRegistry.find("p0-fixture", name)
        if (!model || !await pi.setModel(model)) throw new Error("private model unavailable")
        scenario = name; continued = false; queued = false
        // Save private path for session replacement tests; never put it in retained traces.
        const file = ctx.sessionManager.getSessionFile()
        if (file) writeFileSync(process.env.VJ_PI_SESSION_CONTROL!, file, { mode: 0o600 })
        pi.sendUserMessage("PRIVATE_USER_PROMPT_SENTINEL")
      } else if (operation === "compact") {
        const model = ctx.modelRegistry.find("p0-fixture", name)
        if (!model || !await pi.setModel(model)) throw new Error("private compaction model unavailable")
        ctx.compact({ onComplete: () => mark("manual_compact_done"), onError: () => mark("manual_compact_error") })
      } else if (operation === "name") pi.setSessionName("PRIVATE_SESSION_NAME_SENTINEL")
      else if (operation === "idle_ui") await ctx.ui.confirm("PRIVATE_IDLE_TITLE_SENTINEL", "PRIVATE_IDLE_BODY_SENTINEL")
      else if (operation === "new") { await ctx.newSession(); return }
      else if (operation === "resume") { await ctx.switchSession(readFileSync(process.env.VJ_PI_SESSION_CONTROL!, "utf8")); return }
      else if (operation === "fork") {
        const entry = ctx.sessionManager.getBranch().find(e => e.type === "message" && e.message.role === "user")
        if (!entry) throw new Error("no private entry to fork")
        await ctx.fork(entry.id, { position: "at" }); return
      } else if (operation === "tree") {
        const entry = ctx.sessionManager.getBranch().find(e => e.type === "message" && e.message.role === "assistant")
        if (!entry) throw new Error("no private entry to navigate")
        await ctx.navigateTree(entry.id, { summarize: false })
      } else if (operation === "reload") { await ctx.reload(); return }
      else if (operation === "quit") { ctx.shutdown(); return }
      else throw new Error("unknown probe operation")
      mark("command_done", undefined, { operation, case: name ?? operation })
    },
  })
}
