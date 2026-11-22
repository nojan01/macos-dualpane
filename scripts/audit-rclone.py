#!/usr/bin/env python3
"""Fail-closed, version-based OSV audit of the pinned rclone binary inventory.

This is not a reachability analysis. The source version of the locally built
main module is accepted only after verifying the complete binary's SHA256.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import sys
import urllib.request


def inventory(binary, manifest):
    data = binary.read_bytes()
    digest = hashlib.sha256(data).hexdigest()
    if digest not in manifest["binarySha256"].values():
        raise ValueError("Binary SHA256 does not match the committed security build")
    start = data.index(b"\xff Go buildinf:")
    if not data[start + 15] & 2:
        raise ValueError("Unsupported Go buildinfo format")

    def string_at(pos):
        size = shift = 0
        while shift < 64:
            byte = data[pos]
            pos += 1
            size |= (byte & 127) << shift
            if byte < 128:
                if pos + size > len(data):
                    raise ValueError("Truncated buildinfo")
                return data[pos:pos + size], pos + size
            shift += 7
        raise ValueError("Invalid buildinfo length")

    go, pos = string_at(start + 32)
    raw, _ = string_at(pos)
    modules = []
    for line in raw[16:-16].decode().splitlines():
        parts = line.split("\t")
        if parts[0] in ("mod", "dep"):
            modules.append({"name": parts[1], "version": parts[2]})
        elif parts[0] == "=>":
            modules[-1] = {"name": parts[1], "version": parts[2]}
    main = next(m for m in modules if m["name"] == "github.com/rclone/rclone")
    if main["version"] == "(devel)":
        main["version"] = manifest["sourceVersion"]
    if main["version"] != manifest["sourceVersion"] or go.decode() != manifest["goVersion"]:
        raise ValueError("Build inventory disagrees with manifest")
    modules.append({"name": "stdlib", "version": go.decode().removeprefix("go")})
    for module in modules:
        if not re.fullmatch(r"v?\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.+-]+)?", module["version"]):
            raise ValueError(f"Unqueryable module version: {module}")
    return digest, modules


def audit(modules):
    results = []
    for offset in range(0, len(modules), 100):
        chunk = modules[offset:offset + 100]
        payload = {"queries": [{"package": {"name": m["name"], "ecosystem": "Go"},
                                "version": m["version"]} for m in chunk]}
        request = urllib.request.Request(
            "https://api.osv.dev/v1/querybatch", data=json.dumps(payload).encode(),
            headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(request, timeout=45) as response:
            matches = json.load(response)["results"]
        if len(matches) != len(chunk):
            raise ValueError("Incomplete OSV response")
        if any(not isinstance(r, dict) or "error" in r for r in matches):
            raise ValueError("OSV returned a query error")
        results.extend({"module": m, "matches": r.get("vulns", [])}
                       for m, r in zip(chunk, matches))
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    manifest = json.loads((Path(__file__).parent / "rclone/build.json").read_text())
    digest, modules = inventory(args.binary, manifest)
    results = audit(modules)
    affected = [r for r in results if r["matches"]]
    report = {"binary": str(args.binary), "sha256": digest,
              "sourceVersion": manifest["sourceVersion"], "module_count": len(modules),
              "scope": "Version-based OSV inventory scan; no reachability proof",
              "results": results}
    if args.output:
        args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(f"rclone: {len(modules)} modules including Go; {len(affected)} affected modules")
    for result in affected:
        print(result["module"], [v["id"] for v in result["matches"]])
    return bool(affected)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        print(f"rclone audit failed: {error}", file=sys.stderr)
        sys.exit(2)
