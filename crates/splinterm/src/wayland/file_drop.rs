//! Strict, body-free local file-drop parsing and POSIX path quoting.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use splinterm_core::{DojoId, SplintId, TopologyRevision};

use crate::geometry::Rect;

pub(super) const URI_LIST_MIME: &str = "text/uri-list";
pub(super) const MAX_DROP_BYTES: usize = 64 * 1024;
pub(super) const MAX_DROP_URIS: usize = 32;
const MAX_DROP_PATH_BYTES: usize = 4096;
const MAX_DROP_PAYLOAD_BYTES: usize = 128 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct FileDropTarget {
    pub(super) topology_revision: TopologyRevision,
    pub(super) dojo_id: DojoId,
    pub(super) splint_id: SplintId,
    pub(super) incarnation: u64,
    pub(super) input_generation: u64,
}

pub(super) fn file_drop_target_is_current(
    captured: FileDropTarget,
    current: FileDropTarget,
    modal_open: bool,
    controller_active: bool,
    commands_available: bool,
) -> bool {
    !modal_open && controller_active && commands_available && captured == current
}

pub(super) fn pane_drop_target(
    content: Rect,
    panes: impl IntoIterator<Item = (SplintId, Rect)>,
    point: (f64, f64),
    modal_open: bool,
    over_divider: bool,
) -> Option<SplintId> {
    let contains = |rect: Rect| {
        let right = rect.x.checked_add(rect.width)?;
        let bottom = rect.y.checked_add(rect.height)?;
        Some(
            point.0 >= f64::from(rect.x)
                && point.0 < f64::from(right)
                && point.1 >= f64::from(rect.y)
                && point.1 < f64::from(bottom),
        )
    };
    if modal_open || over_divider || !contains(content)? {
        return None;
    }
    panes.into_iter().find_map(|(splint_id, rect)| {
        contains(rect)
            .is_some_and(|inside| inside)
            .then_some(splint_id)
    })
}

pub(super) fn accepted_uri_list_mime(mimes: &[String]) -> Option<String> {
    mimes
        .iter()
        .find(|mime| mime.as_str() == URI_LIST_MIME)
        .cloned()
}

pub(super) fn copy_action_supported(
    actions: wayland_client::protocol::wl_data_device_manager::DndAction,
) -> bool {
    actions.contains(wayland_client::protocol::wl_data_device_manager::DndAction::Copy)
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn percent_decode(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes
                .get(index + 1)
                .and_then(|byte| hex(*byte))
                .context("malformed file URI percent encoding")?;
            let low = bytes
                .get(index + 2)
                .and_then(|byte| hex(*byte))
                .context("malformed file URI percent encoding")?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(decoded).context("file URI path is not UTF-8")?;
    if decoded.is_empty()
        || decoded.len() > MAX_DROP_PATH_BYTES
        || decoded.chars().any(char::is_control)
    {
        bail!("file URI path is empty, overlong, or contains controls");
    }
    Ok(decoded)
}

fn local_file_uri_path(uri: &str) -> Result<PathBuf> {
    let rest = uri
        .strip_prefix("file:")
        .context("drop URI scheme is not file")?;
    let encoded_path = if let Some(authority_path) = rest.strip_prefix("//") {
        let slash = authority_path
            .find('/')
            .context("file URI has no absolute path")?;
        let authority = &authority_path[..slash];
        if !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost") {
            bail!("remote file URI authority is unsupported");
        }
        &authority_path[slash..]
    } else {
        rest
    };
    if !encoded_path.starts_with('/') {
        bail!("file URI path is not absolute");
    }
    if encoded_path.contains(['?', '#']) {
        bail!("file URI queries and fragments are unsupported");
    }
    let decoded = percent_decode(encoded_path)?;
    let path = PathBuf::from(decoded);
    if !path.is_absolute() {
        bail!("decoded file URI path is not absolute");
    }
    Ok(path)
}

fn quote_posix_path(path: &str) -> String {
    let mut quoted = String::with_capacity(path.len() + 2);
    quoted.push('\'');
    for character in path.chars() {
        if character == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(character);
        }
    }
    quoted.push('\'');
    quoted
}

