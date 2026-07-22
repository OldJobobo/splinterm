#!/usr/bin/env python3
"""Validate the checked-in automation and MCP public-contract fixtures."""

from __future__ import annotations

import hashlib
import json
import sys
from collections.abc import Iterator
from pathlib import Path
from typing import Any

from jsonschema import Draft202012Validator
from jsonschema.exceptions import SchemaError
from referencing import Registry, Resource
from referencing.exceptions import Unresolvable

ROOT = Path(__file__).resolve().parents[2]
AUTOMATION_SCHEMA_DIR = ROOT / "dist" / "schemas" / "v1"
AUTOMATION_FIXTURE_DIR = ROOT / "tests" / "automation" / "fixtures"
MCP_SCHEMA_DIR = ROOT / "dist" / "schemas" / "mcp" / "v1"
MCP_FIXTURE_DIR = ROOT / "tests" / "mcp" / "fixtures"
MAX_JSON_BYTES = 1024 * 1024
EXPECTED_AUTOMATION_SCHEMAS = {
    "audit-record.schema.json",
    "cli-envelope.schema.json",
    "cli-event.schema.json",
    "policy.schema.json",
}
MCP_TOOLS = (
    "splinterm.ping",
    "splinterm.list_dojos",
    "splinterm.inspect_topology",
    "splinterm.inspect_splint",
    "splinterm.read_terminal",
    "splinterm.read_scrollback",
    "splinterm.search_scrollback",
    "splinterm.request_access",
    "splinterm.authorization_status",
    "splinterm.revoke_access",
    "splinterm.inspect_audit",
    "splinterm.create_dojo",
    "splinterm.split_splint",
    "splinterm.new_window",
    "splinterm.relaunch_splint",
    "splinterm.restore_splint",
    "splinterm.restore_window",
    "splinterm.restore_dojo",
    "splinterm.close_splint",
    "splinterm.close_window",
    "splinterm.kill_splint",
    "splinterm.set_split_ratio",
    "splinterm.rename_dojo",
    "splinterm.rename_window",
    "splinterm.rename_splint",
    "splinterm.set_window_default_focus",
    "splinterm.acquire_control",
    "splinterm.request_control_transfer",
    "splinterm.decide_control_transfer",
    "splinterm.release_control",
    "splinterm.input",
    "splinterm.resize",
)
MCP_RESOURCES = ("topology", "terminal", "control")
MCP_ERROR_CODES = (
    "authentication_failed",
    "handshake_required",
    "incompatible_version",
    "invalid_request",
    "unsupported_schema",
    "consent_unavailable",
    "consent_denied",
    "unauthorized",
    "confirmation_required",
    "controller_unavailable",
    "control_transfer_unavailable",
    "stale_topology",
    "not_found",
    "stale_incarnation",
    "invalid_argument",
    "resource_limit",
    "cancelled",
    "timeout",
    "internal",
)
MCP_MUTATION_TOOLS = {
    "splinterm.request_access",
    "splinterm.revoke_access",
    "splinterm.create_dojo",
    "splinterm.split_splint",
    "splinterm.new_window",
    "splinterm.relaunch_splint",
    "splinterm.restore_splint",
    "splinterm.restore_window",
    "splinterm.restore_dojo",
    "splinterm.close_splint",
    "splinterm.close_window",
    "splinterm.kill_splint",
    "splinterm.set_split_ratio",
    "splinterm.rename_dojo",
    "splinterm.rename_window",
    "splinterm.rename_splint",
    "splinterm.set_window_default_focus",
    "splinterm.acquire_control",
    "splinterm.request_control_transfer",
    "splinterm.decide_control_transfer",
    "splinterm.release_control",
    "splinterm.input",
    "splinterm.resize",
}
MCP_CONFIRMED_TOOLS = {
    "splinterm.revoke_access",
    "splinterm.close_splint",
    "splinterm.close_window",
    "splinterm.kill_splint",
}
MCP_UNTRUSTED_OUTPUT_TOOLS = {
    "splinterm.list_dojos",
    "splinterm.inspect_topology",
    "splinterm.inspect_splint",
    "splinterm.read_terminal",
    "splinterm.read_scrollback",
    "splinterm.search_scrollback",
}
MCP_TERMINAL_OUTPUT_TOOLS = {
    "splinterm.read_terminal",
    "splinterm.read_scrollback",
    "splinterm.search_scrollback",
}
MCP_OUTPUT_RESOURCE_DEFS = {
    "splinterm.ping": "daemon_resource",
    "splinterm.list_dojos": "topology_resource",
    "splinterm.inspect_topology": "topology_resource",
    "splinterm.inspect_splint": "logical_splint_resource",
    "splinterm.read_terminal": "terminal_resource",
    "splinterm.read_scrollback": "terminal_resource",
    "splinterm.search_scrollback": "terminal_resource",
    "splinterm.request_access": "authorization_resource",
    "splinterm.authorization_status": "splint_resource",
    "splinterm.revoke_access": "authorization_resource",
    "splinterm.inspect_audit": "audit_resource",
    "splinterm.create_dojo": "dojo_resource",
    "splinterm.split_splint": "splint_resource",
    "splinterm.new_window": "window_resource",
    "splinterm.relaunch_splint": "splint_resource",
    "splinterm.restore_splint": "splint_resource",
    "splinterm.restore_window": "window_resource",
    "splinterm.restore_dojo": "dojo_resource",
    "splinterm.close_splint": "splint_resource",
    "splinterm.close_window": "window_resource",
    "splinterm.kill_splint": "splint_resource",
    "splinterm.set_split_ratio": "splint_resource",
    "splinterm.rename_dojo": "dojo_resource",
    "splinterm.rename_window": "window_resource",
    "splinterm.rename_splint": "splint_resource",
    "splinterm.set_window_default_focus": "window_resource",
    "splinterm.acquire_control": "control_resource",
    "splinterm.request_control_transfer": "control_resource",
    "splinterm.decide_control_transfer": "control_resource",
    "splinterm.release_control": "control_resource",
    "splinterm.input": "control_resource",
    "splinterm.resize": "control_resource",
}
MCP_RESOURCE_IDENTITY_DEFS = {
    "topology": "topology_resource",
    "terminal": "terminal_resource",
    "control": "control_resource",
}
MCP_FORBIDDEN_OUTPUT_PROPERTIES = {
    "argv",
    "body",
    "capability_token",
    "controller_id",
    "cwd",
    "daemon_id",
    "daemon_request_id",
    "environment",
    "full_argv",
    "input",
    "policy_body",
    "query",
    "raw_bytes",
    "request_id",
    "subscription_id",
    "terminal_bytes",
    "transfer_id",
}
EXPECTED_MCP_SCHEMAS = {
    "common.schema.json",
    "error.schema.json",
    *(f"resources/{resource}.schema.json" for resource in MCP_RESOURCES),
    *(
        f"tools/{tool.removeprefix('splinterm.')}.{direction}.schema.json"
        for tool in MCP_TOOLS
        for direction in ("input", "output")
    ),
}
EXPECTED_MCP_SCHEMA_SHA256 = {
    'common.schema.json': '16b85b7eaa5c05142ede88842650c8ec8c483b21c412a4b716b9f0b532b3a8e8',
    'error.schema.json': '83bf7548fa544b0dec1dadcfe27a235cf87bd98e2aac4e87bc201b809e46b821',
    'resources/control.schema.json': 'b6b4fa840655944b8167cc788af3a0a26796b540450c14ecbd92b57c98b65f66',
    'resources/terminal.schema.json': 'bb28b71e300fb0296af0513bd7a2be80eca9c1218229e0882ed972ce6b136452',
    'resources/topology.schema.json': 'be6db4a6bcddaac58ebfbbf5a51aa347738a3ee465c9a95e0e64829bb390f81f',
    'tools/acquire_control.input.schema.json': 'ffa5d776719a3c3099ec8ae6b10c1484322b4cc43bb366ceab6090acd99ca18c',
    'tools/acquire_control.output.schema.json': '4fa12bf4c04ce5dff1f838d854843da719203deb4d0e042766b8c43da84e27ca',
    'tools/authorization_status.input.schema.json': '1277194f262497b7b5773be9161978ec709527d08f42e251864ad084b5f333b5',
    'tools/authorization_status.output.schema.json': '83fcebe8137f82e0307d98e37bc25b54709601ada9a1c8e8ad12ba8ed9080792',
    'tools/close_splint.input.schema.json': 'cea471e7289ed86b496852159e2d5972d3eb0c0a7665f63945c685cce9598586',
    'tools/close_splint.output.schema.json': '004f295f5b2249fd98de1b5032743c6b6c942fcd86bee7ac75462ad1016ae44b',
    'tools/close_window.input.schema.json': 'a42eaab413a6de4be80c96f899a16f4ca5e3da6ea7c4fe574cb7a2936a487aa6',
    'tools/close_window.output.schema.json': '728b7805864edc5362d6c2ad8a4134c4c3f952d66dcc1bba96feabd3c24c20fe',
    'tools/create_dojo.input.schema.json': '4cf0e8ad8c2272805ab86745bfd875bdc246435a60ac27352eb3b7adea575ca1',
    'tools/create_dojo.output.schema.json': '1d769a982682c7ffab4301b8d778560ed215dc8536326ff6ec0863d3c5f058b1',
    'tools/decide_control_transfer.input.schema.json': '0c2d8b3df9047ab10064597ad2386b1445f8943573b69ad4c7ceddef2461c35a',
    'tools/decide_control_transfer.output.schema.json': '9a231ff1699850635e745977e5f63bbbe3bf50d2aa03ceb329dcc4438a95f6af',
    'tools/input.input.schema.json': '7bbe42cebe151ac289a91a30dfb008bcbc04934125c88d514386cce2a3cfc466',
    'tools/input.output.schema.json': 'c73707b59ca35c8d942ecaddb9931e6fb29a9c98178e8e178ee0f7f3009367c1',
    'tools/inspect_audit.input.schema.json': '5a34cc510b84239e327cdc952aeea9d0054d48da398e077ff190501705222c35',
    'tools/inspect_audit.output.schema.json': '29a8b2523ddd8c18074c1cdfadac994462e7d31ba8391acbf6e79036d81f02b1',
    'tools/inspect_splint.input.schema.json': '076dae805c9963cc5d8f3bd2329f8666e91437bf4cd99ac7c19f87c561c8cf21',
    'tools/inspect_splint.output.schema.json': 'dff3ac9baf8a2976d6c5c46767ed33e9fd57f85a41efb8a268cb82f7cf4641ff',
    'tools/inspect_topology.input.schema.json': '15f7caf8e6f919ef0b623405516040fa07dc8582495a2398091cc57840b3e9b8',
    'tools/inspect_topology.output.schema.json': 'ee87f47ef61f1b0529fe4821fc3aa9975907dc05a9ed9adefa84e64a80a8ebeb',
    'tools/kill_splint.input.schema.json': '3add9763a456f3d5fcdf2e8c1865b9ad2049a5543a962728ab13c77cdd9edc0e',
    'tools/kill_splint.output.schema.json': '68b50cdc972426cf0bc510a046cbc459878d92c3cbcc4cea1f90ee9283a10aa0',
    'tools/list_dojos.input.schema.json': 'd893683bbb03709602ba69548821ae1645ac262f118ec1e459b4d2758bcc4b52',
    'tools/list_dojos.output.schema.json': '82c41cfe554d09c96a5bda51065971efa45d6649210fbe2201597f5bfc7099bc',
    'tools/new_window.input.schema.json': 'f8e5e66fac7ddf89f711b235c06f6a02f58e93d90955d6dde0c97c315a5c04a1',
    'tools/new_window.output.schema.json': '24797c20e76b2ee79ce4b379ad7d4a928015f6188bc048d852430006d723fd83',
    'tools/ping.input.schema.json': '59a1f163d501d4f8c3e420b3d3cd346ad65ecc3bce6286d1f82f910651e19e79',
    'tools/ping.output.schema.json': 'fdbed2b95a57b6c2306067f1e1e403e0a2829d2246ccd739e4404c3407e0421c',
    'tools/read_scrollback.input.schema.json': '6a577d1926895eac1fdc86e4bff6c0f8e3ccebd3b1f2bc8b24bbc14d294e8cfe',
    'tools/read_scrollback.output.schema.json': '57fc9e44f761e478ca103a264b1bd22827714c1b7952082ba1538226c031763a',
    'tools/read_terminal.input.schema.json': 'e851d8814224faa718fbb0bd7cbc06c38af5c18d9f4a573bd1c9db21d23342b7',
    'tools/read_terminal.output.schema.json': '51e338c7fbdf1a238b8f13c3cfeb69e0fbf540409825050c07645fb78c571d72',
    'tools/relaunch_splint.input.schema.json': 'dd57d9c0e13a14add5924d802e830e517f28ecf5fd8e368f557ddc23b37e8418',
    'tools/relaunch_splint.output.schema.json': 'de9fef7511b805661b5c616a0c6369ed87d5c6d68f6e004db125d192a1eeed8c',
    'tools/release_control.input.schema.json': '2523c29e3646a78df63c92113a0326208a0fb11bfea3cfddba05561d88b98130',
    'tools/release_control.output.schema.json': '26a9dc866c805f02cd8355bc1888df4caa91963e7033b602edf693bda960f33f',
    'tools/rename_dojo.input.schema.json': 'ed452965131cacc362f684fda57fd091e5ee8294cf9ff7822e11918941bfaadf',
    'tools/rename_dojo.output.schema.json': 'cd0b65e4eb4bbba491aa79598d64c8ae20e6a6ac1ce299f264c51e2fbd9b7c58',
    'tools/rename_splint.input.schema.json': '31228fbce76185c16f444e9c857832c0990bc75e19bc1977ea4c6217ef31efcd',
    'tools/rename_splint.output.schema.json': 'a635b22f783f2b2ee41a16edcf001d82953fd68df74eb2b56944ba715a3808c6',
    'tools/rename_window.input.schema.json': '21ff865b1c196a137af920845a65583d403db34d25aca65ec3386beab16675a2',
    'tools/rename_window.output.schema.json': 'e135283f5523125adb0f56c69ad66387c57b2f2208d479553a084952b802ca1b',
    'tools/request_access.input.schema.json': '341a6aadf1733be71448c465eea7c1960ef47fa0e67e0a14a6382e268a34dda2',
    'tools/request_access.output.schema.json': '40c6700b46ebab5670429492228be6af263789467a09676ba88cee481c328f8f',
    'tools/request_control_transfer.input.schema.json': '90ae2be9ba12fcb598b80260fc8f9be65fa2518cc9684b859b6f8959768fb922',
    'tools/request_control_transfer.output.schema.json': 'a949da9eca3ff557c7b1bcc43a60a8090fe8c73eeaca659761ce0a34c2e083c9',
    'tools/resize.input.schema.json': '17f63e0437313e73f514e515cc8378ec24e3471b4bbd9ca1c16458e47e3f5027',
    'tools/resize.output.schema.json': 'aa076cd80adade4658a4dc94b0bd38e33df4d8e0d190fc88826ca26df94ecefa',
    'tools/restore_dojo.input.schema.json': '2f2126b58d357cffdd8808688ec86b66feeffa00e0956b23a0e8d1e77e10490b',
    'tools/restore_dojo.output.schema.json': '62afd3cc76ac488523e1a101efe1ed34398bb9a94d8aff399bc83ad442919b49',
    'tools/restore_splint.input.schema.json': '6df78b6d7f5f6eac2f912ff02da2acb3399cfdd68c6288b778aa3e0e6164aac5',
    'tools/restore_splint.output.schema.json': '23bb19d849fd1f5deaf6602f8e12ee0c555e68c3ee1729aa85e9c183942aea1c',
    'tools/restore_window.input.schema.json': 'd0417f2999a865582017fa1e94588570955d772a9e32a36f11a52bd3ed9a9b83',
    'tools/restore_window.output.schema.json': '4613758a1a6fb99ba54c88ad2354b5b8fcc7a38af70fe922e45cf334b6d7f9a9',
    'tools/revoke_access.input.schema.json': '144ef62eb04f6e021dfbf72fe4f750933b1d512fe29e8a42a0b689b1b377eece',
    'tools/revoke_access.output.schema.json': '12fd2dbfc06bc86d053d9ee25acec0438cb1a93619bc384f400e949e410f73b1',
    'tools/search_scrollback.input.schema.json': '74959ef167a4f9bbe89f2963e80eadac6aebbdc608ea26f38fc8a12419bd381c',
    'tools/search_scrollback.output.schema.json': 'b84f1282bdd3eeadc54b6dd13b31a4602225b3579edc59eb0ceee2c66bb5d4db',
    'tools/set_split_ratio.input.schema.json': 'c9ad9cdb071e132cfb418628884b8aff32970effe91f5af745261d04f9947303',
    'tools/set_split_ratio.output.schema.json': 'd7e8ad0a730f9a121a34bcfe6dfbe926357231a26cafbb5fbfa99e0d7ff73d2d',
    'tools/set_window_default_focus.input.schema.json': 'e766d933fa294e5f93ebbc8cb66626b0074bba8a64e7df432572f8030e0264e4',
    'tools/set_window_default_focus.output.schema.json': 'cf105a633e3e1063f5312a87821a8e09dd395fd07592743aec47f9d8278824c3',
    'tools/split_splint.input.schema.json': 'd879b1d60e36d810e799124aa9f81a22f0f05b5c977b6996f7de883b4916b190',
    'tools/split_splint.output.schema.json': 'd0588a8a89bab18fcd60e6efdb17c8e9d0f0f229013c708fe9b8027ddebd63d9',
}
EXPECTED_MCP_VALID_FIXTURES = {
    *(f"{tool.removeprefix('splinterm.')}.input.json" for tool in MCP_TOOLS),
    *(f"{tool.removeprefix('splinterm.')}.output.json" for tool in MCP_TOOLS),
    *(f"resource-{resource}.json" for resource in MCP_RESOURCES),
    *(f"error-{error_code.replace('_', '-')}.json" for error_code in MCP_ERROR_CODES),
}
EXPECTED_MCP_INVALID_FIXTURES = {
    "argv-echo.json",
    "body-echo.json",
    "duplicate-provenance-mismatch.json",
    "excessive-audit-limit.json",
    "excessive-row-limit.json",
    "false-confirmation.json",
    "false-terminal-trust.json",
    "hostile-title-false-trust.json",
    "input-echo.json",
    "malformed-controller-handle.json",
    "malformed-cursor.json",
    "malformed-transfer-handle.json",
    "malformed-uuid.json",
    "missing-confirmation.json",
    "missing-provenance.json",
    "missing-request-access-incarnation.json",
    "noncanonical-grant-id.json",
    "noncanonical-uuid.json",
    "open-output-object.json",
    "oversized-error-text.json",
    "oversized-input-text.json",
    "private-daemon-id.json",
    "query-echo.json",
    "raw-bytes-leak.json",
    "resource-open-metadata.json",
    "resource-private-controller-id.json",
    "uncommitted-mutation-success.json",
    "unknown-input-field.json",
    "uppercase-error-code.json",
    "wrong-resource-kind.json",
}


