import { Plugin } from "@opencode/plugin/tui"
import { appendFileSync } from "node:fs"
import { resolve } from "node:path"
import { observe } from "./cli-observer-core.mjs"

export default Plugin.define({
  id: "verij.cli.contract",
  setup(context) {
    const root = process.env.VJ_V2_PROBE_ROOT
    if (!root || resolve(root, "work") !== process.cwd()) throw new Error("private_fixture_required")
    return observe(context, frame => {
      appendFileSync(resolve(root, "cli-observer.ndjson"), JSON.stringify({ ...frame, pid: process.pid }) + "\n", { mode: 0o600 })
    }, { generation: context.options.generation })
  },
})
