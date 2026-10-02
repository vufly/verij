#!/usr/bin/env python3
"""Project only literal first-run flags from a read-only Agy state file."""
import argparse
import json
from pathlib import Path
import re


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("state", type=Path)
    args = parser.parse_args()
    fields = []
    for line in args.state.read_text().splitlines():
        match = re.fullmatch(r"\s*([A-Za-z_][A-Za-z_0-9]*):\s*(true|false|[0-9]+|[A-Z][A-Z_0-9]*)\s*", line)
        if match and any(word in match[1].lower() for word in ("onboard", "consent", "telemetry", "survey", "privacy", "welcome", "theme")):
            fields.append({"field": match[1], "literal": match[2]})
    print(json.dumps({"read_only_literal_flags": fields}, indent=2))


if __name__ == "__main__":
    main()
