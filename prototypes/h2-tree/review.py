#!/usr/bin/env python3
"""Operate only a manifest-qualified private H2 review fixture."""
import argparse
import importlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import time

sys.path.insert(0,str(Path(__file__).resolve().parents[1] / "h1-navigation"))
support=importlib.import_module("verify")


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation",choices=("query","focus","sidebar","fixture","prepare","clean"))
    parser.add_argument("--root",type=Path,required=True)
    parser.add_argument("--host",choices=("a","b"),default="a")
    parser.add_argument("--pane",type=int,choices=(0,1,2,3),default=0)
    parser.add_argument("--status",choices=("working","done","idle","needs_input","error","unknown"),default="working")
    args=parser.parse_args()
    root=args.root.resolve(strict=True)
    manifest=json.loads((root / "review-manifest.json").read_text())
    assert root.name.startswith("vj-h2-") and manifest["root"]==str(root)
    sockets=Path(manifest["socket_dir"])
    assert sockets.name.startswith("vj-h2-sock-") and manifest["tmux_socket"]==str(sockets / "terminal")
    if not sockets.exists():raise RuntimeError("this review instance was cleaned")
    env=support.environment(root,sockets)
    env["XDG_RUNTIME_DIR"]=str(root / "runtime")
    env["ZELLIJ_CONFIG_FILE"]=str(root / "inner.kdl")
    def run(argv,data=None):return support.run(argv,env,root,timeout=30,data=data)
    if args.operation=="sidebar":
        session=f"h2-host-{args.host}"
        panes=json.loads(run([manifest["binary"],"-s",session,"action","list-panes","--all","--json"]))
        pane=next(value for value in panes if not value["is_plugin"] and value["id"]==0)
        for suffix in ("M","m"):
            wire=f"\x1b[<0;{pane['pane_content_x']+11};{pane['pane_content_y']+16}{suffix}".encode()
            run(["tmux","-f","/dev/null","-S",manifest["tmux_socket"],"send-keys","-t",f"host-{args.host}","-H",*[f"{byte:02x}" for byte in wire]])
            time.sleep(.1)
        support.wait(lambda:"terminal_0" in run([manifest["binary"],"-s",session,"action","list-clients"]))
        print(json.dumps({"host":args.host,"sidebar_focused":True,"acknowledgement_requested":False}))
        return
    if args.operation=="prepare":
        script=str(Path(__file__).resolve())
        for pane,status in ((0,"done"),(2,"working")):
            run([sys.executable,script,"fixture","--root",root,"--pane",str(pane),"--status",status])
        if "review_unknown" not in manifest:
            receiver=Path(__file__).resolve().parents[1] / "h1-navigation/receiver.py"
            terminal=run([manifest["binary"],"-s",manifest["inner_session"],"action","new-pane","--",sys.executable,receiver,root,"h2-review-unknown"])
            assert terminal.startswith("terminal_")
            pane=int(terminal.removeprefix("terminal_"))
            def topology():
                inventory=json.loads(run([manifest["cli"],"inventory","--session",manifest["inner_session"]]))[0]["inventory"]
                return next((value for value in inventory["panes"] if value["terminal_id"]==pane and value.get("pane_process")),None)
            value=support.wait(topology)
            registered=json.loads(run([manifest["cli"],"agent","register","--session",manifest["inner_session"],"--pane",str(pane),"--pid",str(value["pane_process"]["pid"]),"--kind","opencode","--synthetic"]))
            instance=registered["agent_instance_id"]
            run([manifest["cli"],"agent","report","--instance",instance],json.dumps({
                "schema_version":1,"source_id":"fixture-review-unknown","source_epoch":1,"source_revision":1,
                "turn_epoch":1,"turn_revision":1,"turn_id":"fixture-review-unknown-1",
                "conversation_title":"Fixture-unknown","conversation_id":"fixture-review-unknown","activity":"initializing"}))
            manifest["review_unknown"]={"terminal":pane,"instance":instance,"process":value["pane_process"]}
            manifest["known_births"][str(value["pane_process"]["pid"])]=value["pane_process"]
        manifest["prepared_at_ms"]=time.time_ns()//1_000_000
        support.controller.atomic(root / "review-manifest.json",manifest)
        print(json.dumps({"prepared":str(root),"semantic_records_synthetic":True,"unknown":manifest["review_unknown"]},indent=2))
        return
    if args.operation=="clean":
        # Reject a replaced private server before invoking name-based teardown.
        for file in (root / "control").glob("focus-*.json"):
            pid=json.loads(file.read_text())["context"]["server_pid"]
            birth=support.controller.birth(pid)
            if birth is not None:
                assert all(birth.get(key)==value for key,value in manifest["known_births"][str(pid)].items() if key in birth)
        for session in manifest["sessions"]:
            run([manifest["binary"],"kill-session",session])
        subprocess.run(["tmux","-f","/dev/null","-S",manifest["tmux_socket"],"kill-server"],capture_output=True,timeout=8,check=True)
        shutil.rmtree(sockets)
        def gone():
            for pid,birth in manifest["known_births"].items():
                current=support.controller.birth(int(pid))
                if current is not None and all(current[key]==birth[key] for key in ("pid","boot_id","start_jiffies")):return False
            return True
        support.wait(gone,seconds=10)
        manifest["cleaned_at_ms"]=time.time_ns()//1_000_000
        support.controller.atomic(root / "review-manifest.json",manifest)
        print(json.dumps({"cleaned":str(root),"private_socket_removed":True,"known_processes_exited":True}))
        return
    if args.operation=="fixture":
        instance=manifest["instances"][str(args.pane)]
        state=json.loads(run([manifest["cli"],"agent","inspect","--instance",instance]))["state"]
        source=manifest["sources"][str(args.pane)]["source_id"]
        previous=state["sources"][source]
        turn=previous["turn_revision"]+1
        turn_id=f"fixture-review-{args.pane}-{turn}"
        record={"schema_version":1,"source_id":source,"source_epoch":previous["source_epoch"],
                "source_revision":previous["source_revision"]+1,"turn_epoch":previous["turn_epoch"],
                "turn_revision":turn,"turn_id":turn_id,"conversation_id":previous["conversation_id"],
                "conversation_title":previous["conversation_title"],"pending_requests":[],
                "activity":{"working":"working","unknown":"initializing"}.get(args.status,"idle")}
        if args.status=="done":record["successful_completion"]={"turn_id":turn_id,"turn_epoch":record["turn_epoch"],"turn_revision":turn,"completed_at_ms":time.time_ns()//1_000_000}
        if args.status=="needs_input":record.update(activity="working",pending_requests=[{"id":"fixture-review-permission","kind":"permission"}])
        if args.status=="error":record["execution_error"]={"turn_id":turn_id,"turn_epoch":record["turn_epoch"],"turn_revision":turn,"message":"synthetic review failure","is_terminal":True}
        result=json.loads(run([manifest["cli"],"agent","report","--instance",instance],json.dumps(record)))
        print(json.dumps({"semantic_records_synthetic":True,"pane":args.pane,"status":result["reduced_status"],"completion_revision":result["completion_revision"]},indent=2))
        return
    command=[manifest["cli"],"navigate",args.operation,"--host",args.host]
    if args.operation=="focus":command.extend(["--pane",str(args.pane)])
    print(run(command))


if __name__=="__main__":main()
