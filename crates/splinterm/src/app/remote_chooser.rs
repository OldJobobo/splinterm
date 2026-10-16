//! Standalone remote-profile chooser (`splinterm remotes`) and its Recent Dojos entry.

use std::sync::mpsc as std_mpsc;

use anyhow::{Context, Result};
use splinterm::{
    SessionPickerDecision, SessionPickerItem, SessionPickerUi,
    config::AppConfig,
    diagnostics::{DiagnosticErrorCode, DiagnosticEventCode, DiagnosticLevel, global},
    remote::{RemoteCatalog, RemoteProfile},
    remote_launch::hand_off_to_remote_window,
};

use super::{remote_status::destination, sessions::run_standalone_picker};

const REMOTE_DOJOS_TITLE: &str = "Remote Dojos…";
const PROFILES_BLOCKER: &str = "Add profiles in remotes.toml";
const EMPTY_BLOCKER: &str = "No remote profiles · add one to remotes.toml";
const INVALID_BLOCKER: &str = "remotes.toml is invalid · run `splinterm remote list`";

/// A profile row. Rows without panes render without a running status.
fn remote_profile_item(profile: &RemoteProfile) -> SessionPickerItem {
    SessionPickerItem {
        display_title: profile.name().to_owned(),
        breadcrumb: destination(profile),
        working_directory: String::new(),
        pane_count: 0,
        running_pane_count: 0,
    }
}

/// The chooser's profile names, rows, and creation-row explanation.
fn chooser_contents(
    catalog: &Result<RemoteCatalog>,
) -> (Vec<String>, Vec<SessionPickerItem>, &'static str) {
    match catalog {
        Err(_) => (Vec::new(), Vec::new(), INVALID_BLOCKER),
        Ok(catalog) if catalog.is_empty() => (Vec::new(), Vec::new(), EMPTY_BLOCKER),
        Ok(catalog) => (
            catalog
                .iter()
                .map(|profile| profile.name().to_owned())
                .collect(),
            catalog.iter().map(remote_profile_item).collect(),
            PROFILES_BLOCKER,
        ),
    }
}

/// The Recent Dojos row that opens the remote chooser. It is offered when
/// profiles exist, and also when `remotes.toml` is invalid so the chooser can
/// say so; an absent or empty catalog adds no row.
fn remote_dojos_row_for(catalog: &Result<RemoteCatalog>) -> Option<SessionPickerItem> {
    let breadcrumb = match catalog {
        Err(_) => "Check remotes.toml".to_owned(),
        Ok(catalog) if catalog.is_empty() => return None,
        Ok(catalog) if catalog.iter().len() == 1 => "1 remote profile".to_owned(),
        Ok(catalog) => format!("{} remote profiles", catalog.iter().len()),
    };
    Some(SessionPickerItem {
        display_title: REMOTE_DOJOS_TITLE.to_owned(),
        breadcrumb,
        working_directory: String::new(),
        pane_count: 0,
        running_pane_count: 0,
    })
}

pub(in crate::app) fn remote_dojos_row() -> Option<SessionPickerItem> {
    remote_dojos_row_for(&RemoteCatalog::load_default())
}

fn report_invalid_catalog(error: &anyhow::Error) {
    if let Some(diagnostics) = global() {
        diagnostics.emit(
            DiagnosticLevel::Warn,
            DiagnosticEventCode::ConfigWarning,
            Some(DiagnosticErrorCode::Config),
        );
    }
    eprintln!("splinterm remote profiles are invalid: {error:#}");
}

/// Shows the remote chooser and opens the selected profile in a new Window.
pub(in crate::app) async fn run_remotes(config: AppConfig) -> Result<()> {
    let catalog = RemoteCatalog::load_default();
    if let Err(error) = &catalog {
        report_invalid_catalog(error);
    }
    let (names, items, blocker) = chooser_contents(&catalog);
    let decision = tokio::task::spawn_blocking(move || {
        let (decision, receiver) = std_mpsc::channel();
        let picker = SessionPickerUi::remote_profiles(items, blocker, decision);
        run_standalone_picker(&config, picker, &receiver, None)
    })
    .await
    .context("remote chooser task failed")??;
    match decision {
        Some(SessionPickerDecision::Open(index)) => {
            let name = names
                .get(index)
                .context("remote chooser returned an invalid selection")?;
            hand_off_to_remote_window(name)
        }
        Some(SessionPickerDecision::New) | None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(text: &str) -> Result<RemoteCatalog> {
        RemoteCatalog::parse(text, None)
    }

    const TWO_PROFILES: &str = r#"
version = 1
[remotes.wintermute]
host = "wintermute.example"
user = "case"
port = 2222
[remotes.straylight]
host = "straylight"
"#;

    #[test]
    fn chooser_lists_profiles_in_stable_order_with_destinations() {
        let (names, items, blocker) = chooser_contents(&catalog(TWO_PROFILES));
        assert_eq!(names, ["straylight", "wintermute"]);
        assert_eq!(items[0].display_title, "straylight");
        assert_eq!(items[0].breadcrumb, "straylight");
        assert_eq!(items[1].display_title, "wintermute");
        assert_eq!(items[1].breadcrumb, "case@wintermute.example:2222");
        assert!(items.iter().all(|item| item.pane_count == 0));
        assert_eq!(blocker, PROFILES_BLOCKER);
    }

    #[test]
    fn chooser_explains_missing_empty_and_invalid_profiles() {
        // `load_default` maps an absent default file to an empty catalog.
        let (names, items, blocker) = chooser_contents(&Ok(RemoteCatalog::default()));
        assert!(names.is_empty() && items.is_empty());
        assert_eq!(blocker, EMPTY_BLOCKER);

        let (names, items, blocker) = chooser_contents(&catalog("version = 1\n"));
        assert!(names.is_empty() && items.is_empty());
        assert_eq!(blocker, EMPTY_BLOCKER);

        for invalid in [
            "not toml",
            "version = 2\n",
            "version = 1\n[remotes.bad]\nhost = \"\"\n",
            "version = 1\n[remotes.bad]\nhost = \"x\"\nextra = 1\n",
        ] {
            let (names, items, blocker) = chooser_contents(&catalog(invalid));
            assert!(names.is_empty() && items.is_empty(), "{invalid:?}");
            assert_eq!(blocker, INVALID_BLOCKER, "{invalid:?}");
        }
    }

    #[test]
    fn recent_dojos_offers_a_remote_row_only_when_it_can_help() {
        assert_eq!(remote_dojos_row_for(&Ok(RemoteCatalog::default())), None);
        let row = remote_dojos_row_for(&catalog(TWO_PROFILES)).unwrap();
        assert_eq!(row.display_title, "Remote Dojos…");
        assert_eq!(row.breadcrumb, "2 remote profiles");
        assert_eq!(row.pane_count, 0);
        let one = remote_dojos_row_for(&catalog("version = 1\n[remotes.solo]\nhost = \"solo\"\n"))
            .unwrap();
        assert_eq!(one.breadcrumb, "1 remote profile");
        let invalid = remote_dojos_row_for(&catalog("version = 9\n")).unwrap();
        assert_eq!(invalid.breadcrumb, "Check remotes.toml");
    }
}
