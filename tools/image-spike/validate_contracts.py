#!/usr/bin/env python3
"""Validate Phase 5 Slice 0 contracts, fixtures, provenance, and limits."""

from __future__ import annotations

import hashlib
import json
import pathlib
import subprocess
import sys
from typing import Any

ROOT = pathlib.Path(__file__).resolve().parents[2]
ARTIFACT = ROOT / "docs/spikes/artifacts/0025-terminal-images"
CONTRACTS = ARTIFACT / "contracts.json"
SIXEL = ARTIFACT / "fixtures/sixel-v1.json"
KITTY = ARTIFACT / "fixtures/kitty-static-v1.json"
BUDGET = ARTIFACT / "budget-probe.json"
CLIENTS = ARTIFACT / "representative-clients.json"
CAPTURES = ARTIFACT / "foot-sixel-captures"
CAPTURE_SCRIPT = ROOT / "tools/image-spike/capture_foot_sixel.py"
STATE_PATCH = ROOT / "tools/image-spike/foot-sixel-state-dump.patch"
ORACLE_PATCHES = ROOT / "tools/foot-oracle/patches"
ORACLE_PROVENANCE = ROOT / "tools/foot-oracle/provenance.json"
ORACLE_BINARY = pathlib.Path("/tmp/splinterm-foot-oracle-build/foot")
FOOT = pathlib.Path.home() / "Playground/foot"
PINNED_FOOT = "3c5b584b0eafa772eb4376fb6eaf6643399e190e"
MAX_U32 = 2**32 - 1