class ContractError(ValueError):
    """A checked-in contract artifact is malformed or has the wrong expectation."""


def load_json(path: Path) -> Any:
    size = path.stat().st_size
    if size == 0 or size > MAX_JSON_BYTES:
        raise ContractError(f"{path}: JSON size {size} is outside 1..{MAX_JSON_BYTES}")
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ContractError(f"{path}: cannot read JSON: {error}") from error


def assert_inventory(label: str, actual: set[str], expected: set[str]) -> None:
    if actual != expected:
        missing = sorted(expected - actual)
        extra = sorted(actual - expected)
        raise ContractError(f"{label} inventory mismatch; missing={missing}, extra={extra}")


def load_schema_documents(
    schema_dir: Path, expected: set[str]
) -> dict[str, dict[str, Any]]:
    actual = {
        path.relative_to(schema_dir).as_posix()
        for path in schema_dir.rglob("*.schema.json")
    }
    assert_inventory(f"{schema_dir.relative_to(ROOT)} schema", actual, expected)

    schemas: dict[str, dict[str, Any]] = {}
    for name in sorted(actual):
        schema = load_json(schema_dir / name)
        if not isinstance(schema, dict):
            raise ContractError(f"{name}: schema document must be an object")
        expected_id = (
            "https://splinterm.oldjobobo.com/schemas/"
            f"{schema_dir.relative_to(ROOT / 'dist' / 'schemas').as_posix()}/{name}"
        )
        if schema.get("$schema") != "https://json-schema.org/draft/2020-12/schema":
            raise ContractError(f"{name}: $schema must select canonical Draft 2020-12")
        if schema.get("$id") != expected_id:
            raise ContractError(
                f"{name}: $id must be path-derived canonical ID {expected_id!r}"
            )
        try:
            Draft202012Validator.check_schema(schema)
        except SchemaError as error:
            raise ContractError(
                f"{name}: invalid Draft 2020-12 schema: {error.message}"
            ) from error
        schemas[name] = schema
    return schemas


