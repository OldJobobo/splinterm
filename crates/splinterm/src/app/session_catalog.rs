//! Neutral launch requests and session-catalog projections.

use std::path::PathBuf;

use anyhow::{Context, Result};
use splinterm::{
    SessionPickerItem,
    config::AppConfig,
    endpoint::{ConnectionFactory, LaunchSemantics},
    session_picker::{RecentDojos, SessionEntry},
};
use splinterm_core::{DojoId, LairId, TopologyRevision};
use splinterm_protocol::{AutomationLaunch, LaunchParameters, Request};

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

pub(in crate::app) fn automation_launch(
    cwd: Option<PathBuf>,
    argv: Vec<String>,
) -> AutomationLaunch {
    AutomationLaunch { cwd, argv }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the shared local/remote request boundary keeps every launch field explicit"
)]
pub(in crate::app) fn new_dojo_request_for(
    semantics: LaunchSemantics,
    expected_topology_revision: TopologyRevision,
    lairs: &[splinterm_core::Lair],
    lair_id: LairId,
    name: Option<String>,
    cwd: Option<PathBuf>,
    command: Vec<String>,
    config: &AppConfig,
) -> Result<Request> {
    let name = match name {
        Some(name) => name,
        None => next_default_dojo_name(lairs, lair_id)?,
    };
    Ok(match semantics {
        LaunchSemantics::LocalTrusted => Request::NewDojo {
            expected_topology_revision,
            lair_id,
            name,
            launch: launch_parameters(
                cwd.context("local Dojo working directory is unavailable")?,
                command,
                config,
            ),
        },
        LaunchSemantics::RemoteInteractive => Request::NewDojoAutomation {
            expected_topology_revision,
            lair_id,
            name,
            launch: automation_launch(cwd, command),
        },
    })
}

pub(in crate::app) fn create_request(
    factory: &ConnectionFactory,
    expected_topology_revision: TopologyRevision,
    name: String,
    cwd: Option<PathBuf>,
    command: Vec<String>,
    config: &AppConfig,
) -> Result<Request> {
    create_request_for(
        factory.capabilities().launch_semantics,
        expected_topology_revision,
        name,
        cwd,
        command,
        config,
    )
}

pub(in crate::app) fn create_request_for(
    semantics: LaunchSemantics,
    expected_topology_revision: TopologyRevision,
    name: String,
    cwd: Option<PathBuf>,
    command: Vec<String>,
    config: &AppConfig,
) -> Result<Request> {
    Ok(match semantics {
        LaunchSemantics::LocalTrusted => Request::CreateLair {
            expected_topology_revision,
            name,
            launch: launch_parameters(
                cwd.context("local launch working directory is unavailable")?,
                command,
                config,
            ),
        },
        LaunchSemantics::RemoteInteractive => Request::CreateLairAutomation {
            expected_topology_revision,
            name,
            launch: automation_launch(cwd, command),
        },
    })
}

pub(in crate::app) fn next_default_dojo_name(
    lairs: &[splinterm_core::Lair],
    lair_id: LairId,
) -> Result<String> {
    lairs
        .iter()
        .find(|lair| lair.id == lair_id)
        .context("selected Lair is not present in the current topology")?
        .next_default_dojo_name()
        .map_err(Into::into)
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

pub(in crate::app) fn recent_dojo_ids(factory: &ConnectionFactory) -> Vec<DojoId> {
    RecentDojos::discover_namespace(&factory.capabilities().recency_namespace).map_or_else(
        |error| {
            eprintln!("splinterm recent Dojos unavailable: {error:#}");
            Vec::new()
        },
        |store| store.load(),
    )
}

pub(in crate::app) fn remember_dojo(factory: &ConnectionFactory, dojo_id: DojoId) {
    match RecentDojos::discover_namespace(&factory.capabilities().recency_namespace)
        .and_then(|store| store.record(dojo_id))
    {
        Ok(()) => {}
        Err(error) => eprintln!("splinterm could not update recent Dojos: {error:#}"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use splinterm_core::Dojo;

    #[test]
    fn new_dojo_requests_preserve_revision_and_resolve_names_for_both_semantics() {
        let mut lair = splinterm_core::Lair::new("main", PathBuf::from("/tmp"));
        let lair_id = lair.id;
        lair.dojos
            .push(Dojo::with_shell("Dojo 3", PathBuf::from("/tmp")));
        let revision = TopologyRevision::new(41);
        let config = AppConfig::default();

        let local = new_dojo_request_for(
            LaunchSemantics::LocalTrusted,
            revision,
            std::slice::from_ref(&lair),
            lair_id,
            None,
            Some(PathBuf::from("/work")),
            vec!["local".to_owned()],
            &config,
        )
        .unwrap();
        assert!(matches!(
            local,
            Request::NewDojo {
                expected_topology_revision,
                lair_id: requested_lair,
                name,
                launch,
            } if expected_topology_revision == revision
                && requested_lair == lair_id
                && name == "Dojo 4"
                && launch.cwd == std::path::Path::new("/work")
                && launch.command == ["local"]
        ));

        let local_explicit = new_dojo_request_for(
            LaunchSemantics::LocalTrusted,
            revision,
            std::slice::from_ref(&lair),
            lair_id,
            Some("logs".to_owned()),
            Some(PathBuf::from("/work")),
            Vec::new(),
            &config,
        )
        .unwrap();
        assert!(matches!(
            local_explicit,
            Request::NewDojo {
                expected_topology_revision,
                name,
                ..
            } if expected_topology_revision == revision && name == "logs"
        ));

        let remote_implicit = new_dojo_request_for(
            LaunchSemantics::RemoteInteractive,
            revision,
            std::slice::from_ref(&lair),
            lair_id,
            None,
            None,
            Vec::new(),
            &config,
        )
        .unwrap();
        assert!(matches!(
            remote_implicit,
            Request::NewDojoAutomation {
                expected_topology_revision,
                name,
                ..
            } if expected_topology_revision == revision && name == "Dojo 4"
        ));

        let remote = new_dojo_request_for(
            LaunchSemantics::RemoteInteractive,
            revision,
            &[lair],
            lair_id,
            Some("logs".to_owned()),
            None,
            vec!["remote".to_owned()],
            &config,
        )
        .unwrap();
        assert!(matches!(
            remote,
            Request::NewDojoAutomation {
                expected_topology_revision,
                lair_id: requested_lair,
                name,
                launch,
            } if expected_topology_revision == revision
                && requested_lair == lair_id
                && name == "logs"
                && launch.cwd.is_none()
                && launch.argv == ["remote"]
        ));
    }
}
