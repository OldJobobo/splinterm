use std::{collections::BTreeSet, fs, path::Path};

const TOOLS: &[&str] = &[
    "acquire_control",
    "authorization_status",
    "close_splint",
    "close_window",
    "create_dojo",
    "decide_control_transfer",
    "input",
    "inspect_audit",
    "inspect_splint",
    "inspect_topology",
    "kill_splint",
    "list_dojos",
    "new_window",
    "ping",
    "read_scrollback",
    "read_terminal",
    "relaunch_splint",
    "release_control",
    "rename_dojo",
    "rename_splint",
    "rename_window",
    "request_access",
    "request_control_transfer",
    "resize",
    "restore_dojo",
    "restore_splint",
    "restore_window",
    "revoke_access",
    "search_scrollback",
    "set_split_ratio",
    "set_window_default_focus",
    "split_splint",
];

const ERROR_CODES: &[&str] = &[
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
];

const REVIEWED_SCHEMA_FNV64: &[(&str, u64)] = &[
    ("common.schema.json", 0xae6b_fe67_32f0_f36d),
    ("error.schema.json", 0x4053_5e11_b0db_4733),
    ("resources/control.schema.json", 0x3393_ec95_f7c9_7690),
    ("resources/terminal.schema.json", 0x2d2a_83b3_94b2_2751),
    ("resources/topology.schema.json", 0xe10f_4500_035f_0f82),
    (
        "tools/acquire_control.input.schema.json",
        0xa11e_ba62_346d_7477,
    ),
    (
        "tools/acquire_control.output.schema.json",
        0xe607_3dcb_3e8f_958d,
    ),
    (
        "tools/authorization_status.input.schema.json",
        0x0b7c_37ba_5ae8_1c63,
    ),
    (
        "tools/authorization_status.output.schema.json",
        0x8ee6_8b43_a1da_8776,
    ),
    (
        "tools/close_splint.input.schema.json",
        0x7f7f_5994_b8b0_1a1c,
    ),
    (
        "tools/close_splint.output.schema.json",
        0x7fe5_aa85_715b_cbf0,
    ),
    (
        "tools/close_window.input.schema.json",
        0x127a_fbf3_ff77_c540,
    ),
    (
        "tools/close_window.output.schema.json",
        0x31e4_67a4_14f4_ab18,
    ),
    ("tools/create_dojo.input.schema.json", 0x61de_2c09_a8fe_e045),
    (
        "tools/create_dojo.output.schema.json",
        0x5eb9_3d75_f8dc_77eb,
    ),
    (
        "tools/decide_control_transfer.input.schema.json",
        0xf007_00eb_006a_717d,
    ),
    (
        "tools/decide_control_transfer.output.schema.json",
        0xbc50_04e5_ccf7_99bd,
    ),
    ("tools/input.input.schema.json", 0x4449_374a_d3cc_83fe),
    ("tools/input.output.schema.json", 0x77bf_c206_fa4c_61f0),
    (
        "tools/inspect_audit.input.schema.json",
        0x98ff_a9aa_40a2_c41b,
    ),
    (
        "tools/inspect_audit.output.schema.json",
        0xc968_f027_5b40_25a9,
    ),
    (
        "tools/inspect_splint.input.schema.json",
        0xb802_ae31_d06d_17af,
    ),
    (
        "tools/inspect_splint.output.schema.json",
        0xe9f8_b0d3_7958_7f08,
    ),
    (
        "tools/inspect_topology.input.schema.json",
        0xc2e0_c93a_9aad_d0e7,
    ),
    (
        "tools/inspect_topology.output.schema.json",
        0x581e_ab5e_70fb_890b,
    ),
    ("tools/kill_splint.input.schema.json", 0x3c9f_43ef_a081_1353),
    (
        "tools/kill_splint.output.schema.json",
        0x4511_2f0b_564a_56c8,
    ),
    ("tools/list_dojos.input.schema.json", 0x7320_4873_7e12_c347),
    ("tools/list_dojos.output.schema.json", 0xd5b3_5626_1d6c_cbee),
    ("tools/new_window.input.schema.json", 0x9ad8_688a_3b94_d9d7),
    ("tools/new_window.output.schema.json", 0x6aa3_173c_8218_7a71),
    ("tools/ping.input.schema.json", 0x9ec5_15a1_f0ef_2cff),
    ("tools/ping.output.schema.json", 0xd840_42fc_2fba_cd5d),
    (
        "tools/read_scrollback.input.schema.json",
        0xd9cf_4d15_a24c_6a3a,
    ),
    (
        "tools/read_scrollback.output.schema.json",
        0xdd6e_6bd6_a593_789a,
    ),
    (
        "tools/read_terminal.input.schema.json",
        0x55d9_e2fd_ec56_03b5,
    ),
    (
        "tools/read_terminal.output.schema.json",
        0x576f_9ed5_aac8_bd84,
    ),
    (
        "tools/relaunch_splint.input.schema.json",
        0x1490_4ef7_acb3_0ed1,
    ),
    (
        "tools/relaunch_splint.output.schema.json",
        0xdd78_eaec_7355_df76,
    ),
    (
        "tools/release_control.input.schema.json",
        0x5aa1_c2fc_ba9d_65e3,
    ),
    (
        "tools/release_control.output.schema.json",
        0x7a56_ace0_ac98_f01c,
    ),
    ("tools/rename_dojo.input.schema.json", 0x863d_bb26_b489_0bf5),
    (
        "tools/rename_dojo.output.schema.json",
        0x741c_fada_77ab_4c71,
    ),
    (
        "tools/rename_splint.input.schema.json",
        0x13fd_4802_6e8f_54a1,
    ),
    (
        "tools/rename_splint.output.schema.json",
        0x2c08_b387_5258_0049,
    ),
    (
        "tools/rename_window.input.schema.json",
        0x7e8f_7d0f_eef2_258d,
    ),
    (
        "tools/rename_window.output.schema.json",
        0x7ff3_4bc0_b57c_fe25,
    ),
    (
        "tools/request_access.input.schema.json",
        0x8407_d0d2_79f7_f83b,
    ),
    (
        "tools/request_access.output.schema.json",
        0x23c3_ff30_bcf6_42f5,
    ),
    (
        "tools/request_control_transfer.input.schema.json",
        0xd7fe_596e_7b89_90ad,
    ),
    (
        "tools/request_control_transfer.output.schema.json",
        0x5bb1_92cc_d760_4f87,
    ),
    ("tools/resize.input.schema.json", 0xb830_ccf0_96f8_8ee7),
    ("tools/resize.output.schema.json", 0xc2cd_5bda_332b_8b0e),
    (
        "tools/restore_dojo.input.schema.json",
        0x47a3_3edb_d83b_8bcf,
    ),
    (
        "tools/restore_dojo.output.schema.json",
        0xfe75_93a9_10a9_326a,
    ),
    (
        "tools/restore_splint.input.schema.json",
        0x7c94_0c32_f226_ad5b,
    ),
    (
        "tools/restore_splint.output.schema.json",
        0xedb4_4e80_4d07_0d97,
    ),
    (
        "tools/restore_window.input.schema.json",
        0x31e0_61ec_b474_7737,
    ),
    (
        "tools/restore_window.output.schema.json",
        0xe8cc_f577_941b_6456,
    ),
    (
        "tools/revoke_access.input.schema.json",
        0xdaaf_f64b_8ac0_9c3f,
    ),
    (
        "tools/revoke_access.output.schema.json",
        0xfab6_c681_8e2e_2d71,
    ),
    (
        "tools/search_scrollback.input.schema.json",
        0x6e15_b917_4a29_b06c,
    ),
    (
        "tools/search_scrollback.output.schema.json",
        0x3929_7d8c_0372_ab6d,
    ),
    (
        "tools/set_split_ratio.input.schema.json",
        0x02ff_f308_05f5_f9af,
    ),
    (
        "tools/set_split_ratio.output.schema.json",
        0x7b46_8d12_5cbe_05c4,
    ),
    (
        "tools/set_window_default_focus.input.schema.json",
        0x1aca_681f_12b4_9e71,
    ),
    (
        "tools/set_window_default_focus.output.schema.json",
        0xd244_4068_e24c_151b,
    ),
    (
        "tools/split_splint.input.schema.json",
        0x9148_e283_d622_e28c,
    ),
    (
        "tools/split_splint.output.schema.json",
        0xfe09_2c41_9af7_5e3e,
    ),
];

