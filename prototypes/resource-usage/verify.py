#!/usr/bin/env python3
"""Measure Verij inventory cost with stock Zellij and private sockets/XDG roots.

Two read-only CLI pollers deliberately generate client-disconnect state updates.
Compare plugin builds with identical arguments. CPU is measured from /proc;
performance results are observations, not machine-independent pass thresholds.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import threading
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plugin", type=Path, required=True)
    parser.add_argument("--cli", type=Path, help="Measure real sidebars/visit protocol instead of list-panes pollers")
    parser.add_argument("--hosts", type=int, choices=(1, 2), default=1)
    parser.add_argument("--warmup", type=float, default=3)
    parser.add_argument("--monitoring", choices=("both", "enabled", "disabled"), default="both")
    parser.add_argument("--agent-runtime", choices=("synthetic", "opencode"), default="synthetic")
    parser.add_argument("--quiet", action="store_true", help="Print summary; retain full JSON in --output")
    parser.add_argument("--verify-p1-visits", action="store_true",
                        help="Check idle RPC budgets, sidebar/other-pane suppression and manual inner visits")
    parser.add_argument("--status-plugin", type=Path,
                        help="Local status-bar WASM to load once per tab")
    parser.add_argument("--tabs", type=int, default=1)
    parser.add_argument("--max-rss-mib", type=float, default=512,
                        help="Abort private experiment before server exceeds this RSS")
    parser.add_argument("--max-agent-rss-mib", type=float,
                        help="Optional separate OpenCode server/TUI RSS abort guard")
    parser.add_argument("--seconds", type=float, default=15)
    parser.add_argument("--pollers", type=int, default=2)
    parser.add_argument("--query-interval", type=float, default=0.02)
    parser.add_argument("--scratch-dir", type=Path, default=Path("/tmp/opencode"))
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.cli:
        from baseline import run_baseline
        return run_baseline(args)
    if (args.seconds <= 0 or args.pollers < 0 or args.query_interval < 0
            or args.tabs < 1 or args.max_rss_mib <= 0):
        parser.error("seconds, tabs and RSS limit must be positive; pollers and interval must be nonnegative")
    plugin = args.plugin.resolve(strict=True)
    status_plugin = args.status_plugin.resolve(strict=True) if args.status_plugin else None
    binary_path = shutil.which("zellij")
    if binary_path is None:
        parser.error("stock zellij is not installed")
    binary = Path(binary_path).resolve(strict=True)
    root = Path(tempfile.mkdtemp(prefix="verij-resource-", dir=args.scratch_dir))
    root.chmod(0o700)
    staged_status = root / "status.wasm"
    if status_plugin:
        shutil.copyfile(status_plugin, staged_status)
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("ZELLIJ", "TMUX", "VERIJ"))}
    for kind in ("CONFIG", "DATA", "CACHE", "STATE", "RUNTIME"):
        directory = root / kind.lower()
        directory.mkdir()
        directory.chmod(0o700)
        env["XDG_RUNTIME_DIR" if kind == "RUNTIME" else f"XDG_{kind}_HOME"] = str(directory)
    for name in ("states", "control", "sockets"):
        (root / name).mkdir()
    env.update(TERM="xterm-256color", ZELLIJ_SOCKET_DIR=str(root / "sockets"))
    url = "file:" + str(plugin)
    (root / "cache/zellij").mkdir()
    permissions = "".join(
        json.dumps(name) + " { ReadApplicationState; ChangeApplicationState; ReadCliPipes; }\n"
        for name in (url, str(plugin)))
    if status_plugin:
        permissions += "".join(
            json.dumps(name) + " { ReadApplicationState; ChangeApplicationState; RunCommands; }\n"
            for name in ("file:" + str(staged_status), str(staged_status)))
    (root / "cache/zellij/permissions.kdl").write_text(permissions)
    (root / "config.kdl").write_text(
        'default_shell "/bin/sh"\nshow_startup_tips false\nshow_release_notes false\n'
        'session_serialization false\n'
        f'load_plugins {{\n {json.dumps(url)} {{\n'
        ' state_dir "/host/states"\n control_dir "/host/control"\n }\n}\n')
    status_bar = (f'pane size=1 borderless=true {{\n plugin location={json.dumps("file:" + str(staged_status))} {{\n'
                  ' format_left "{mode} {session}"\n format_center "{tabs}"\n'
                  ' format_right "{datetime}"\n datetime_format "%H:%M:%S"\n }\n }\n'
                  if status_plugin else "")
    (root / "layout.kdl").write_text("layout {\n" + "".join(
        f'tab name="probe-{index}" {{ pane; {status_bar} }}\n'
        for index in range(args.tabs)) + "}\n")
    mux = ["tmux", "-f", "/dev/null", "-S", str(root / "terminal")]
    session = "resource-probe"
    report = {"status": "FAIL", "root": str(root), "plugin": str(plugin),
               "plugin_sha256": hashlib.sha256(plugin.read_bytes()).hexdigest(),
               "status_plugin": str(status_plugin) if status_plugin else None,
               "status_plugin_sha256": hashlib.sha256(status_plugin.read_bytes()).hexdigest() if status_plugin else None,
               "tabs": args.tabs, "max_rss_mib": args.max_rss_mib,
               "stock_binary": str(binary), "measurements": []}
    stop = threading.Event()
    threads = []
    queries = 0
    query_errors = []
    counter_lock = threading.Lock()
    pid = None

    def run(argv):
        return subprocess.check_output(list(map(str, argv)), cwd=root, env=env,
                                       text=True, stderr=subprocess.STDOUT, timeout=8)

    def inventory():
        return json.loads((root / "states/resource-probe.json").read_text())

    def wait(predicate):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            try:
                if predicate():
                    return
            except (OSError, ValueError):
                pass
            time.sleep(0.1)
        raise TimeoutError("private inventory did not converge")

    def sample():
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        return (time.monotonic(), int(fields[11]) + int(fields[12]),
                int(fields[21]) * os.sysconf("SC_PAGE_SIZE"))

    def measure(phase):
        first, count = sample(), queries
        deadline = first[0] + args.seconds
        peak = first[2]
        while time.monotonic() < deadline:
            time.sleep(min(0.25, max(0, deadline - time.monotonic())))
            current = sample()
            peak = max(peak, current[2])
            if peak > args.max_rss_mib * 2**20:
                raise RuntimeError(f"{phase}: private server RSS exceeded {args.max_rss_mib} MiB")
        last = sample()
        elapsed = last[0] - first[0]
        report["measurements"].append({
            "phase": phase, "elapsed_seconds": round(elapsed, 2),
            "cpu_percent": round((last[1] - first[1]) / os.sysconf("SC_CLK_TCK") / elapsed * 100, 2),
            "rss_mib": round(last[2] / 2**20, 2),
            "rss_delta_mib": round((last[2] - first[2]) / 2**20, 2),
            "peak_rss_mib": round(peak / 2**20, 2),
            "queries_per_second": round((queries - count) / elapsed, 2),
            "inventory_revision": inventory()["inventory"]["revision"],
        })

    def poll():
        nonlocal queries
        while not stop.is_set():
            started = time.monotonic()
            try:
                run([binary, "-s", session, "action", "list-panes", "--json"])
                with counter_lock:
                    queries += 1
            except (subprocess.SubprocessError, OSError) as error:
                with counter_lock:
                    if len(query_errors) < 32:
                        query_errors.append(str(error))
            stop.wait(max(0, args.query_interval - (time.monotonic() - started)))

    try:
        command = [binary, "--config", root / "config.kdl", "--layout", root / "layout.kdl",
                   "attach", "-c", session]
        run(mux + ["new-session", "-d", "-s", "probe", "-x", "120", "-y", "40",
                    "-c", str(root), shlex.join(map(str, command)) + " || sleep 12"])
        wait(lambda: inventory().get("inventory"))
        pid = inventory()["inventory"]["producer"]["server_pid"]
        pane = run([binary, "-s", session, "action", "new-pane"]).strip()
        wait(lambda: len(inventory()["inventory"]["panes"]) == args.tabs + 1)
        run([binary, "-s", session, "action", "close-pane", "--pane-id", pane])
        wait(lambda: len(inventory()["inventory"]["panes"]) == args.tabs)
        report["topology_refresh"] = "PASS"
        measure("idle_attached")
        for _ in range(args.pollers):
            thread = threading.Thread(target=poll)
            thread.start()
            threads.append(thread)
        measure("cli_query_storm")
        stop.set()
        for thread in threads:
            thread.join(timeout=10)
        # CLI detach can select its own transient client on stock 0.45.1.
        # Close only this private terminal to disconnect the actual client.
        run(mux + ["kill-session", "-t", "probe"])
        wait(lambda: inventory().get("connected_clients") == 0)
        measure("idle_detached")
        report.update(status="PASS", query_errors=query_errors)
    except Exception as error:
        report["error"] = str(error)
        try:
            report["terminal_on_failure"] = run(mux + ["capture-pane", "-p", "-t", "probe"])
        except (subprocess.SubprocessError, OSError):
            pass
    finally:
        stop.set()
        for thread in threads:
            thread.join(timeout=10)
        subprocess.run([str(binary), "kill-session", session], env=env, cwd=root,
                       capture_output=True, timeout=8)
        subprocess.run(mux + ["kill-server"], env=env, capture_output=True, timeout=8)
        deadline = time.monotonic() + 5
        while pid is not None and Path(f"/proc/{pid}").exists() and time.monotonic() < deadline:
            time.sleep(0.1)
        report["cleanup"] = {"server_exited": pid is None or not Path(f"/proc/{pid}").exists()}
        if not report["cleanup"]["server_exited"]:
            report.update(status="FAIL", error="private server did not exit")
        args.output.write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, indent=2))
    return 0 if report["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
