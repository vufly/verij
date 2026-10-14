/** Active P4 fixture. Never installed globally or included in production assets. */
import { spawn } from "node:child_process"
import { writeFileSync } from "node:fs"
import { join } from "node:path"
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent"
import { Type } from "typebox"

export default function (pi: ExtensionAPI) {
  pi.registerTool({ name: "p4_burst", label: "P4 burst", description: "Private resource workload",
    parameters: Type.Object({}),
    async execute(_id, _args, _signal, _update, ctx) {
      // Real Pi nested execution events inside one aggregate, not projector calls.
      for (let i = 0; i < 160; i++) await ctx.executeTool("probe_leaf", { delayMs: 5 })
      writeFileSync(join(process.env.VJ_P4_ROOT!, `burst-done-${process.env.ZELLIJ_PANE_ID}`), "done", { mode: 0o600 })
      return { content: [{ type: "text", text: "PRIVATE_BURST_RESULT_SENTINEL" }], details: undefined }
    },
  })
  pi.registerCommand("p4", { description: "Private acceptance controls", handler: async (args, ctx) => {
    const root = process.env.VJ_P4_ROOT!
    if (args === "burst") {
      const model = ctx.modelRegistry.find("p4-workload", "burst")
      if (!model || !await pi.setModel(model)) throw new Error("private burst model missing")
      pi.sendUserMessage("PRIVATE_BURST_PROMPT_SENTINEL")
    } else if (args === "detach") {
      spawn(process.env.VJ_P4_PYTHON!, [process.env.VJ_P4_DETACHED!, root], {
        detached: true, stdio: "ignore", env: process.env,
      }).unref()
    }
  } })
}
