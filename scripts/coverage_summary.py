#!/usr/bin/env python3
"""Print per-file line coverage and append the same table to GitHub's job summary."""
import json
import os
import sys


def main(path):
    files = json.load(open(path, encoding="utf-8"))["data"][0]["files"]
    rows = [(item["filename"], item["summary"]["lines"]["percent"]) for item in files]
    rows.sort()
    lines = ["### Line coverage", "", "| File | Coverage |", "| --- | ---: |"]
    lines += [f"| `{name}` | {pct:.2f}% |" for name, pct in rows]
    output = "\n".join(lines) + "\n"
    print(output, end="")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as handle:
            handle.write(output)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: coverage_summary.py cov.json")
    main(sys.argv[1])