class ContractError(RuntimeError):
    """One checked Phase 5 contract invariant failed."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ContractError(message)


def load(path: pathlib.Path, schema: str) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    require(isinstance(value, dict) and value.get("schema") == schema, f"{path}: schema")
    return value


def sha256(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def validate_provenance(contracts: dict[str, Any]) -> None:
    foot = contracts["foot"]
    revision = subprocess.run(
        ["git", "-C", str(FOOT), "rev-parse", "HEAD"],
        text=True,
        capture_output=True,
        check=True,
    ).stdout.strip()
    require(revision == PINNED_FOOT == foot["commit"], "Foot revision drift")
    clean = subprocess.run(
        ["git", "-C", str(FOOT), "diff", "--exit-code"], check=False
    )
    require(clean.returncode == 0 and foot["dirty"] is False, "Foot checkout dirty")
    for name, digest in foot["sources"].items():
        require(sha256(FOOT / name) == digest, f"Foot source hash drift: {name}")
    provenance_path = ROOT / foot["oracle_provenance"]
    require(provenance_path == ORACLE_PROVENANCE, "Foot provenance path drift")
    require(
        sha256(provenance_path) == foot["oracle_provenance_sha256"],
        "Foot provenance hash drift",
    )
    provenance = json.loads(provenance_path.read_text(encoding="utf-8"))
    require(provenance["schema"] == 3, "Foot provenance schema")
    require(provenance["reference"]["commit"] == PINNED_FOOT, "Foot provenance commit")
    if ORACLE_BINARY.exists():
        require(
            sha256(ORACLE_BINARY) == foot["oracle_binary_sha256"],
            "Foot oracle binary hash drift",
        )
    fcft_version = subprocess.run(
        ["pkg-config", "--modversion", "fcft"],
        text=True,
        capture_output=True,
        check=True,
    ).stdout.strip()
    require(fcft_version == provenance["build"]["fcft_version"], "fcft version drift")
    profile = provenance["default_final_buffer_profile"]
    require(sha256(pathlib.Path(profile["font_file"])) == profile["font_sha256"], "Foot oracle font drift")
    kitty = contracts["kitty"]
    require(
        sha256(pathlib.Path(kitty["local_document"]))
        == kitty["local_document_sha256"],
        "Kitty document hash drift",
    )


def validate_limits(contracts: dict[str, Any]) -> None:
    limits = contracts["limits"]
    exact = {
        "encoded_bytes_per_transmission": 8 * 1024 * 1024,
        "decoded_bytes_per_content": 16 * 1024 * 1024,
        "decoded_pixels_per_content": 4_194_304,
        "decoded_bytes_per_splint": 32 * 1024 * 1024,
        "decoded_bytes_per_daemon": 64 * 1024 * 1024,
        "contents_per_splint": 64,
        "placements_per_splint": 256,
        "inbound_kitty_uploads_per_pty": 1,
        "outbound_content_transfers_per_splint": 2,
        "outbound_content_transfers_per_daemon": 4,
        "encoded_bytes_in_flight_per_daemon": 16 * 1024 * 1024,
        "client_source_cache_bytes": 32 * 1024 * 1024,
        "client_scaled_cache_bytes": 32 * 1024 * 1024,
        "client_total_image_cache_bytes": 64 * 1024 * 1024,
        "kitty_control_bytes": 1024,
        "kitty_encoded_payload_bytes_per_chunk": 4096,
        "kitty_decoded_bytes_per_full_chunk": 3072,
        "kitty_unchunked_compatibility_bytes": 8 * 1024 * 1024,
        "reply_text_bytes": 512,
        "sixel_colors": 1024,
        "decoder_expansion_ratio": 64,
        "decoded_pixel_writes_per_command": 16_777_216,
        "token_bytes": 32,
        "token_ttl_milliseconds": 5000,
        "pending_tokens_per_peer": 4,
        "pending_tokens_per_daemon": 32,
        "unauthenticated_content_connections": 8,
        "content_socket_handshake_bytes": 512,
        "content_connection_deadline_milliseconds": 5000,
        "content_handshake_deadline_milliseconds": 5000,
    }
    for name, expected in exact.items():
        require(limits[name] == expected, f"limit drift: {name}")
    transport = contracts["transport"]
    require(transport["chunk_bytes"] == 64 * 1024, "content chunk cap")
    require(transport["receive_window_chunks"] == 4, "content receive window")
    require(transport["socket_mode"] == "0600", "content socket mode")
    require(limits["maximum_width"] <= 4096, "width exceeds accepted ceiling")
    require(limits["maximum_height"] <= 4096, "height exceeds accepted ceiling")
    require(
        limits["decoded_pixels_per_content"] * 4
        == limits["decoded_bytes_per_content"],
        "pixel and byte caps disagree",
    )
    require(
        limits["decoded_bytes_per_content"]
        <= limits["decoded_bytes_per_splint"]
        <= limits["decoded_bytes_per_daemon"],
        "decoded hierarchy is not monotonic",
    )
    require(
        limits["client_source_cache_bytes"] + limits["client_scaled_cache_bytes"]
        == limits["client_total_image_cache_bytes"],
        "client cache total disagrees",
    )
    require(limits["kitty_encoded_payload_bytes_per_chunk"] == 4096, "Kitty chunk cap")
    require(limits["kitty_decoded_bytes_per_full_chunk"] == 3072, "Kitty decoded cap")
    require(limits["inbound_kitty_uploads_per_pty"] == 1, "Kitty upload interleaving")
    require(limits["token_bytes"] >= 32, "transfer token entropy")
    require(0 < limits["token_ttl_milliseconds"] <= 5000, "token TTL")
    baseline = contracts["no_image_baseline"]
    expected = int(baseline["rss_median_bytes"] * 0.05)
    require(baseline["allowed_rss_growth_bytes"] == expected, "RSS allowance arithmetic")


def validate_sixel(value: dict[str, Any]) -> None:
    seen: set[str] = set()
    for case in value["cases"]:
        case_id = case["id"]
        require(case_id not in seen, f"duplicate Sixel case: {case_id}")
        seen.add(case_id)
        raw = bytes.fromhex(case["input_hex"])
        require(raw.startswith(b"\x1bP") and raw.endswith(b"\x1b\\"), f"{case_id}: framing")
        expected = case["expected"]
        width, height = expected["width"], expected["height"]
        require(0 < width <= 4096 and 0 < height <= 4096, f"{case_id}: dimensions")
        rows = expected["rows"]
        require(len(rows) == height, f"{case_id}: row count")
        for row in rows:
            require(sum(run[0] for run in row) == width, f"{case_id}: row width")
            for count, pixel in row:
                require(count > 0 and len(bytes.fromhex(pixel)) == 4, f"{case_id}: pixel run")


def expand_bgra(case: dict[str, Any]) -> bytes:
    output = bytearray()
    for row in case["expected"]["rows"]:
        for count, pixel in row:
            output.extend(bytes.fromhex(pixel) * count)
    return bytes(output)


def validate_foot_captures(sixel: dict[str, Any], contracts: dict[str, Any]) -> None:
    expected_ids = {case["id"] for case in sixel["cases"]}
    report_paths = sorted(CAPTURES.glob("*/report.json"))
    require({path.parent.name for path in report_paths} == expected_ids, "Foot capture set")
    oracle_hashes = {
        patch.name: sha256(patch) for patch in sorted(ORACLE_PATCHES.glob("*.patch"))
    }
    foot_contract = contracts["foot"]
    provenance = json.loads(ORACLE_PROVENANCE.read_text(encoding="utf-8"))
    profile = provenance["default_final_buffer_profile"]
    expected_isolation = {
        "cleanup_verified": True,
        "monitor": "DP-2",
        "no_initial_focus": True,
        "workspace": 8,
    }
    for case in sixel["cases"]:
        case_id = case["id"]
        directory = CAPTURES / case_id
        report = load(directory / "report.json", "splinterm.phase5.foot-sixel-capture.v1")
        metadata_path = directory / "foot.json"
        state_path = directory / "foot-sixel-state.json"
        pixels_path = directory / "foot.argb"
        metadata = load(metadata_path, "splinterm.final-buffer.v1")
        state = json.loads(state_path.read_text(encoding="utf-8"))
        require(report["case"] == case_id, f"{case_id}: report identity")
        require(
            report["exact"]
            and report["semantic_exact"]
            and report["viewport_origin_matches"],
            f"{case_id}: semantic/render mismatch",
        )
        require(report["foot_commit"] == PINNED_FOOT, f"{case_id}: Foot commit")
        require(
            report["foot_binary_sha256"] == foot_contract["oracle_binary_sha256"],
            f"{case_id}: Foot binary",
        )
        require(report["isolation"] == expected_isolation, f"{case_id}: isolation")
        require(report["capture_script_sha256"] == sha256(CAPTURE_SCRIPT), f"{case_id}: capture script")
        require(report["state_patch_sha256"] == sha256(STATE_PATCH), f"{case_id}: state patch")
        require(report["oracle_patch_sha256"] == oracle_hashes, f"{case_id}: oracle patches")
        require(report["source_argb_sha256"] == sha256(pixels_path), f"{case_id}: framebuffer hash")
        require(report["source_metadata_sha256"] == sha256(metadata_path), f"{case_id}: metadata hash")
        require(report["state_sha256"] == sha256(state_path), f"{case_id}: state hash")
        metadata_provenance = metadata["provenance"]
        require(
            metadata_provenance
            == {
                "implementation": "foot",
                "commit": PINNED_FOOT,
                "fcft_version": provenance["build"]["fcft_version"],
                "font_file": profile["font_file"],
                "font_index": profile["font_index"],
                "font_sha256": profile["font_sha256"],
            },
            f"{case_id}: metadata provenance",
        )
        require(metadata["format"] == "argb8888" and metadata["byte_order"] == "bgra", f"{case_id}: format")

        expected = case["expected"]
        expected_bgra = expand_bgra(case)
        raw = pixels_path.read_bytes()
        require(len(raw) == metadata["stride"] * metadata["height"], f"{case_id}: framebuffer length")
        origin_x, origin_y = metadata["origin"]["x"], metadata["origin"]["y"]
        observed = bytearray()
        for row in range(expected["height"]):
            start = (origin_y + row) * metadata["stride"] + origin_x * 4
            observed.extend(raw[start : start + expected["width"] * 4])
        require(bytes(observed) == expected_bgra, f"{case_id}: framebuffer pixels")

        sixels = state.get("sixels", [])
        require(sixels == report["state_sixels"] and len(sixels) == 1, f"{case_id}: semantic image count")
        image = sixels[0]
        expected_argb = "".join(
            f"{int.from_bytes(expected_bgra[index : index + 4], 'little'):08x}"
            for index in range(0, len(expected_bgra), 4)
        )
        require(
            image["width"] == expected["width"]
            and image["height"] == expected["height"]
            and image["opaque"] == expected["opaque"]
            and image["row"] == 0
            and image["column"] == 0
            and image["argb"] == expected_argb,
            f"{case_id}: semantic pixels",
        )


def controls(command: str) -> dict[str, str]:
    require(command.startswith("\x1b_G") and command.endswith("\x1b\\"), "Kitty framing")
    body = command[3:-2]
    control = body.split(";", 1)[0]
    result: dict[str, str] = {}
    for item in control.split(","):
        if not item:
            continue
        key, separator, value = item.partition("=")
        require(separator == "=" and len(key) == 1 and key not in result, "Kitty control")
        result[key] = value
    return result


def validate_budget(value: dict[str, Any], contracts: dict[str, Any]) -> None:
    records = value["records"]
    require(records["baseline"]["bytes"] == 0, "budget baseline allocation")
    limits = contracts["limits"]
    require(
        records["daemon_authoritative_full"]["bytes"]
        == limits["decoded_bytes_per_daemon"],
        "daemon budget probe size",
    )
    require(
        records["client_cache_full"]["bytes"]
        == limits["client_total_image_cache_bytes"],
        "client budget probe size",
    )
    for name in ("daemon_authoritative_full", "client_cache_full"):
        require(records[name]["rss_delta_bytes"] > records[name]["bytes"], f"{name}: pages not resident")
        require(records[name]["rss_delta_bytes"] < records[name]["bytes"] + 2 * 1024 * 1024, f"{name}: overhead")


def validate_clients(value: dict[str, Any]) -> None:
    require(sha256(pathlib.Path(value["input"]["path"])) == value["input"]["sha256"], "client trace input hash")
    seen: set[str] = set()
    for client in value["clients"]:
        require(client["name"] not in seen, f"duplicate representative client: {client['name']}")
        seen.add(client["name"])
        require(sha256(pathlib.Path(client["executable"])) == client["sha256"], f"{client['name']}: executable hash")
        require(client["output_bytes"] > 0 and len(bytes.fromhex(client["output_sha256"])) == 32, f"{client['name']}: output identity")


def validate_kitty(value: dict[str, Any]) -> None:
    seen: set[str] = set()
    for case in value["cases"]:
        case_id = case["id"]
        require(case_id not in seen, f"duplicate Kitty case: {case_id}")
        seen.add(case_id)
        commands = case.get("inputs", [case.get("input")])
        require(commands and all(isinstance(item, str) for item in commands), f"{case_id}: input")
        parsed = [controls(item) for item in commands]
        if len(parsed) > 1:
            require(parsed[0].get("m") == "1", f"{case_id}: first continuation")
            for item in parsed[1:-1]:
                require(set(item) <= {"m", "q"} and item.get("m") == "1", f"{case_id}: middle continuation")
            require(set(parsed[-1]) <= {"m", "q"} and parsed[-1].get("m") == "0", f"{case_id}: final continuation")
        for item in parsed:
            for key in ("i", "p"):
                if key in item:
                    number = int(item[key])
                    require(0 <= number <= MAX_U32, f"{case_id}: {key} range")
        expected = case.get("expected_reply")
        if expected is not None:
            require(expected.startswith("\x1b_G") and expected.endswith("\x1b\\"), f"{case_id}: reply")


def main() -> int:
    try:
        contracts = load(CONTRACTS, "splinterm.phase5.image-contracts.v1")
        sixel = load(SIXEL, "splinterm.phase5.sixel-fixtures.v1")
        kitty = load(KITTY, "splinterm.phase5.kitty-static-fixtures.v1")
        budget = load(BUDGET, "splinterm.phase5.image-budget-probe.v1")
        clients = load(CLIENTS, "splinterm.phase5.representative-image-clients.v1")
        validate_provenance(contracts)
        validate_limits(contracts)
        validate_sixel(sixel)
        validate_foot_captures(sixel, contracts)
        validate_kitty(kitty)
        validate_budget(budget, contracts)
        validate_clients(clients)
    except (ContractError, KeyError, OSError, ValueError, json.JSONDecodeError) as error:
        print(f"Phase 5 contract validation failed: {error}", file=sys.stderr)
        return 1
    print(
        f"Validated Phase 5 contracts: {len(sixel['cases'])} pinned-Foot Sixel fixtures, "
        f"{len(kitty['cases'])} Kitty fixtures, "
        f"{len(clients['clients'])} representative client traces"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
