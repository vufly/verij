# Continue agent monitoring on another computer

This guide reconstructs the development environment for the approved H2 baseline and H3 OpenCode review checkpoint. It also records the handoff context that otherwise lives in conversations and machine-local workspace files.

## 1. Checkpoint and source of truth

Repository: `https://github.com/vufly/verij`, branch **`agent-monitoring`**.

| Checkpoint | Commit | State |
|---|---|---|
| H0: stock navigation contract | `e66293f` | Approved and pushed |
| H1: topology, normalized store and navigation | `bd2cc4f` | Approved and pushed |
| H2: agent tree and host-local completion UI | `0b713191047b8d30ddb42edaa5e94b64b58b2b1c` | Approved and pushed |
| H3: OpenCode production adapter | `445519f` | Committed and verified; human review pending |

H2 includes Session → Tab → Agent rows, nested folds, configurable titles/styles, status summaries, persistent tree state, asynchronous activation and host-local acknowledgement after confirmed visits. **H3 OpenCode is implemented and ready for review; H4 Agy/Magy remains pending.** After [adapter setup](../integrations/opencode/README.md) and an OpenCode restart, normal interactive OpenCode/attach processes register automatically against verified native inventory. H2's harness continues to use real receiver processes with explicitly synthetic semantic reports.

Read these files when resuming:

1. [Progress](AGENT_MONITORING_PROGRESS.md): current implementation, approval and evidence ledger.
2. [Product design](AGENT_MONITORING.md): approved behavior and accepted limits.
3. [Implementation guide](AGENT_MONITORING_IMPLEMENTATION.md): contracts and the H3 onward sequence. Its early proposed module layout is not the actual implementation inventory.
4. [Gate readiness](AGENT_MONITORING_GATE_READINESS.md): resolved decisions and remaining checkpoint ownership.
5. [H2 fixture guide](../prototypes/h2-tree/README.md): reproduction and review helper.
6. [H3 fixture guide](../prototypes/h3-opencode/README.md): production OpenCode reproduction and human-review checklist.

Older dated paragraphs describe historical states. The latest checkpoint ledger and separate approval records take precedence over earlier “pending” or “unapproved” text. The old machine's workspace `context/brief.md` is an obsolete H0 startup brief, not the current task.

### Constraints to retain

- Use **stock, unmodified Zellij**. No fork patches, fork builds/installations, packaging or upstream transport work.
- Native shared-stack behavior and documented Agy question/cancellation/deadline limits are accepted.
- Use native boot/PID/start/TTY ownership and exact registered client context. Pane names, inherited environment and CLI success are not ownership or visit proof.
- Done is an unread successful completion for this host. Highlight, folding and sidebar focus do not acknowledge it; failed or unverified navigation preserves it.
- Stock actions already dispatched cannot be revoked. Verij suppresses obsolete local follow-up/results.
- Retain explicit human checkpoints. H2 approval is recorded; later checkpoints need their own feedback.

## 2. What Git carries, and what it does not

| Item | Available after clone? | New-machine action |
|---|---|---|
| Verij source, `Cargo.lock`, design/progress docs | Yes | Check out `agent-monitoring` |
| Harness scripts, reviewed fixture projections, result summaries, approval JSON | Yes | Read them; generate new local evidence to verify this computer |
| `target/release/verij`, `dist/verij_plugin.wasm` | No; build directories are ignored | Rebuild both artifacts |
| Original scratch roots, callback metadata, diagnostic frames, failed attempts | No; under sibling workspace `artifacts/` | Optional private backup, or rerun relevant harnesses |
| Workspace `context/`, local handoffs and historical worker reports | No | Optional backup; this guide and progress log provide the essential handoff |
| Sibling `opencode-reference/` checkout | No | Optional read-only clone for H3 research |
| Historical sibling `zellij/` checkout | No | Not required to build or continue Verij |
| User Verij/Zellij configuration and host state | No | Recreate, or selectively copy customization |
| OpenCode/Agy/Magy installation, profile login, model availability, agent harness/MCP settings | No | Install/configure/authenticate on the new computer when needed |
| Live sessions, PTYs, sockets, process/client bindings, control journals | No | Start and register fresh processes; these identities cannot migrate |

