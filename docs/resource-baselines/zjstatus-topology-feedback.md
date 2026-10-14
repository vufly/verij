# zjstatus topology-change feedback investigation

Date: **2026-10-15**

The reported regression affected `zjstatus` mouse tab switching and its Mode widget
for approximately five to six seconds after a tab switch or pane creation/removal.
Direct keybinding and sidebar navigation remained responsive. Reloading status bars
reduced the accumulated delay but did not establish the cause.

## Verij producers identified

1. **Global SDK scan side effects.** In stock Zellij 0.45.1,
   `zellij-server/src/plugins/zellij_exports.rs::get_session_list` scans session
   metadata and sends `ScreenInstruction::UpdateSessionInfos`. The screen handler
   broadcasts `Event::SessionUpdate` to subscribers even when visible facts are
   unchanged. Verij's one-second scan per exporter therefore causes additional
   work in every subscribed status bar.
2. **Failed host-classification retries.** `fs_watcher.rs` previously did not cache
   `HostClassification::Unknown`. An exited session that could not be classified
   caused another `list-panes` subprocess on subsequent filesystem refreshes.
   Transient CLI connection teardown itself publishes Zellij state updates.
3. **Context-free focus erasure.** Stock `Tab::get_pane_info` explicitly returns
   `is_focused=false` because its query has no client context. Verij treated that
   value as a negative focus fact. Native pane events restored client-scoped focus,
   making otherwise unchanged inventory alternate and get rewritten.

These are Verij monitoring/refresh issues. The location of executor CPU consumption
in `zjstatus` was not evidence that an independent status-bar defect caused the
regression.

## Implemented changes

- Native pane/tab events discover topology changes promptly. Verij opts into
  lightweight ModeUpdate delivery through InitialKeybinds and uses mode metadata
  to track session renames.
- Existing topology still receives one-second direct validation. Full global
  session scans are reserved for bootstrap, ten-second recovery, and repair of
  an actual invalid topology cache. An asynchronous own-client list maintains
  attachment-count presentation without a global scan.
- Direct pane validation preserves the last client-scoped layer-focus flag while
  replacing live identity/membership/geometry facts. Missing panes and tabs still
  withdraw stale exporter authority.
- Unknown host classification is retained until periodic discovery, which expires
  only Unknown entries. A newly registered host overrides that temporary cache
  immediately.

## Measurements

A bounded live A/B used the same exporter build with a temporary diagnostic-only
timer pause. Session and pane identities were retained, and the original asset
was restored after measurement. Zellij CPU was approximately **85% enabled,
30% paused, and 86% resumed**. This isolates periodic Verij monitoring as an
additional producer rather than attributing all server CPU to it.

A fresh private matched setup used the same configuration, five status bars,
private XDG/socket roots, and actual terminal mouse input:

| Exporters | Original idle CPU | Initial event-driven recovery idle CPU |
|---|---:|---:|
| 0 | 1.4% | 1.2% |
| 1 | 8.4% | 3.8% |
| 2 | 16.0% | 5.0% |

That small fresh setup did not reproduce the live multi-second delay; its mouse
checks remained approximately 30–120 ms. The final exporter separately passed
five-tab pane creation/removal, actual display detachment, and private-server
cleanup with approximately 2.0% attached CPU and 1.6% detached CPU in short samples.

After bounding failed CLI probes and correcting focus erasure, a live event sample
fell from approximately 10–25 pane/tab updates per second during feedback to
approximately 2.6 per second. A corresponding server sample was approximately
72% CPU, down from approximately 260–360% during feedback. These are observations
from different short windows, not an isolated machine-independent performance
threshold.

The final live mouse check used one persistent helper in the outer Workspace
session. It wrote SGR mouse bytes to the existing nested client's terminal and
observed the exported active tab through the filesystem. No repeated CLI queries
were used during each trial. Times include tab switching and inventory publication:

| Trial | Observed time |
|---|---:|
| Settled switch | 57 ms |
| Second click 0.5 seconds later | 28 ms |
| Switch after waiting six seconds | 54 ms |
| Following rapid switch | 28 ms |
| After pane creation | 56 ms |
| Following switch after pane creation | 27 ms |
| After pane removal | 57 ms |
| Following switch after pane removal | 28 ms |

An earlier live test repeatedly polled `list-tabs` to detect completion and measured
0.5–2.7 seconds. Those probes add the same state-update traffic being investigated;
that measurement is retained as an observer-effect failure, not accepted as the
final click-latency result.

## Verification and deployment

- All **242 native unit tests**, **38 native integration tests**, and **12 plugin
  unit tests** passed. The WASM target check and `git diff --check` passed.
- The H1 runtime harness passed eight ownership/focus/acknowledgement checks before
  its known cross-session registration failure. The original exporter control
  failed at the same stage; no complete H1 runtime pass is claimed.
- The final exporter was installed and reloaded by existing plugin IDs. The native
  CLI was installed and the sidebar rerun in its existing terminal pane. Workspace
  attachment remained running, and mouse tests restored the original tab and
  removed only their newly created panes.
- Diagnostic helper permissions and any remaining helper panes were removed after
  verification. No temporary monitoring-pause feature remains in production code.

Retained scratch reports are under `/tmp/opencode`, including
`verij-live-monitoring-isolation.json`, `verij-final-topology-verification.json`,
`verij-event-topology-navigation.json`, and
`verij-original-topology-navigation-control.json`. Live filesystem mouse reports
are `/tmp/zellij-1000/verij-live-mouse-{tab-switches,pane-create,pane-remove}.json`.
These scratch files may disappear after reboot; this document retains their scoped
conclusions.

Recheck topology, detachment, and resource behavior with the existing harness:

```bash
cargo test -p verij-cli
cargo test -p verij-plugin
cargo check -p verij-plugin --target wasm32-wasip1
cargo build -p verij-plugin --target wasm32-wasip1 --release
python3 prototypes/resource-usage/verify.py \
  --plugin target/wasm32-wasip1/release/verij-plugin.wasm \
  --status-plugin /path/to/zjstatus.wasm --tabs 5 --pollers 0 --seconds 5 \
  --output /tmp/opencode/verij-topology-recheck.json
```