def build_validators(
    schemas: dict[str, dict[str, Any]],
) -> dict[str, Draft202012Validator]:
    resources = []
    schema_ids: set[str] = set()
    for name, schema in schemas.items():
        schema_id = schema.get("$id")
        if not isinstance(schema_id, str):
            raise ContractError(f"{name}: schema must declare a string $id")
        if schema_id in schema_ids:
            raise ContractError(f"{name}: duplicate schema $id {schema_id!r}")
        schema_ids.add(schema_id)
        resources.append((schema_id, Resource.from_contents(schema)))
    registry = Registry().with_resources(resources)
    return {
        name: Draft202012Validator(schema, registry=registry)
        for name, schema in schemas.items()
    }


def iter_schema_nodes(
    value: Any, path: tuple[str | int, ...] = ()
) -> Iterator[tuple[tuple[str | int, ...], dict[str, Any]]]:
    """Yield every schema node and its JSON path."""
    if isinstance(value, dict):
        yield path, value
        for key, child in value.items():
            yield from iter_schema_nodes(child, (*path, key))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from iter_schema_nodes(child, (*path, index))


def require_schema_condition(condition: bool, message: str) -> None:
    if not condition:
        raise ContractError(message)


def validate_mcp_schema_contract(schemas: dict[str, dict[str, Any]]) -> None:
    """Audit security-sensitive invariants across the complete MCP inventory."""
    assert_inventory(
        "MCP reviewed schema hash",
        set(schemas),
        set(EXPECTED_MCP_SCHEMA_SHA256),
    )
    for name, schema in schemas.items():
        canonical = json.dumps(
            schema, sort_keys=True, separators=(",", ":"), ensure_ascii=False
        ).encode("utf-8")
        actual_hash = hashlib.sha256(canonical).hexdigest()
        expected_hash = EXPECTED_MCP_SCHEMA_SHA256[name]
        if actual_hash != expected_hash:
            raise ContractError(
                f"{name}: reviewed structural hash mismatch; "
                f"expected={expected_hash}, actual={actual_hash}"
            )

    resources = [
        (schema["$id"], Resource.from_contents(schema)) for schema in schemas.values()
    ]
    registry = Registry().with_resources(resources)

    for name, schema in schemas.items():
        resolver = registry.resolver(base_uri=schema["$id"])
        for path, node in iter_schema_nodes(schema):
            location = "/".join(str(part) for part in path) or "<root>"
            if node.get("type") == "object":
                require_schema_condition(
                    node.get("additionalProperties") is False,
                    f"{name}:{location}: object schema must be closed",
                )
            reference = node.get("$ref")
            if isinstance(reference, str):
                try:
                    resolver.lookup(reference)
                except Unresolvable as error:
                    raise ContractError(
                        f"{name}:{location}: unresolved $ref {reference!r}: {error}"
                    ) from error

            if name.startswith(("tools/", "resources/")):
                forbidden = MCP_FORBIDDEN_OUTPUT_PROPERTIES.intersection(
                    node.get("properties", {})
                )
                if name.endswith(".output.schema.json") or name.startswith(
                    "resources/"
                ):
                    require_schema_condition(
                        not forbidden,
                        f"{name}:{location}: forbidden public output fields "
                        f"{sorted(forbidden)}",
                    )

    common_defs = schemas["common.schema.json"]["$defs"]
    require_schema_condition(
        tuple(common_defs["tool_name"]["enum"]) == MCP_TOOLS,
        "common.schema.json: tool_name inventory or order is not exact",
    )
    require_schema_condition(
        tuple(common_defs["error_code"]["enum"]) == MCP_ERROR_CODES,
        "common.schema.json: error_code inventory or order is not exact",
    )

    for tool in MCP_TOOLS:
        stem = tool.removeprefix("splinterm.")
        input_schema = schemas[f"tools/{stem}.input.schema.json"]["allOf"][0]
        output_overlay = schemas[f"tools/{stem}.output.schema.json"]["allOf"][1]
        output_properties = output_overlay["properties"]
        data_schema = output_properties["data"]
        if "$ref" in data_schema:
            data_schema = resolver.lookup(data_schema["$ref"]).contents

        require_schema_condition(
            input_schema.get("type") == "object"
            and input_schema.get("additionalProperties") is False,
            f"{tool}: input root must be a closed object",
        )
        require_schema_condition(
            output_properties["tool"].get("const") == tool,
            f"{tool}: output tool discriminator is not exact",
        )
        require_schema_condition(
            data_schema.get("type") == "object"
            and data_schema.get("additionalProperties") is False,
            f"{tool}: output data must be a closed object",
        )

        expected_trust = (
            "untrusted_terminal_data"
            if tool in MCP_UNTRUSTED_OUTPUT_TOOLS
            else "trusted_metadata"
        )
        require_schema_condition(
            output_properties["content_trust"].get("const") == expected_trust,
            f"{tool}: output trust label is not exact",
        )
        expected_resource_ref = (
            "https://splinterm.oldjobobo.com/schemas/mcp/v1/common.schema.json#/$defs/"
            f"{MCP_OUTPUT_RESOURCE_DEFS[tool]}"
        )
        require_schema_condition(
            output_properties.get("resource", {}).get("$ref")
            == expected_resource_ref,
            f"{tool}: output resource identity is not exact",
        )
        if tool in MCP_TERMINAL_OUTPUT_TOOLS:
            require_schema_condition(
                "provenance" not in data_schema.get("properties", {})
                and MCP_OUTPUT_RESOURCE_DEFS[tool] == "terminal_resource",
                f"{tool}: provenance must be carried once by terminal resource identity",
            )
        if tool in MCP_MUTATION_TOOLS:
            require_schema_condition(
                "committed" in data_schema.get("required", [])
                and data_schema["properties"]["committed"].get("const") is True,
                f"{tool}: successful mutation must require committed=true",
            )
        if tool in MCP_CONFIRMED_TOOLS:
            require_schema_condition(
                "confirm" in input_schema.get("required", [])
                and input_schema["properties"]["confirm"].get("const") is True,
                f"{tool}: destructive input must require confirm=true",
            )

    identity_properties = {
        "daemon_resource": {"kind"},
        "topology_resource": {"kind", "topology_revision"},
        "dojo_resource": {"kind", "dojo_id", "topology_revision"},
        "window_resource": {"kind", "dojo_id", "window_id", "topology_revision"},
        "splint_resource": {
            "kind",
            "dojo_id",
            "window_id",
            "splint_id",
            "incarnation",
            "topology_revision",
        },
        "logical_splint_resource": {
            "kind",
            "dojo_id",
            "window_id",
            "splint_id",
            "current_incarnation",
            "last_incarnation",
            "topology_revision",
        },
        "terminal_resource": {
            "kind",
            "dojo_id",
            "window_id",
            "splint_id",
            "incarnation",
            "topology_revision",
            "terminal_revision",
            "history_generation",
        },
        "authorization_resource": {
            "kind",
            "dojo_id",
            "window_id",
            "splint_id",
            "incarnation",
            "grant_id",
            "authorization_revision",
        },
        "audit_resource": {"kind"},
        "control_resource": {
            "kind",
            "dojo_id",
            "window_id",
            "splint_id",
            "incarnation",
            "control_revision",
        },
    }
    for definition, expected_properties in identity_properties.items():
        identity = common_defs[definition]
        require_schema_condition(
            identity.get("additionalProperties") is False
            and set(identity.get("properties", {})) == expected_properties
            and set(identity.get("required", [])) == expected_properties,
            f"common.schema.json: {definition} identity inventory is not exact",
        )

    require_schema_condition(
        common_defs["grant_id"]
        == {"type": "string", "pattern": "^[1-9][0-9]{0,19}$"},
        "common.schema.json: grant_id must be a canonical public nonzero decimal string",
    )
    topology_shapes = {
        "topology_data": {"dojos"},
        "topology_dojo": {"dojo_id", "name", "windows"},
        "topology_window": {
            "window_id",
            "title",
            "default_focus_splint_id",
            "splints",
        },
        "topology_splint": {
            "splint_id",
            "current_incarnation",
            "last_incarnation",
            "title",
            "state",
        },
    }
    for definition, expected_properties in topology_shapes.items():
        topology = common_defs[definition]
        require_schema_condition(
            topology.get("additionalProperties") is False
            and set(topology.get("properties", {})) == expected_properties
            and set(topology.get("required", [])) == expected_properties,
            f"common.schema.json: {definition} full topology DTO is not exact",
        )

    for resource_name, identity_def in MCP_RESOURCE_IDENTITY_DEFS.items():
        resource = schemas[f"resources/{resource_name}.schema.json"]["allOf"][0]
        expected_ref = (
            "https://splinterm.oldjobobo.com/schemas/mcp/v1/common.schema.json#/$defs/"
            f"{identity_def}"
        )
        require_schema_condition(
            resource["properties"].get("resource", {}).get("$ref") == expected_ref
            and "resource" in resource.get("required", []),
            f"{resource_name} resource contents must require exact resource identity",
        )

    terminal_resource = schemas["resources/terminal.schema.json"]["allOf"][0]
    require_schema_condition(
        terminal_resource["properties"]["content_trust"].get("const")
        == "untrusted_terminal_data"
        and "provenance" not in terminal_resource.get("properties", {}),
        "terminal resource must use untrusted trust and single resource provenance",
    )
    topology_resource = schemas["resources/topology.schema.json"]["allOf"][0]
    require_schema_condition(
        topology_resource["properties"]["content_trust"].get("const")
        == "untrusted_terminal_data"
        and topology_resource["properties"]["data"].get("$ref", "").endswith(
            "#/$defs/topology_data"
        ),
        "topology resource must carry bounded untrusted full topology data",
    )

    for name, schema in schemas.items():
        if not (name.endswith(".output.schema.json") or name.startswith("resources/")):
            continue
        property_names = {
            property_name
            for _, node in iter_schema_nodes(schema)
            for property_name in node.get("properties", {})
        }
        if property_names.intersection({"title", "name"}):
            root = schema["allOf"][0] if name.startswith("resources/") else schema["allOf"][1]
            require_schema_condition(
                root["properties"]["content_trust"].get("const")
                == "untrusted_terminal_data",
                f"{name}: attacker-controlled title/name output must be untrusted",
            )


