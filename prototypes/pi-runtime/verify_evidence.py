#!/usr/bin/env python3
"""Independently check P0 traces, privacy, lifecycle semantics, and cleanup."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re

HERE = Path(__file__).resolve().parent
RUNTIMES = {("compiled", "observer-first"), ("compiled", "observer-last"), ("node", "observer-first"), ("node", "observer-last")}
RUN_CASES = (
    "success", "parallel", "nested", "tool_error", "provider_error", "retry_success", "retry_exhausted",
    "cancel", "queued", "late_queued", "continuation", "auto_compact", "ui", "ui_nested", "postrun_error", "continuation_error",
)
CASE_ORDER = ["startup", *RUN_CASES, "manual_compact", "success", "compact_error", "idle_ui", "name", "tree", "new", "resume", "fork", "reload", "quit"]
EVENTS = {
    "session_start", "session_shutdown", "session_info_changed", "session_before_switch", "session_before_fork",
    "session_before_tree", "session_tree", "session_before_compact", "session_compact", "session_compact_failed",
    "before_agent_start", "agent_start", "agent_end", "agent_before_settle", "agent_settled", "turn_start", "turn_end",
    "message_start", "message_end", "tool_execution_start", "tool_execution_end", "ui_prompt_start", "ui_prompt_end",
    "observer_loaded", "fixture_loaded", "fixture_ready", "command_start", "command_done", "queue_submitted",
    "continuation_requested", "nested_child_start", "nested_child_end", "manual_compact_done", "manual_compact_error",
    "fixture_error", "postrun_failure_injected",
}
ENUMS = {
    "source": {"observer", "fixture"}, "event": EVENTS, "version": {"1.2.0"},
    "reason": {"startup", "reload", "new", "resume", "fork", "quit", "manual", "threshold", "overflow", "ui_prompt"},
    "outcome": {"completed", "aborted", "error"}, "kind": {"select", "confirm", "input", "editor", "custom"},
    "role": {"user", "assistant", "toolResult", "custom", "bashExecution"},
    "stop_reason": {"stop", "length", "toolUse", "aborted", "error"},
    "case": set(CASE_ORDER), "operation": {"run", "compact", "name", "new", "resume", "fork", "tree", "reload", "quit", "idle_ui"},
    "tool": {"probe_leaf", "probe_nested", "probe_queue", "probe_ui", "probe_ui_nested"},
    "mode": {"tui", "print", "json", "rpc"}, "engine": {"bun", "node"},
}
BOOLEANS = {
    "idle", "pending", "signal_aborted", "aborted", "will_retry", "from_extension", "continue_proposed", "can_continue",
    "is_error", "error_present", "name_present",
}
HASHES = {"runtime", "session", "call", "parent_call", "entry"}
INTEGERS = {"schema_version", "pid", "turn_index", "tool_count", "at_ms", "index"}
FIELDS = set(ENUMS) | BOOLEANS | HASHES | INTEGERS | {"engine_version"}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def validate_trace(trace):
    require(isinstance(trace, list) and 0 < len(trace) < 8192, "empty or oversized trace")
    for index, event in enumerate(trace):
        require(isinstance(event, dict) and set(event) <= FIELDS, "unapproved trace field")
        require(event.get("schema_version") == 1 and event.get("index") == index, "invalid trace schema/order")
        for field, allowed in ENUMS.items():
            if field in event:
                require(isinstance(event[field], str) and event[field] in allowed, f"invalid enum {field}")
        for field in BOOLEANS & set(event):
            require(type(event[field]) is bool, f"non-boolean {field}")
        for field in HASHES & set(event):
            require(isinstance(event[field], str) and re.fullmatch(r"[a-f0-9]{20}", event[field]), f"unhashed {field}")
        for field in INTEGERS & set(event):
            require(type(event[field]) is int and event[field] >= 0, f"invalid integer {field}")
        require(isinstance(event.get("engine_version"), str) and re.fullmatch(r"\d+\.\d+\.\d+", event["engine_version"]), "missing runtime engine version")
        require({"source", "runtime", "event", "pid", "at_ms", "version", "engine", "engine_version"} <= set(event), "missing event provenance")
        require(event["event"] != "fixture_error", "fixture failed")
        if event["source"] == "observer" and event["event"] != "observer_loaded":
            require(event.get("mode") == "tui" and "session" in event and type(event.get("idle")) is bool, "not a real TUI session observation")


def observer(case):
    return [e for e in case["trace"] if e["source"] == "observer"]


def named(trace, event):
    return [e for e in trace if e["event"] == event]


def single(trace, event):
    values = named(trace, event)
    require(len(values) == 1, f"expected one {event}, got {len(values)}")
    return values[0]


def index(trace, event):
    return single(trace, event)["index"]


def validate_run_case(case, order):
    name, trace = case["case"], observer(case)
    require(len(named(trace, "before_agent_start")) == 1, f"{name}: aggregate must start once")
    settled = single(trace, "agent_settled")
    require(settled.get("idle") is True and settled.get("pending") is False, f"{name}: not quiescent")
    require("outcome" not in settled, "settlement unexpectedly claims an outcome")
    require(settled["index"] > max(e["index"] for e in named(trace, "agent_end")), f"{name}: early settlement")
    turns = named(trace, "turn_end")
    require(turns and all(e["index"] < settled["index"] for e in turns), f"{name}: invalid turn order")
    if name == "cancel":
        require(settled.get("aborted") is True and turns[-1].get("outcome") == "aborted", "cancel misclassified")
        require(not named(trace, "agent_before_settle"), "unexpected canceled before-settle")
    elif name in ("postrun_error", "continuation_error"):
        require(settled.get("aborted") is False and turns[-1].get("outcome") == "completed", "postrun failure fixture did not complete its turn")
        boundaries = named(trace, "agent_before_settle")
        if name == "postrun_error":
            require(not boundaries, "postrun exception unexpectedly passed pre-settlement")
        else:
            require(len(boundaries) == 1 and len(named(trace, "agent_start")) == 2, "continuation failure not exercised")
            require(boundaries[0]["index"] < named(trace, "agent_start")[-1]["index"], "stale success candidate not followed by resumed work")
        require(index(case["trace"], "postrun_failure_injected") < settled["index"], "missing actual postrun injection")
    else:
        boundaries = named(trace, "agent_before_settle")
        require(boundaries and boundaries[-1]["index"] < settled["index"], f"{name}: missing final boundary")
        outcome = "error" if name in ("provider_error", "retry_exhausted") else "completed"
        require(boundaries[-1].get("outcome") == turns[-1].get("outcome") == outcome, f"{name}: wrong final outcome")
        require(settled.get("aborted") is False, f"{name}: unexpected abort")
        require(turns[-1].get("stop_reason") == ("error" if outcome == "error" else "stop"), f"{name}: outcome not backed by final assistant metadata")
    active = set()
    peak = 0
    for event in trace:
        if event["event"] == "tool_execution_start":
            require(event["call"] not in active, "duplicate active call")
            active.add(event["call"])
            peak = max(peak, len(active))
        elif event["event"] == "tool_execution_end":
            require(event["call"] in active, "tool end without start")
            active.remove(event["call"])
        elif event["event"] == "agent_settled":
            require(not active, "settled with active tools")
    if name == "parallel":
        require(peak == 2 and len(named(trace, "tool_execution_start")) == 2, "parallel tools were not actually overlapping")
    if name == "nested":
        starts = named(trace, "tool_execution_start")
        require(len(starts) == 3 and peak == 3, "missing nested overlapping calls")
        root = next(e for e in starts if e.get("tool") == "probe_nested")
        children = [e for e in starts if e.get("parent_call") == root["call"]]
        require(len(children) == 2, "nested calls lost parent attribution")
    if name == "tool_error":
        require(any(e.get("is_error") is True for e in named(trace, "tool_execution_end")), "recoverable tool error not observed")
    if name in ("retry_success", "retry_exhausted"):
        require(len(named(trace, "agent_start")) == len(named(trace, "agent_end")) == 2, "retry not an actual second low-level run")
        require(turns[0].get("outcome") == "error", "retry did not begin with failure")
    if name == "queued":
        require(len(turns) == 3 and len(named(case["trace"], "queue_submitted")) == 1
            and any(e.get("pending") is True for e in trace), "steering/follow-up not actually queued")
    if name == "late_queued":
        require(len(named(trace, "agent_start")) == 2 and len(turns) == 2, "late queued run not continued")
        require(index(case["trace"], "queue_submitted") < settled["index"], "late queue ran after settlement")
    if name == "continuation":
        boundaries = named(trace, "agent_before_settle")
        require(len(boundaries) == 2 and len(named(trace, "agent_start")) == 2, "extension continuation not actually executed")
        require(boundaries[0].get("continue_proposed") is (order == "observer-last"), "load-order-sensitive proposal missing")
        require(boundaries[-1].get("continue_proposed") is False, "continuation did not finish")
        require(index(case["trace"], "continuation_requested") < settled["index"], "missing continuation request")
    if name == "auto_compact":
        start, end = single(trace, "session_before_compact"), single(trace, "session_compact")
        require(start.get("reason") == end.get("reason") == "threshold" and start["index"] < end["index"] < settled["index"], "automatic compaction not within aggregate run")
        require(start.get("idle") is False and len(turns) == 2, "compaction lost active run")
    if name in ("ui", "ui_nested"):
        starts, ends = named(trace, "ui_prompt_start"), named(trace, "ui_prompt_end")
        expected = ["select", "confirm", "input", "editor", "custom"] if name == "ui" else ["custom"]
        require([e.get("kind") for e in starts] == [e.get("kind") for e in ends] == expected, "wrong UI prompt visibility")
        for start, end in zip(starts, ends):
            require(start["index"] < end["index"] < settled["index"], "prompt not resolved before settlement")
        if name == "ui_nested":
            require(starts[0]["index"] < index(case["trace"], "nested_child_start") < index(case["trace"], "nested_child_end") < ends[0]["index"], "nested prompt was not inside outer prompt")
    else:
        require(not named(trace, "ui_prompt_start"), f"{name}: fabricated input request")
    requests = case.get("provider_requests", [])
    require(requests and all(r["model"] == name for r in requests), f"{name}: no actual model requests")
    expected_status = {"provider_error": [400], "retry_success": [503, 200], "retry_exhausted": [503, 503]}.get(name)
    if expected_status:
        require([r["status"] for r in requests] == expected_status, f"{name}: wrong provider fixture requests")


def validate_report(report, check_hashes=True):
    require(report.get("schema_version") == 1 and report.get("complete") is True, "incomplete report")
    runs = report.get("runs", [])
    require(len(runs) == 4 and {(r.get("runtime"), r.get("order")) for r in runs} == RUNTIMES, "full 2-runtime x 2-order matrix required")
    hashes = report.get("fixture_hashes", {})
    require(set(hashes) == {"verify.py", "provider.py", "observer.ts", "fixture.ts", "trace.mjs"}, "missing fixture provenance")
    for name, expected in hashes.items():
        require(isinstance(expected, str) and re.fullmatch(r"[a-f0-9]{64}", expected), "invalid fixture hash")
        if check_hashes:
            require(hashlib.sha256((HERE / name).read_bytes()).hexdigest() == expected, f"stale fixture hash: {name}")
    for run in runs:
        require(run.get("version") == "1.2.0" and re.fullmatch(r"[a-f0-9]{64}", run.get("executable_sha256", "")), "unqualified executable")
        require(not run.get("failure") and not run.get("partial_trace"), "partial runtime run")
        cleanup = run.get("cleanup", {})
        require(set(cleanup) == {"pi_process_group_empty", "provider_stopped", "private_directory_removed"}
            and all(v is True for v in cleanup.values()), "private cleanup failed")
        launch = run.get("private_launch", {})
        require(all(launch.get(k) is True for k in ("no_inherited_credentials", "isolated_agent_directory", "project_resources_disabled", "controlling_pty", "provider_loopback_only")), "launch not isolated")
        allowed_env = {"PATH", "TERM", "LANG", "SHELL", "HOME", "TMPDIR", "PI_CODING_AGENT_DIR", "PI_OFFLINE", "PI_SKIP_VERSION_CHECK", "PI_TELEMETRY",
            "VJ_PI_TRACE", "VJ_PI_SESSION_CONTROL", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"}
        require(set(launch.get("environment_allowlist", [])) == allowed_env, "credential or user configuration inheritance")
        cases = run.get("cases", [])
        require([case.get("case") for case in cases] == CASE_ORDER, "missing or reordered P0 cases")
        pids, engines, previous_time = set(), set(), -1
        for case in cases:
            trace = case.get("trace", [])
            validate_trace(trace)
            require(case.get("event_counts") == dict(Counter(e["event"] for e in observer(case))), "fabricated event counts")
            for event in trace:
                require(event["at_ms"] >= previous_time, "nonmonotonic process event order")
                previous_time = event["at_ms"]
                pids.add(event["pid"])
                engines.add(event["engine"])
            for request in case.get("provider_requests", []):
                require(set(request) == {"model", "attempt", "status", "summary"}, "unapproved provider metadata")
                require(request["model"] in {*RUN_CASES, "manual_compact", "compact_error"} and type(request["attempt"]) is int
                    and request["attempt"] > 0 and type(request["summary"]) is bool and request["status"] in (200, 400, 503), "invalid provider metadata")
            if case["case"] in RUN_CASES:
                validate_run_case(case, run["order"])
        require(len(pids) == 1 and 0 not in pids, "session replacements changed Pi process")
        require(engines == {"bun" if run["runtime"] == "compiled" else "node"}, "wrong actual runtime engine")
        by_case = {case["case"]: case for case in cases}
        startup = by_case["startup"]["trace"]
        require((index(startup, "observer_loaded") < index(startup, "fixture_loaded")) is (run["order"] == "observer-first"), "claimed extension order not exercised")
        initial_session = single(observer(by_case["startup"]), "session_start")["session"]
        for name, end_event in (("manual_compact", "session_compact"), ("compact_error", "session_compact_failed")):
            trace = observer(by_case[name])
            start, end = single(trace, "session_before_compact"), single(trace, end_event)
            require(start.get("reason") == end.get("reason") == "manual" and start["index"] < end["index"], "manual compaction pairing failed")
            require(not named(trace, "agent_settled") and not named(trace, "agent_start"), "manual compaction invented agent completion")
            require(by_case[name].get("provider_requests"), "manual compaction did not use provider")
        idle = observer(by_case["idle_ui"])
        start, end = single(idle, "ui_prompt_start"), single(idle, "ui_prompt_end")
        require(start.get("idle") is True and start.get("kind") == "confirm" and start["index"] < end["index"], "idle blocking UI not exercised")
        require(not named(idle, "agent_settled"), "idle UI closing fabricated completion")
        require(single(observer(by_case["name"]), "session_info_changed").get("name_present") is True, "name change not observed")
        tree = observer(by_case["tree"])
        require(index(tree, "session_before_tree") < index(tree, "session_tree") and not named(tree, "agent_settled"), "tree navigation invented completion")
        previous_session = initial_session
        for name in ("new", "resume", "fork", "reload"):
            trace = observer(by_case[name])
            shutdown, start = single(trace, "session_shutdown"), single(trace, "session_start")
            require(shutdown.get("reason") == start.get("reason") == name and shutdown["index"] < start["index"], "session replacement pairing failed")
            require(shutdown["runtime"] != start["runtime"] and shutdown["session"] == previous_session, "observer runtime not recreated")
            require((start["session"] == previous_session) is (name == "reload"), "conversation identity reset error")
            if name == "resume":
                require(start["session"] == initial_session, "resume did not restore original conversation")
            require(not named(trace, "agent_settled"), "session replacement replayed Done")
            previous_session = start["session"]
        require(single(observer(by_case["quit"]), "session_shutdown").get("reason") == "quit", "missing orderly process shutdown")
    serialized = json.dumps(report)
    require("PRIVATE_" not in serialized and "SENTINEL" not in serialized, "private fixture content leaked")
    return {"status": "PASS", "runtime_order_runs": len(runs), "scenario_runs": sum(len(r["cases"]) for r in runs),
        "trace_events": sum(len(c["trace"]) for r in runs for c in r["cases"]), "p0_contract_gate": True,
        "production_adapter_qualified": False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", nargs="?", type=Path, default=HERE / "results.json")
    args = parser.parse_args()
    print(json.dumps(validate_report(json.loads(args.report.read_text())), indent=2))


if __name__ == "__main__":
    main()
