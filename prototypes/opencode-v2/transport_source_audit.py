#!/usr/bin/env python3
"""Offline source-boundary audit of pinned 2.0.32; never executes upstream code."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat

from smoke import HERE, sha
from verify_evidence import keys, read_report, require

SOURCES = {
    "packages/plugin/src/tui/context.ts": "db984caa696e830185dfd99493e94acbda3aa4ced50b19c3aafdaeeb4db442fa",
    "packages/tui/src/plugin/api.tsx": "8e836be5f9178a6759dd36d9c68475a02e844ea039e6d1595d73f622bd63dbe5",
    "packages/tui/src/context/client.tsx": "714c7f71f6073d27a1cfd52a421f3940a476eb4749e343925507ab5d8c4a4ea6",
    "packages/tui/src/context/data.tsx": "e1722ee1982cbd85623756778a424ccd323852f29cc4667d63cf7aa00e47a6ad",
    "packages/client/src/promise/generated/client.ts": "9e4da23f8bcd598b45c6fe589ecaf434e3a8561bca9913cf1e4ec332d80aff36",
    "packages/cli/src/services/server-connection.ts": "9ab1863048e60f427d5e27daff8536fb888b8c261d2e89d9bfb937f60b785dd7",
    "packages/cli/src/services/standalone.ts": "dc8197da471d3d213acf51ef809bb1c05a7cf697bce1f0795e49fdd4c0165cf9",
}
CONTEXT_FIELDS = ["options", "location", "app", "renderer", "client", "data", "attention", "theme", "themeMode",
                  "markdown", "keymap", "storage", "ui"]
REQUEST_FIELDS = ["signal", "headers", "onActivity"]
FACTS = ["context_has_no_observation_factory", "plugin_receives_constructed_api", "sdk_transport_is_constructor_owned",
         "success_and_declared_error_parse_full_json", "standalone_uses_private_random_credential",
         "explicit_server_auth_resolves_outside_plugin", "host_can_replace_api_on_reconnect",
         "data_cache_uses_host_connection_not_plugin_fetch"]
BLOCKERS = ["bounded_transport", "terminal_identity", "fresh_complete_family"]
ARTIFACTS = ("transport_source_audit.py", "smoke.py", "verify_evidence.py", "pin.json")


def read_source(root, name, expected):
    path = root / name
    require(path.resolve() == path, "canonical source path required")
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        require(stat.S_ISREG(os.fstat(fd).st_mode), "regular source required")
        with os.fdopen(fd, "rb", closefd=False) as stream:
            raw = stream.read(128 * 1024 + 1)
    finally:
        os.close(fd)
    require(len(raw) <= 128 * 1024, "source byte bound")
    require(hashlib.sha256(raw).hexdigest() == expected, "source pin mismatch")
    return raw.decode("utf-8", errors="strict")


def characterize(sources):
    # Full source hashes are mandatory before these release-specific structural
    # checks. This is not a general TypeScript parser or proof about newer tags.
    context, plugin, host, data, sdk, connection, standalone = (sources[name] for name in SOURCES)
    context_block = context.split("export interface Context {", 1)[1]
    require(re.findall(r"^  readonly (\w+)[?:]", context_block, re.M) == CONTEXT_FIELDS, "context surface changed")
    request_block = sdk.split("export interface RequestOptions {", 1)[1].split("\n}", 1)[0]
    require(re.findall(r"^  readonly (\w+)[?:]", request_block, re.M) == REQUEST_FIELDS, "request surface changed")
    snippets = (
        (plugin, "client: host.client.api,"),
        (sdk, "const fetch = options.fetch ?? globalThis.fetch"),
        (sdk, "return await fetch(prepared.url, prepared.init)"),
        (sdk, "throw declared((await json(response)) as DeclaredErrorBody)"),
        (sdk, "return (await json(response)) as A"),
        (sdk, "text = await response.text()"),
        (sdk, "return JSON.parse(text)"),
        (standalone, 'const password = randomBytes(32).toString("base64url")'),
        (standalone, "env: { OPENCODE_PASSWORD: password }"),
        (connection, "const password = yield* Env.password"),
        (connection, "baseUrl: endpoint.url, headers: Service.headers(endpoint)"),
        (host, "api = next.api"),
        (host, "get api() {\n        return api"),
        (data, "api: () => client.api"),
        (data, "connection: client.connection"),
    )
    require(all(snippet in source for source, snippet in snippets), "source boundary changed")
    return [{"name": name, "observed": True} for name in FACTS]


def audit(root):
    require(root.is_absolute() and root.resolve() == root and root.is_dir(), "canonical source directory required")
    sources = {name: read_source(root, name, digest) for name, digest in SOURCES.items()}
    report = {"schema": 1, "target": read_report(HERE / "pin.json"), "evidence_kind": "pinned-source-boundary-audit",
              "status": "SOURCE_AUDIT_PASS", "p0_status": "BLOCKED", "production_admission": False,
              "connected_transport_qualified": False, "sources": dict(SOURCES),
              "artifact_hashes": {name: sha(HERE / name) for name in ARTIFACTS},
              "context_fields": CONTEXT_FIELDS, "request_fields": REQUEST_FIELDS,
              "facts": characterize(sources), "remaining_blockers": BLOCKERS}
    validate(report)
    return report


def validate(report):
    keys(report, "schema target evidence_kind status p0_status production_admission connected_transport_qualified sources artifact_hashes context_fields request_fields facts remaining_blockers")
    require(type(report["schema"]) is int and report["schema"] == 1, "schema")
    require(report["target"] == read_report(HERE / "pin.json"), "target pin")
    require(report["evidence_kind"] == "pinned-source-boundary-audit" and report["status"] == "SOURCE_AUDIT_PASS", "audit scope")
    require(report["p0_status"] == "BLOCKED" and report["production_admission"] is False
            and report["connected_transport_qualified"] is False, "source audit cannot admit P0")
    keys(report["sources"], " ".join(SOURCES))
    require(report["sources"] == SOURCES, "source pins")
    keys(report["artifact_hashes"], " ".join(ARTIFACTS))
    require(all(report["artifact_hashes"][name] == sha(HERE / name) for name in ARTIFACTS), "stale artifacts")
    require(report["context_fields"] == CONTEXT_FIELDS and report["request_fields"] == REQUEST_FIELDS, "surface facts")
    require(type(report["facts"]) is list and len(report["facts"]) == len(FACTS), "fact matrix")
    for item, name in zip(report["facts"], FACTS):
        keys(item, "name observed")
        require(item["name"] == name and item["observed"] is True, "source fact")
    require(report["remaining_blockers"] == BLOCKERS, "unresolved gates")
    return {"status": "SOURCE_AUDIT_PASS", "sources": len(SOURCES), "facts": len(FACTS), "p0_status": "BLOCKED",
            "production_admission": False, "connected_transport_qualified": False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--source-dir", type=Path)
    group.add_argument("--verify", type=Path)
    args = parser.parse_args()
    if args.verify:
        print(json.dumps(validate(read_report(args.verify)), indent=2))
    else:
        print(json.dumps(audit(args.source_dir), indent=2))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, IndexError):
        raise SystemExit("Source audit failed; P0 remains blocked") from None