def validate_automation_fixture(
    path: Path,
    validators: dict[str, Draft202012Validator],
    *,
    should_pass: bool,
) -> None:
    fixture = load_json(path)
    if not isinstance(fixture, dict) or set(fixture) != {"$schema_file", "document"}:
        raise ContractError(f"{path}: fixture must contain only $schema_file and document")
    schema_file = fixture["$schema_file"]
    if schema_file not in validators:
        raise ContractError(f"{path}: unknown schema {schema_file!r}")

    errors = sorted(
        validators[schema_file].iter_errors(fixture["document"]),
        key=lambda error: tuple(str(part) for part in error.absolute_path),
    )
    if should_pass and errors:
        raise ContractError(f"{path}: expected valid document: {errors[0].message}")
    if not should_pass and not errors:
        raise ContractError(f"{path}: invalid fixture unexpectedly passed")


def validate_mcp_fixture(
    path: Path,
    validators: dict[str, Draft202012Validator],
    *,
    should_pass: bool,
) -> None:
    fixture = load_json(path)
    expected_keys = {"$schema_file", "document"}
    if not should_pass:
        expected_keys.add("$expected_keyword")
    if not isinstance(fixture, dict) or set(fixture) != expected_keys:
        raise ContractError(f"{path}: fixture keys must be exactly {sorted(expected_keys)}")
    schema_file = fixture["$schema_file"]
    if schema_file not in validators:
        raise ContractError(f"{path}: unknown schema {schema_file!r}")

    errors = list(validators[schema_file].iter_errors(fixture["document"]))
    if should_pass and errors:
        raise ContractError(f"{path}: expected valid document: {errors[0].message}")
    if not should_pass:
        if not errors:
            raise ContractError(f"{path}: invalid fixture unexpectedly passed")
        expected_keyword = fixture["$expected_keyword"]
        actual = sorted({str(error.validator) for error in errors})
        if not isinstance(expected_keyword, str) or actual != [expected_keyword]:
            raise ContractError(
                f"{path}: expected only {expected_keyword!r} failures; "
                f"actual keywords={actual}"
            )


