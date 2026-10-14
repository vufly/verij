# Agent monitoring P0 baselines

Reports in this directory retain the pre-throttling monitoring baseline. Each matched
pair uses fresh private Zellij sockets, XDG directories, permissions, configurations,
agent records, and control files. The enabled case runs real Verij sidebars and the
native reporter. The disabled case replaces sidebars with sleeping terminals and
disables the OpenCode bridge; exporters, status bars, layouts, and attachments remain
identical. This isolates sidebar/bridge overhead rather than exporter overhead.

`synthetic-*.json` uses a real foreground Node process, production bridge, native
reporter, Zellij servers, and passive visit protocol, but synthetic API responses and
owned events. `opencode-*.json` uses installed OpenCode and its actual API/TUI event
stream against a private controlled SSE provider. `scheduler.json` measures synthetic
API scheduling only, with an immediately exiting reporter stub. These measurements
must not be combined as if synthetic requests were real HTTP traffic.

Each report records tested and installed asset hashes, binary versions, topology,
warm-up/workload duration, per-process CPU/RSS, operation/target CLI counters,
endpoint API counters, committed native record writes, proof attempts, filesystem
notification ingress/coalescing, host-local completion acknowledgement, detachment,
and private-process cleanup. Native and API counters publish one replaceable snapshot
per second outside watched directories. Native CLI counter cardinality is capped at
32 keys with an overflow counter. Interval summaries retain at most 60 entries per
phase. No per-event payload logs are retained by the measurement probes.

Successful synthetic pairs cover `(hosts, tabs)` = `(1, 4)`, `(1, 8)`, `(2, 4)`,
and `(2, 8)`. Most phases run for 10 seconds after five seconds of warm-up. The
two-host/eight-tab long run hit the 512 MiB Zellij RSS guard; its raw failure and
successful cleanup are retained alongside a three-second matched pair covering all
phases. Actual OpenCode pairs cover one host/four tabs and two hosts/eight tabs.
`rss-guard.json` deliberately lowers the guard to 32 MiB and verifies early-abort
cleanup, including private processes not yet announced by exporters.

The existing H1 navigation harness also retains asset hashes and elapsed time.
`navigation-installed-assets.json` passed; `navigation-instrumented-cli.json` and
`navigation-head-control.json` expose the same cross-session registration failure.
The HEAD control has no P0 instrumentation and the same binary hash as the installed
CLI. These reports document a pre-existing intermittent correctness failure rather
than treating it as a resource-baseline success.

CPU/RSS totals cover long-lived private servers, terminal clients, pane processes,
and reporters. Transient CLI processes are counted separately; their CPU/RSS is not
included in the long-lived totals. Counter snapshots have one-second resolution, so
phase edges can include up to one snapshot interval of adjacent work. Actual measured
event counts and elapsed durations are retained. Short P0 windows establish baseline
cost, not leak freedom or the later four-hour soak acceptance.

## Reproduction

```bash
cargo build -p verij-cli --release
python prototypes/resource-usage/verify.py \
  --cli target/release/verij --plugin /path/to/installed/verij_plugin.wasm \
  --status-plugin /path/to/zjstatus.wasm --tabs 4 --hosts 1 \
  --warmup 5 --seconds 10 --quiet --output /tmp/opencode/p0.json

# Repeat for (hosts, tabs): (1, 8), (2, 4), (2, 8).
# Actual OpenCode runtime, controlled local provider; same matched topology:
python prototypes/resource-usage/verify.py \
  --cli target/release/verij --plugin /path/to/installed/verij_plugin.wasm \
  --status-plugin /path/to/zjstatus.wasm --tabs 4 --hosts 1 \
  --agent-runtime opencode --max-agent-rss-mib 1536 \
  --warmup 5 --seconds 10 --quiet --output /tmp/opencode/p0-runtime.json

node prototypes/resource-usage/opencode_probe.mjs \
  --warmup 1 --seconds 3 --output /tmp/opencode/p0-scheduler.json

python prototypes/resource-usage/verify_evidence.py
python prototypes/resource-usage/verify_evidence.py --summary --phase idle_agent
python prototypes/resource-usage/verify_evidence.py \
  --expect-rss-abort docs/resource-baselines/rss-guard.json
```

The default 512 MiB RSS abort guard applies to each private process. Actual OpenCode
has a larger startup footprint; `--max-agent-rss-mib` overrides only its server/TUI
guard, leaving Zellij's 512 MiB guard intact. Every convergence wait and workload
checks RSS. Cleanup targets only private sessions and birth-qualified retained PIDs.

## Initial P2 scheduler milestone

`p2-synthetic-start.json` and `p2-opencode-start.json` are post-P0 development
measurements of the scheduler foundation, not replacements for the P0 controls.
They use the globally built CLI/exporter and checkout bridge/core; every report
records those asset hashes. Installed bridge/core still identify the P0 assets.

The synthetic one-member case measured 0.8 API calls/sec and 0.2 native writes/sec
while idle. Both 20- and 100-event/sec stream phases measured 8.0 API calls/sec;
unchanged working snapshots renewed their source lease instead of writing per event.
The actual OpenCode pair also passed completion acknowledgement, detachment, and
cleanup. Its five-second windows are too short to estimate a ten-second idle
recovery rate precisely; deterministic scheduler tests check that interval.

P2 remains in progress: family caching, resumable traversal within the endpoint
budget, and metadata-only outcome recovery are still pending. Existing full message
retrieval is throttled but not yet removed. Verify these milestone reports explicitly:

```bash
python prototypes/resource-usage/verify_evidence.py \
  docs/resource-baselines/p2-synthetic-start.json \
  docs/resource-baselines/p2-opencode-start.json
```

## P1 passive proof scheduling

`p1-1host-4tabs.json` and `p1-2hosts-8tabs.json` retain matched controls and nine enabled
workload phases. They check idle/working without proofs, completion on the sidebar,
another inner pane, actual keyboard navigation back to the agent, a second outer
display, failed proof backoff, focus-triggered recovery, and display detachment.
Both hosts retain independent acknowledgements for two completion revisions.

Each successful passive visit consumes one fresh five-command proof. A second outer
display fails the fresh `list-clients` check: three attempts in ten seconds, without
pipe calls or acknowledgement for that host. Removing the display and changing focus
resets backoff and obtains one new proof. Idle CLI traffic is 0.1 calls/sec per host,
with zero visit RPCs: approximately 98% below P0's 5.5 calls/sec per host.

No-exporter discovery now has a ten-second fallback. Filesystem-driven exporter
topology remains prompt and does not inherit ten seconds of stale discovery
authority. `p1-opencode-1host-4tabs.json` also verifies actual OpenCode runtime.
These runs include the P2 scheduler foundation and newer installed exporter; hashes
are retained. CLI/proof budgets isolate P1, while total CPU/RSS includes those other
paths. Short checks do not replace the later combined four-hour soak.

`p1-navigation.json` passed H1's ownership, fullscreen/float/stack focus, explicit
host-local acknowledgement, old-revision races, and sidebar-refusal gates, then hit
the already recorded cross-session registration failure. Cleanup passed.

```bash
python prototypes/resource-usage/verify.py \
  --cli target/release/verij --plugin /path/to/installed/verij_plugin.wasm \
  --status-plugin /path/to/zjstatus.wasm --tabs 8 --hosts 2 \
  --warmup 5 --seconds 10 --verify-p1-visits --quiet \
  --output /tmp/opencode/p1-visits.json
python prototypes/resource-usage/verify_evidence.py /tmp/opencode/p1-visits.json
```
