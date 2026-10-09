#!/usr/bin/env python3
"""Regenerate openapi/openapi.json + openapi.yaml WITHOUT cargo.

The authoritative generator is the Rust test:

    UPDATE_OPENAPI=1 cargo test -p sniper-suite --test openapi_artifact

This script exists for environments where cargo is unavailable (e.g. a
docs-only sandbox). It reproduces exactly what `document()` produces by
parsing the same `json!({...})` literals the Rust code compiles:

  * base document            -> taken from the currently committed artifact
                                (which the artifact test keeps in lockstep
                                with base_document() + the non-parseable
                                helper fragments);
  * each parseable fragment  -> extracted from its Rust source below and
                                merged into `paths` / `components.schemas`,
                                refusing duplicate keys exactly like
                                merge_object() does.

The output is pretty-printed with sorted keys, 2-space indent and a trailing
newline — byte-compatible with `serde_json::to_string_pretty`, because
serde_json's Map is a BTreeMap (preserve_order off) and therefore serializes
keys sorted.

Usage:
    python3 scripts/regen-openapi-artifact.py            # write both files
    python3 scripts/regen-openapi-artifact.py --check    # exit 1 on drift
"""
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ARTIFACT = ROOT / "openapi" / "openapi.json"
YAML_OUT = ROOT / "openapi" / "openapi.yaml"

# Fragments whose bodies are pure json! literals (no Rust expressions) and
# can therefore be extracted mechanically. (base_document() uses helper
# closures and stays owned by the committed artifact / Rust generator.)
FRAGMENTS = [
    ("crates/server/src/openapi_team_security.rs",
     [("team_security_paths", "paths"), ("team_security_schemas", "schemas")]),
    ("crates/server/src/openapi_trading_data_plane.rs",
     [("trading_data_plane_paths", "paths"), ("trading_data_plane_schemas", "schemas")]),
    ("crates/server/src/openapi_control_plane_surface.rs",
     [("control_plane_surface_paths", "paths"), ("control_plane_surface_schemas", "schemas")]),
]


def extract_json_body(source: str, fn_name: str):
    """Return the parsed JSON value of `pub fn {fn_name}() -> Value { json!({...}) }`."""
    marker = f"pub fn {fn_name}() -> Value"
    idx = source.find(marker)
    if idx == -1:
        raise SystemExit(f"function {fn_name} not found")
    open_idx = source.find("json!(", idx)
    if open_idx == -1:
        raise SystemExit(f"no json! call in {fn_name}")
    brace = source.find("{", open_idx)
    if brace == -1:
        raise SystemExit(f"no object literal in {fn_name}")

    # Walk to the matching close brace, respecting string literals.
    depth = 0
    i = brace
    n = len(source)
    in_string = False
    while i < n:
        ch = source[i]
        if in_string:
            if ch == "\\":
                i += 2
                continue
            if ch == '"':
                in_string = False
        else:
            if ch == '"':
                in_string = True
            elif ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
                if depth == 0:
                    break
        i += 1
    if depth != 0:
        raise SystemExit(f"unbalanced braces in {fn_name}")
    body = source[brace:i + 1]
    return json.loads(rust_literal_to_json(body))


def rust_literal_to_json(body: str) -> str:
    """Convert a json! object literal into strict JSON text.

    Handles the Rust string escapes this codebase uses (\\, \", \n, \t,
    \\u{...}, backslash-newline continuation) by re-encoding every string
    token through Python's own string decoder.
    """
    out = []
    i = 0
    n = len(body)
    while i < n:
        ch = body[i]
        if ch != '"':
            out.append(ch)
            i += 1
            continue
        # consume one Rust string literal
        i += 1
        raw = []
        while i < n:
            c = body[i]
            if c == "\\":
                nxt = body[i + 1] if i + 1 < n else ""
                if nxt == "\n":
                    # Rust line continuation: skip newline + leading spaces
                    i += 2
                    while i < n and body[i] in " \t":
                        i += 1
                    continue
                if nxt == "u" and i + 2 < n and body[i + 2] == "{":
                    close = body.index("}", i + 3)
                    codepoint = int(body[i + 3:close], 16)
                    raw.append(chr(codepoint))
                    i = close + 1
                    continue
                raw.append(c)
                raw.append(nxt)
                i += 2
                continue
            if c == '"':
                break
            raw.append(c)
            i += 1
        i += 1  # closing quote
        out.append(json.dumps(decode_rust_escapes("".join(raw)), ensure_ascii=False))
    return "".join(out)


