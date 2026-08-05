//! Neutral launch requests and session-catalog projections.

use std::path::PathBuf;

use anyhow::{Context, Result};
use splinterm::{
    SessionPickerItem,
    config::AppConfig,
    session_picker::{RecentDojos, SessionEntry},
};
use splinterm_core::{DojoId, LairId, TopologyRevision};
use splinterm_protocol::{LaunchParameters, Request};

pub(in crate::app) fn launch_parameters(
    cwd: PathBuf,
    command: Vec<String>,
    config: &AppConfig,
) -> LaunchParameters {
    LaunchParameters {
        cwd,
        command,
        shell: config.shell.clone(),
        login_shell: config.login_shell,
        scrollback_lines: config.scrollback_lines,
    }
}

pub(in crate::app) fn create_request(
    expected_topology_revision: TopologyRevision,
    name: String,
    cwd: PathBuf,
    command: Vec<String>,
    config: &AppConfig,
) -> Request {
    Request::CreateLair {
        expected_topology_revision,
        name,
        launch: launch_parameters(cwd, command, config),
    }
}

pub(in crate::app) fn select_dojo_from(
    lairs: &[splinterm_core::Lair],
    selection: (LairId, DojoId),
) -> Result<splinterm_core::Dojo> {
    let (lair_id, dojo_id) = selection;
    let lair = lairs
        .iter()
        .find(|lair| lair.id == lair_id)
        .context("selected Lair is not present in the current topology")?;
    let dojo = lair
        .dojos
        .iter()
        .find(|dojo| dojo.id == dojo_id)
        .context("selected Dojo does not belong to the selected Dojo")?;
    dojo.root
        .find_splint(dojo.default_focus)
        .context("selected Dojo has an invalid default-focus hint")?;
    Ok(dojo.clone())
}

pub(in crate::app) fn recent_dojo_ids() -> Vec<DojoId> {
    RecentDojos::discover().map_or_else(
        |error| {
            eprintln!("splinterm recent sessions unavailable: {error:#}");
            Vec::new()
        },
        |store| store.load(),
    )
}

pub(in crate::app) fn remember_dojo(dojo_id: DojoId) {
    match RecentDojos::discover().and_then(|store| store.record(dojo_id)) {
        Ok(()) => {}
        Err(error) => eprintln!("splinterm could not update recent sessions: {error:#}"),
    }
}

pub(in crate::app) fn session_picker_item(entry: &SessionEntry) -> SessionPickerItem {
    SessionPickerItem {
        display_title: entry.display_title(),
        working_directory: entry.working_directory(),
        pane_count: entry.pane_count,
        running_pane_count: entry.running_panes,
    }
}