Committed result JSON contains absolute paths to the original computer's evidence roots. Those paths are provenance, not setup instructions. A validator invoked on an old committed report can fail because its scratch root is absent even though the code is present. Do not rewrite original evidence to make it appear newly verified; pass a newly generated report to the validator.

Before leaving the old computer, check `git status --short --branch`. Commit and push intended new work, including this guide when publishing it, or transfer uncommitted files separately. A clone restores pushed Git history, not the old worktree's uncommitted files.

## 3. Prerequisites

Use **Linux or Linux inside WSL2** for the verified monitoring path. Ownership checks use Linux `/proc`, boot IDs, process births and foreground terminals. macOS/native Windows do not supply the currently verified process-binding implementation.

Install:

- Git and access to the repository.
- A C linker/build tools, `make`, `pkg-config`, OpenSSL development headers/libraries, Python **3.9 or newer**, `tmux`, and util-linux `script`. The native workspace checks/tests compile the published Zellij SDK's HTTP dependencies, which need OpenSSL discovery through `pkg-config`.
- Rust through `rustup`; the final H2 build was tested with **Rust 1.98.1** and **`wasm32-wasip1`**.
- Stock **Zellij 0.45.1** on `PATH`. `Cargo.lock` resolves the published `zellij-tile` SDK to 0.45.1.

For Debian/Ubuntu/WSL Ubuntu, the native tools can be installed with:

```sh
sudo apt-get update
sudo apt-get install -y build-essential make git python3 tmux util-linux pkg-config libssl-dev
```

Install rustup using its official installation instructions if it is absent, then install the tested toolchain without changing your global default:

```sh
rustup toolchain install 1.98.1 --profile minimal --component rustfmt --target wasm32-wasip1
export RUSTUP_TOOLCHAIN=1.98.1
rustc --version
rustup target list --installed --toolchain 1.98.1
```

The last command must include `wasm32-wasip1`. The old machine's `/tmp/opencode/verij-rustup` was a temporary workaround, not something to copy or a required path on the new computer.

Install unmodified Zellij 0.45.1 using your preferred installer. For example, if mise is already installed:

```sh
mise install zellij@0.45.1
export PATH="$(mise where zellij@0.45.1):$PATH"
command -v zellij
zellij --version
```

The harness discovers `zellij` from `PATH`; installing a version without selecting it in the current shell is insufficient. Use the official executable, not a binary from a historical dependency checkout. This version selection is local to the shell.

H1/H2 verification needs **no OpenCode, Agy, Magy, model provider or agent authentication**. These are dependencies for later adapter work and optional runtime probes, not for rebuilding the approved foundation/UI.

## 4. Clone and build

The following layout matches the harness documentation. A different absolute workspace path is fine.

```sh
export WORKSPACE="$HOME/repos/workspaces/verij-agent-monitoring"
mkdir -p "$WORKSPACE/artifacts/scratch"
git clone --branch agent-monitoring git@github.com:vufly/verij.git "$WORKSPACE/verij"
cd "$WORKSPACE/verij"
git status --short --branch
git log --oneline -5
```

Use `https://github.com/vufly/verij.git` instead if SSH access is not configured. The branch history should contain the approved H2 commit `0b71319`; later documentation commits can appear above it.

From the checkout root, with the toolchain and Zellij selected in this shell:

```sh
cargo fetch --locked
cargo check --workspace --locked
cargo test --workspace --locked
make check
make dev
```

At the H2 checkpoint, the expected Rust total is **170 tests**: 136 CLI unit, eight host integration, four plugin and 22 shared-type tests. The build creates:

```text
target/release/verij
dist/verij_plugin.wasm
```

Keep the committed lockfile; dependency upgrades are separate changes. `make dev` builds Verij and its published-SDK WASM plugin, not Zellij. The checkout does not require the old computer's installed Verij binary or a global Verij installation.

## 5. Recreate verification evidence

Run from the Verij checkout root. Store new reports outside tracked source files:

```sh
python3 prototypes/h2-tree/verify.py \
  --cli target/release/verij \
  --plugin dist/verij_plugin.wasm \
  --scratch-dir ../artifacts/scratch \
  --output ../artifacts/h2-new-machine.json
python3 prototypes/h2-tree/verify_evidence.py ../artifacts/h2-new-machine.json
```

Expected result: **ten live checks pass**, the evidence validator passes, and private socket/known-process cleanup passes. The harness creates generated configuration and private XDG/socket directories; it does not need your usual Zellij layout or user plugin permissions. Semantic records are visibly synthetic over actual receiver processes and stock navigation.

For the H1 navigation/storage regression:

```sh
python3 prototypes/h1-navigation/verify.py \
  --cli target/release/verij \
  --plugin dist/verij_plugin.wasm \
  --scratch-dir ../artifacts/scratch \
  --output ../artifacts/h1-new-machine.json
python3 prototypes/h1-navigation/verify_evidence.py ../artifacts/h1-new-machine.json
```

Expected result: **eleven live checks pass** and the validator passes. Run live harnesses sequentially so simultaneous native terminal tests do not add avoidable contention. If a run fails, retain its original FAIL report and printed diagnostic root, then diagnose that failure rather than treating earlier approval as a new-machine PASS.

### Optional interactive fixture

```sh
python3 prototypes/h2-tree/verify.py \
  --cli target/release/verij \
  --plugin dist/verij_plugin.wasm \
  --scratch-dir ../artifacts/scratch \
  --output ../artifacts/h2-review-new-machine.json \
  --keep-live
```

Only a passing run is retained. It prints **new** host-a and host-b tmux attach commands. All previously documented review sockets were cleaned after approval and cannot be reused.

Load the new root from the report, then prepare display states:

```sh
ROOT="$(python3 -c 'import json; print(json.load(open("../artifacts/h2-review-new-machine.json"))["root"])')"
python3 prototypes/h2-tree/review.py prepare --root "$ROOT"
python3 prototypes/h2-tree/review.py sidebar --root "$ROOT" --host a
python3 prototypes/h2-tree/review.py sidebar --root "$ROOT" --host b
```

Attach using the commands printed by this run. Both tmux views share their existing native outer display; do not add extra direct Zellij attachments to an outer host. Verified whole-host visits currently require one attached native display per outer host.

After inspection:

```sh
python3 prototypes/h2-tree/review.py clean --root "$ROOT"
```

The helper validates the private manifest and known process births. Copying a manifest from another machine does not recreate a running fixture.

## 6. User configuration and runtime state

For a normal local host, initialize configuration and create a fresh named host after building:

```sh
target/release/verij config init
target/release/verij config path
target/release/verij start --session-name monitoring-dev
```

The normal host is separate from the synthetic test fixture. To enable H3 OpenCode monitoring, run `target/release/verij agent setup opencode`, quit/restart OpenCode, then inspect `target/release/verij agent doctor`. The installed reporter path must remain available. Agy/Magy adapters follow at H4.

These customization files can be copied if you want the same appearance:

- `$XDG_CONFIG_HOME/verij/config.toml` and `verij.kdl`, normally `~/.config/verij/`.
- Relevant Zellij configuration, themes and layout templates, normally `~/.config/zellij/`.
- Your global agent instructions, skills and MCP configuration, if desired; these are not part of this repository.

Keep the Verij layout's `{{verij_bin}}` placeholder rather than copying a generated layout containing the old executable path. Check custom plugin URLs and pane commands for old absolute paths. Generated files under `$XDG_CACHE_HOME/verij` should be regenerated.

Durable host state is normally under `~/.local/state/verij/`: `host-registry.toml`, `hosts.toml`, `agent-monitoring/hosts/` and `agent-monitoring/tree/`. You may back it up, but saved acknowledgements and selected/folded nodes reference old agent/session instances. They do not restore a visit or matching pane on a different computer. Recreate host wrappers and registrations before relying on navigation.

Runtime agent records, inventory, attachment markers and control journals live under runtime/tmp directories, with `VERIJ_*` and XDG overrides available to harnesses. **Do not restore them as live state.** New boot/PID/TTY/client identities must be observed locally. Old sockets, plugin epochs and registrations are invalid on the new computer.

## 7. Optional tools for H3/H4 and deeper research

