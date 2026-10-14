# P3 private-runtime smoke verification

Date: **2026-10-15**. Result: **PASS**.

This is a short integration smoke check of the P3 watcher changes, not a matched
performance comparison or a soak. It used stock Zellij, a real sidebar, native
reporting and completion acknowledgement, with synthetic OpenCode API/events.

## Reproduction

```sh
cargo build -p verij-cli
python prototypes/resource-usage/verify.py \
  --cli target/debug/verij \
  --plugin target/wasm32-wasip1/release/verij-plugin.wasm \
  --hosts 1 --tabs 4 --monitoring enabled \
  --seconds 3 --warmup 2 \
  --output /tmp/opencode/verij-p3-smoke.json --quiet
```

The original report was written to `/tmp/opencode/verij-p3-smoke.json`; the
retained parameters and selected results below summarize that run. The private
runtime root was `/tmp/opencode/verij-baseline-y9gij5vq`.

## Tested assets and parameters

| Item | Value |
|---|---|
| Zellij | 0.45.1 |
| Zellij SHA-256 | `d006c521dcb475a6005d741e9dd7c5758e5a23b28dd60a5c10cebfa4876319dd` |
| Debug CLI SHA-256 | `491b1411f015ec8e974aff0df7b269988adcc910f35cebcc3db2b545ec76f0bb` |
| Exporter SHA-256 | `d5c3564ffa4004aacfc7187e0224fa29af493242f0bf3615405dde5dd20683e5` |
| Hosts / inner tabs | 1 / 4 |
| Status bars | 0 |
| Warm-up / each workload phase | 2 seconds / approximately 3 seconds |
| Zellij RSS abort guard | 512 MiB |

## Results

Topology refresh, completion visit, detachment and private-process cleanup all
passed. Completion revision 1 was acknowledged by host `a`, using one proof attempt.
Cleanup reported `known_processes_exited: true` and no remaining PIDs.

| Phase | Observed owned events | Total CPU (%) | Endpoint total RSS (MiB) |
|---|---:|---:|---:|
| Idle agent | 0 | 7.33 | 200.72 |
| Requested 20 events/second | 55 | 12.00 | 200.90 |
| Requested 100 events/second | 279 | 11.67 | 202.12 |
| Completion visit | 22 | 13.99 | 201.71 |
| Idle detached | 0 | 4.00 | 122.28 |

Opt-in native metrics remained enabled throughout. The requested 20-event phase
recorded four incoming topology notifications and three coalesced topology
notifications, confirming the atomic measurement path was exercised.

These brief measurements do not establish long-term RSS stability, multi-host
performance, or behavior with status bars. Those remain part of P5 verification.