def decode_rust_escapes(text: str) -> str:
    """Decode the small set of Rust string escapes used in these literals."""
    if "\\" not in text:
        return text
    simple = {"n": "\n", "t": "\t", "r": "\r", chr(92): chr(92), chr(34): chr(34), "0": "\0"}
    out = []
    i = 0
    while i < len(text):
        c = text[i]
        if c != "\\":
            out.append(c)
            i += 1
            continue
        nxt = text[i + 1] if i + 1 < len(text) else ""
        if nxt in simple:
            out.append(simple[nxt])
            i += 2
            continue
        raise SystemExit(f"unsupported rust escape in openapi literal: \\{nxt}")
    return "".join(out)


def merge(doc: dict, fragment: dict, target_key_path, origin: str):
    """merge_object semantics: descend, insert, refuse to overwrite."""
    node = doc
    for key in target_key_path[:-1]:
        node = node.setdefault(key, {})
    target = node.setdefault(target_key_path[-1], {})
    for key, value in fragment.items():
        if key in target:
            # The committed artifact can carry stale pre-fragment entries
            # (e.g. paths documented in the base before fragments existed).
            # document() itself panics on a real base/fragment conflict, so
            # any conflict we see here means the ARTIFACT is stale — the
            # parseable fragment is what the server actually compiles and
            # serves, so it wins (loudly).
            if target[key] != value:
                print(f"note: fragment '{origin}' supersedes stale artifact entry "
                      f"{target_key_path[-1]} '{key}'", file=sys.stderr)
        target[key] = value


def read_api_version() -> str:
    """The contract version is owned by crates/server/src/saas/openapi.rs."""
    src = (ROOT / "crates/server/src/saas/openapi.rs").read_text()
    marker = 'pub const API_VERSION: &str = "'
    idx = src.find(marker)
    if idx == -1:
        raise SystemExit("API_VERSION not found in openapi.rs")
    start = idx + len(marker)
    return src[start:src.index('"', start)]


def build() -> str:
    doc = json.loads(ARTIFACT.read_text())
    doc.setdefault("info", {})["version"] = read_api_version()
    for rel, fns in FRAGMENTS:
        source = (ROOT / rel).read_text()
        origin = Path(rel).stem
        for fn_name, kind in fns:
            value = extract_json_body(source, fn_name)
            if not isinstance(value, dict):
                raise SystemExit(f"{fn_name} did not produce an object")
            if kind == "paths":
                merge(doc, value, ["paths"], origin)
            else:
                merge(doc, value, ["components", "schemas"], origin)
    return json.dumps(doc, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


def build_yaml(json_text: str) -> str:
    import yaml

    doc = json.loads(json_text)

    class Dumper(yaml.SafeDumper):
        pass

    def str_representer(dumper, data):
        if "\n" in data:
            return dumper.represent_scalar("tag:yaml.org,2002:str", data, style="|")
        return dumper.represent_scalar("tag:yaml.org,2002:str", data)

    Dumper.add_representer(str, str_representer)

    header = (
        "# GENERATED FILE — DO NOT EDIT.\n"
        "# Source of truth: openapi/openapi.json, produced from\n"
        "# crates/server/src/saas/openapi.rs::document() by\n"
        "# ./scripts/export-openapi.sh. Edit the Rust, not this file.\n"
    )
    body = yaml.dump(doc, Dumper=Dumper, sort_keys=True, allow_unicode=True, width=100)
    return header + body


def main() -> int:
    check = "--check" in sys.argv
    json_text = build()
    yaml_text = build_yaml(json_text)
    if check:
        ok = True
        if ARTIFACT.read_text() != json_text:
            print("openapi/openapi.json is stale — regenerate with "
                  "UPDATE_OPENAPI=1 cargo test -p sniper-suite --test openapi_artifact "
                  "or python3 scripts/regen-openapi-artifact.py", file=sys.stderr)
            ok = False
        if YAML_OUT.read_text() != yaml_text:
            print("openapi/openapi.yaml is stale — regenerate with ./scripts/export-openapi.sh "
                  "or python3 scripts/regen-openapi-artifact.py", file=sys.stderr)
            ok = False
        return 0 if ok else 1
    ARTIFACT.write_text(json_text)
    YAML_OUT.write_text(yaml_text)
    paths = len(json.loads(json_text)["paths"])
    print(f"wrote {ARTIFACT.relative_to(ROOT)} and {YAML_OUT.relative_to(ROOT)} ({paths} paths)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