The inspected OpenCode source is an optional sibling checkout:

```sh
git clone --depth 1 --branch v1.18.32 \
  https://github.com/anomalyco/opencode.git "$WORKSPACE/opencode-reference"
git -C "$WORKSPACE/opencode-reference" rev-parse HEAD
```

The original reference commit was `545f51d26cc39a907d2867492d498d9607ea5fa4`. Keep this reference read-only. Installed OpenCode runtime evidence used **1.18.33**; the reference and installed executable are deliberately distinct. Other agent versions need their contracts verified rather than inheriting old version claims.

Agy runtime evidence used **1.2.14**. Agy 1.2.16 was later available for tooling/model-selection attempts, not a new full runtime acceptance matrix. Install/authenticate OpenCode, Agy and Magy separately when their work is needed; credentials are not in Git.

If using Magy:

```sh
magy doctor
magy profile list
agy --version
```

Authenticate a profile on this computer and configure the agent harness/MCP connection using the installed tool's supported commands. Check the executable selected by `magy doctor`, not only the executable reached by a bare `agy` command.

The user's preferred workflow is Sonnet planning for tricky work and Magy's default model for scoped workers. `claude-sonnet-5-5-high` was rejected by the previous profile model list; Sonnet 4.6 Thinking was used as an explicitly reported fallback. Retry the requested identifier only when available in the new profile; model availability is machine/profile-local. Magy launcher metadata such as `terminal_<PID>` is not native Zellij pane ownership.

Before reproducing deeper runtime probes, read their own guides: [OpenCode](../prototypes/opencode-runtime/README.md) and [Agy](../prototypes/agy-runtime/README.md). They have additional executable/authentication requirements and retained capability qualifications. Their original local evidence is optional for H2 continuation but useful for investigating those exact observations.

## 8. Optional backup from the old computer

For original evidence and detailed conversation handoffs, selectively transfer the sibling workspace's `context/handoffs/` and relevant `artifacts/` reports/scratch roots. Useful final evidence roots are:

- `artifacts/scratch/vj-h2-sqlylf7p`: final cleaned H2 automated run.
- `artifacts/scratch/vj-h1-elhs90js`: cleaned H1 regression against H2 code.
- `artifacts/scratch/vj-h2-m3q16cxs`: reviewed H2 instance, now cleaned.

The published result/approval summaries preserve these names. Archived original files retain the old machine's paths and process identities; keep them as historical evidence, not runtime state. Fresh harness runs are the straightforward way to obtain locally runnable validators.

`target/`, `dist/`, caches and the temporary private Rust installation can be rebuilt. Authentication and agent conversation history need their own supported login/export mechanism; they are not part of a source clone. Keep raw runtime logs and credentials out of source commits; only reviewed status/schema/identity projections were intended for publication.

## 9. Handoff prompt for a fresh coding-agent session

```text
Continue Verij agent monitoring on branch agent-monitoring.
H0 e66293f, H1 bd2cc4f and H2 0b71319 are approved and pushed.
H3 is committed as 445519f; master 854ecb1 is integrated. See progress log for merged verification and H3 review status.
Read docs/AGENT_MONITORING_SETUP.md and the current progress/design/implementation docs.
H2 has the three-level tree and host-local completion UI; H3 OpenCode is implemented and ready for its own human review. H4 Agy/Magy remains pending.
Use stock Zellij 0.45.1 and the published SDK. Never patch/build/install the historical Zellij fork.
Native shared-stack behavior and documented Agy limitations are accepted.
Do not infer ownership or Done from terminal text, inherited environment, idle, or wrapper SUCCESS.
Keep completion acknowledgement host-local and require a fresh confirmed visit of the captured revision.
Old runtime bindings/sockets and absolute scratch paths are not valid on this machine.
Build and regenerate H2/H1 evidence locally before relying on this environment.
Use Sonnet planning where available and Magy's default model for explicitly requested scoped workers.
Read integrations/opencode/README.md and prototypes/h3-opencode/README.md; regenerate local production evidence and stop for H3 feedback before H4.
```

This supplies the essential project context without needing the previous agent conversation or workspace orchestration installation.