fn file_stems(directory: &Path, suffix: &str) -> BTreeSet<String> {
    fs::read_dir(directory)
        .expect("schema directory must exist")
        .filter_map(|entry| {
            let entry = entry.expect("schema directory entry must be readable");
            if !entry
                .file_type()
                .expect("schema entry type must be readable")
                .is_file()
            {
                return None;
            }
            let name = entry
                .file_name()
                .into_string()
                .expect("schema filenames must be UTF-8");
            name.strip_suffix(suffix).map(str::to_owned)
        })
        .collect()
}

fn canonical_json(value: &serde_json::Value, output: &mut String) {
    match value {
        serde_json::Value::Null => output.push_str("null"),
        serde_json::Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        serde_json::Value::Number(value) => output.push_str(&value.to_string()),
        serde_json::Value::String(value) => {
            output.push_str(&serde_json::to_string(value).expect("string must serialize"));
        }
        serde_json::Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                canonical_json(value, output);
            }
            output.push(']');
        }
        serde_json::Value::Object(values) => {
            output.push('{');
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_unstable_by_key(|(key, _)| *key);
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                output.push_str(&serde_json::to_string(key).expect("object key must serialize"));
                output.push(':');
                canonical_json(value, output);
            }
            output.push('}');
        }
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[test]
fn schema_inventory_is_exactly_32_tools_and_three_resources() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let schema_root = root.join("dist/schemas/mcp/v1");
    let tools = schema_root.join("tools");
    let resources = schema_root.join("resources");

    assert_eq!(TOOLS.len(), 32);
    let expected_tools = TOOLS
        .iter()
        .map(|tool| (*tool).to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(file_stems(&tools, ".input.schema.json"), expected_tools);
    assert_eq!(file_stems(&tools, ".output.schema.json"), expected_tools);
    assert_eq!(
        file_stems(&resources, ".schema.json"),
        ["control", "terminal", "topology"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );
    assert_eq!(
        file_stems(&schema_root, ".schema.json"),
        ["common", "error"].into_iter().map(str::to_owned).collect()
    );

    let common: serde_json::Value = serde_json::from_slice(
        &fs::read(schema_root.join("common.schema.json")).expect("common schema must be readable"),
    )
    .expect("common schema must be valid JSON");
    let actual_error_codes = common["$defs"]["error_code"]["enum"]
        .as_array()
        .expect("error_code must be an enum")
        .iter()
        .map(|code| code.as_str().expect("error code must be a string"))
        .collect::<Vec<_>>();
    assert_eq!(actual_error_codes, ERROR_CODES);

    assert_eq!(REVIEWED_SCHEMA_FNV64.len(), 69);
    for (relative_path, expected_hash) in REVIEWED_SCHEMA_FNV64 {
        let path = schema_root.join(relative_path);
        let schema: serde_json::Value = serde_json::from_slice(
            &fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display())),
        )
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let expected_id = format!("https://splinterm.oldjobobo.com/schemas/mcp/v1/{relative_path}");
        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema",
            "{} has noncanonical $schema",
            path.display()
        );
        assert_eq!(
            schema["$id"],
            expected_id,
            "{} has an ID that does not match its path",
            path.display()
        );
        let mut canonical = String::new();
        canonical_json(&schema, &mut canonical);
        assert_eq!(
            fnv1a64(canonical.as_bytes()),
            *expected_hash,
            "{} differs from its reviewed structural hash",
            path.display()
        );
    }
}