def validate_automation_contract() -> tuple[int, int, int]:
    schemas = load_schema_documents(
        AUTOMATION_SCHEMA_DIR, EXPECTED_AUTOMATION_SCHEMAS
    )
    validators = build_validators(schemas)
    valid = sorted((AUTOMATION_FIXTURE_DIR / "valid").glob("*.json"))
    invalid = sorted((AUTOMATION_FIXTURE_DIR / "invalid").glob("*.json"))
    if not valid or not invalid:
        raise ContractError("both automation fixture sets must be non-empty")
    for path in valid:
        validate_automation_fixture(path, validators, should_pass=True)
    for path in invalid:
        validate_automation_fixture(path, validators, should_pass=False)
    return len(valid), len(invalid), len(validators)


def validate_mcp_contract() -> tuple[int, int, int]:
    schemas = load_schema_documents(MCP_SCHEMA_DIR, EXPECTED_MCP_SCHEMAS)
    validate_mcp_schema_contract(schemas)
    validators = build_validators(schemas)
    valid_paths = sorted((MCP_FIXTURE_DIR / "valid").glob("*.json"))
    invalid_paths = sorted((MCP_FIXTURE_DIR / "invalid").glob("*.json"))
    assert_inventory(
        "MCP valid fixture", {path.name for path in valid_paths}, EXPECTED_MCP_VALID_FIXTURES
    )
    assert_inventory(
        "MCP invalid fixture",
        {path.name for path in invalid_paths},
        EXPECTED_MCP_INVALID_FIXTURES,
    )
    for path in valid_paths:
        validate_mcp_fixture(path, validators, should_pass=True)
    for path in invalid_paths:
        validate_mcp_fixture(path, validators, should_pass=False)
    return len(valid_paths), len(invalid_paths), len(validators)


def main() -> int:
    try:
        automation = validate_automation_contract()
        mcp = validate_mcp_contract()
    except (ContractError, OSError) as error:
        print(f"contract validation failed: {error}", file=sys.stderr)
        return 1

    print(
        f"Validated {automation[0]} valid and {automation[1]} invalid automation "
        f"fixtures against {automation[2]} schemas."
    )
    print(
        f"Validated {mcp[0]} valid and {mcp[1]} invalid MCP fixtures against "
        f"{mcp[2]} schemas ({len(MCP_TOOLS)} tools, {len(MCP_RESOURCES)} resources, "
        f"{len(MCP_ERROR_CODES)} error codes)."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
