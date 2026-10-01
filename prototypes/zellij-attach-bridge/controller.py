#!/usr/bin/env python3
"""Experimental one-shot controller with generation-qualified restart recovery.

All cooperating producers share the same journal directory. An advisory lock
serializes query, durable reservation, send and result; a lost result never
causes replay. This is a feasibility helper, not the production Verij controller.
"""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import tempfile
import time
import uuid

from verify import completion_control, process_identity

MAX_SEQUENCE = 2**64 - 1


def durable_write(path, value):
    descriptor, temporary = tempfile.mkstemp(prefix=path.name + ".", dir=path.parent)
    try:
        with os.fdopen(descriptor, "w") as output:
            json.dump(value, output)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def probe_barrier(directory, phase, value):
    """Explicit disposable crash-probe hook; unrelated to normal controller use."""
    if directory is None:
        return
    hold = directory / f"{phase}.hold"
    if hold.exists():
        durable_write(directory / f"{phase}.json", value)
        deadline = time.monotonic() + 15
        while hold.exists():
            if time.monotonic() >= deadline:
                raise TimeoutError(f"controller probe barrier {phase} expired")
            time.sleep(.01)


def navigate(socket_path, record, server_birth, client_birth, journal_dir, pane_id, probe_dir=None):
    if (not record.get("attached") or not client_birth
            or process_identity(record["client_pid"]) != client_birth):
        raise RuntimeError("display process is no longer live")
    if not server_birth or server_birth != process_identity(record["server_pid"]):
        raise RuntimeError("server process birth identity changed")
    binding = {"server_birth": server_birth, "client_id": record["client_id"],
               "connection_id": record["connection_id"]}
    key = hashlib.sha256(json.dumps(binding, sort_keys=True).encode()).hexdigest()
    journal_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    journal = journal_dir / f"{key}.json"
    # Never replace the lock inode; kernel releases ownership on process death.
    with (journal_dir / f"{key}.lock").open("a") as lock:
        os.chmod(lock.name, 0o600)
        deadline = time.monotonic() + 5
        while True:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    raise TimeoutError("controller journal lock deadline expired")
                time.sleep(.01)
        # A query is read-only and does not reserve a sequence or focus a pane.
        snapshot = completion_control(socket_path, record, pane_id, query_only=True)
        if snapshot["status"] not in ("focused", "not_focused", "pane_missing"):
            raise RuntimeError(f"cannot recover attachment sequence: {snapshot['status']}")
        latest = snapshot["latest_sequence"]
        if type(latest) is not int or not 0 <= latest <= MAX_SEQUENCE:
            raise RuntimeError("server did not supply a valid reservation watermark")
        try:
            saved = json.loads(journal.read_text())
        except FileNotFoundError:
            saved = {"binding": binding, "reserved_sequence": 0}
        # Corruption is not absence. Refuse to replace an unreadable journal.
        reserved = saved.get("reserved_sequence")
        if (saved.get("binding") != binding or type(reserved) is not int
                or not 0 <= reserved <= MAX_SEQUENCE):
            raise ValueError("invalid controller sequence journal")
        high_water = max(latest, reserved)
        if high_water == MAX_SEQUENCE:
            raise OverflowError("attachment navigation sequence exhausted")
        sequence = high_water + 1
        request_id = uuid.uuid4().hex
        durable_write(journal, {"binding": binding, "reserved_sequence": sequence})
        probe_barrier(probe_dir, "reserved", {"sequence": sequence, "journal": str(journal), "request_id": request_id})
        result = completion_control(socket_path, record, pane_id, sequence=sequence, request_id=request_id)
        probe_barrier(probe_dir, "completed", result)
        # Even failure/timeout consumes this reservation; never retry its sequence.
        return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socket", required=True, type=Path)
    parser.add_argument("--binding", required=True, type=Path,
                        help="Verified attachment record plus captured server_birth/client_birth")
    parser.add_argument("--journal-dir", required=True, type=Path)
    parser.add_argument("--pane", required=True, type=int)
    parser.add_argument("--probe-dir", type=Path, help="Disposable crash barriers only")
    args = parser.parse_args()
    if not 0 <= args.pane < 2**32:
        parser.error("--pane must fit uint32")
    binding = json.loads(args.binding.read_text())
    result = navigate(args.socket, binding["record"], binding["server_birth"],
                      binding["client_birth"], args.journal_dir, args.pane, args.probe_dir)
    print(json.dumps(result), flush=True)
    return 0 if result["status"] == "focused" else 1


if __name__ == "__main__":
    raise SystemExit(main())