pub(super) fn dropped_file_payload(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.is_empty() || bytes.len() > MAX_DROP_BYTES {
        bail!("file drop offer is empty or exceeds its byte limit");
    }
    let text = std::str::from_utf8(bytes).context("file drop offer is not UTF-8")?;
    let mut quoted = Vec::new();
    for raw_line in text.split('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if quoted.len() == MAX_DROP_URIS {
            bail!("file drop offer exceeds its URI count limit");
        }
        let path = local_file_uri_path(line)?;
        let metadata = std::fs::metadata(&path).context("dropped path is unavailable")?;
        if !metadata.file_type().is_file() {
            bail!("dropped path is not a regular file");
        }
        let path = path
            .to_str()
            .context("dropped local path is not valid UTF-8")?;
        quoted.push(quote_posix_path(path));
    }
    if quoted.is_empty() {
        bail!("file drop offer contains no local regular files");
    }
    let payload = quoted.join(" ");
    if payload.len() > MAX_DROP_PAYLOAD_BYTES
        || payload
            .as_bytes()
            .iter()
            .any(|byte| matches!(*byte, b'\n' | b'\r' | 0))
    {
        bail!("file drop payload exceeds limits or contains submission bytes");
    }
    Ok(payload.into_bytes())
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::net::UnixListener};

    use wayland_client::protocol::wl_data_device_manager::DndAction;

    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("splinterm-drop-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let path = root.join(name);
        fs::write(&path, b"fixture body is never read by drop parsing").unwrap();
        path
    }

    fn uri(path: &std::path::Path) -> String {
        let encoded = path
            .to_str()
            .unwrap()
            .replace('%', "%25")
            .replace(' ', "%20")
            .replace('\'', "%27");
        format!("file://{encoded}")
    }

    #[test]
    fn uri_list_preserves_order_unicode_spaces_quotes_and_no_submission_bytes() {
        let first = fixture("a file's name.txt");
        let second = fixture("界-leading-dash--x");
        let list = format!("# source comment\r\n{}\r\n{}\n", uri(&first), uri(&second));
        let payload = dropped_file_payload(list.as_bytes()).unwrap();
        let expected = format!(
            "'{}' '{}'",
            first.to_str().unwrap().replace('\'', "'\\''"),
            second.display()
        );
        assert_eq!(payload, expected.as_bytes());
        assert!(
            !payload
                .iter()
                .any(|byte| matches!(*byte, b'\n' | b'\r' | 0))
        );
    }

    #[test]
    fn uri_list_rejects_remote_malformed_relative_non_utf8_and_wrong_scheme() {
        for invalid in [
            "file://remote.example/tmp/x\n",
            "file:///tmp/%GG\n",
            "file:relative\n",
            "file:///tmp/%FF\n",
            "https://example.test/a\n",
            "file:///tmp/a%0Ab\n",
            "file:///tmp/a?query\n",
            "file:///tmp/a#fragment\n",
        ] {
            assert!(
                dropped_file_payload(invalid.as_bytes()).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn uri_list_rejects_missing_directories_special_files_and_bounds() {
        let directory =
            std::env::temp_dir().join(format!("splinterm-drop-dir-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        assert!(dropped_file_payload(format!("{}\n", uri(&directory)).as_bytes()).is_err());
        let socket = directory.join("socket");
        let _listener = UnixListener::bind(&socket).unwrap();
        assert!(dropped_file_payload(format!("{}\n", uri(&socket)).as_bytes()).is_err());
        assert!(dropped_file_payload(b"file:///definitely/missing\n").is_err());
        assert!(dropped_file_payload(&vec![b'x'; MAX_DROP_BYTES + 1]).is_err());
        let file = fixture("many");
        let too_many = std::iter::repeat_n(uri(&file), MAX_DROP_URIS + 1)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(dropped_file_payload(too_many.as_bytes()).is_err());
    }

    #[test]
    fn pane_target_is_half_open_and_rejects_modal_divider_or_outside_points() {
        let first = SplintId::new();
        let second = SplintId::new();
        let content = Rect {
            x: 0,
            y: 20,
            width: 200,
            height: 100,
        };
        let panes = || {
            [
                (
                    first,
                    Rect {
                        x: 0,
                        y: 20,
                        width: 100,
                        height: 100,
                    },
                ),
                (
                    second,
                    Rect {
                        x: 100,
                        y: 20,
                        width: 100,
                        height: 100,
                    },
                ),
            ]
        };
        assert_eq!(
            pane_drop_target(content, panes(), (0.0, 20.0), false, false),
            Some(first)
        );
        assert_eq!(
            pane_drop_target(content, panes(), (100.0, 20.0), false, false),
            Some(second)
        );
        assert_eq!(
            pane_drop_target(content, panes(), (200.0, 20.0), false, false),
            None
        );
        assert_eq!(
            pane_drop_target(content, panes(), (50.0, 19.0), false, false),
            None
        );
        assert_eq!(
            pane_drop_target(content, panes(), (50.0, 30.0), true, false),
            None
        );
        assert_eq!(
            pane_drop_target(content, panes(), (50.0, 30.0), false, true),
            None
        );
    }

    #[test]
    fn exact_target_revalidation_rejects_every_authority_or_identity_drift() {
        let captured = FileDropTarget {
            topology_revision: TopologyRevision::new(5),
            dojo_id: DojoId::new(),
            splint_id: SplintId::new(),
            incarnation: 7,
            input_generation: 11,
        };
        assert!(file_drop_target_is_current(
            captured, captured, false, true, true
        ));
        for current in [
            FileDropTarget {
                topology_revision: TopologyRevision::new(6),
                ..captured
            },
            FileDropTarget {
                dojo_id: DojoId::new(),
                ..captured
            },
            FileDropTarget {
                splint_id: SplintId::new(),
                ..captured
            },
            FileDropTarget {
                incarnation: 8,
                ..captured
            },
            FileDropTarget {
                input_generation: 12,
                ..captured
            },
        ] {
            assert!(!file_drop_target_is_current(
                captured, current, false, true, true
            ));
        }
        for (modal, controlled, commands) in [
            (true, true, true),
            (false, false, true),
            (false, true, false),
        ] {
            assert!(!file_drop_target_is_current(
                captured, captured, modal, controlled, commands
            ));
        }
    }

    #[test]
    fn mime_selection_and_action_are_uri_list_copy_only() {
        assert_eq!(
            accepted_uri_list_mime(&["text/plain".into(), URI_LIST_MIME.into()]),
            Some(URI_LIST_MIME.into())
        );
        assert_eq!(accepted_uri_list_mime(&["text/plain".into()]), None);
        assert!(copy_action_supported(DndAction::Copy));
        assert!(copy_action_supported(DndAction::Copy | DndAction::Move));
        assert!(!copy_action_supported(DndAction::Move));
        assert!(!copy_action_supported(DndAction::empty()));
    }
}
