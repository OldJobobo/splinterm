//! CLI grammar and binary application orchestration.

use std::env;
#[cfg(test)]
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Parser;
use splinterm::{
    automation::Connection,
    config::{AppConfig, ConfigLoad, load_default},
};
use splinterm_core::SplitRatio;
use splinterm_protocol::{ControlMode, Request, Response};
#[cfg(test)]
use {
    splinterm::{WindowCommand, WindowUpdate, automation::response_protocol_error},
    splinterm_core::{DojoId, LairId, SplintId, TopologyRevision},
    splinterm_protocol::{
        ControlTransferOutcome, ErrorCode, ServerFrame, SubscriptionEvent, TerminalSnapshot,
        TerminalUpdate,
    },
    std::path::PathBuf,
    tokio::sync::mpsc,
};

use super::commands::{Cli, Command, OutputMode};
#[cfg(test)]
use super::commands::{NewSplintSide, SplitAxis};
use super::window::run_live_multipane_window;

#[cfg(test)]
use super::pane_bridge::{
    EventAction, PendingPaneResize, classify_subscription_event, handle_control_event,
    optional_pane_controller, queue_pane_resize, resolved_resize_request, terminal_action_matches,
    update_advances_from, validate_attached_snapshot, validate_scrollback_page_response,
};
use super::{
    consent::run_consent_client,
    human_output::{print_lairs, print_response},
    local_service::{
        confirm_kill, run_policy_command, run_relay_command, run_reset_command, usage_error,
    },
    machine::{
        machine_exit_code, require_expected_incarnation, require_incarnation, run_machine_command,
        run_machine_subscription,
    },
    session_catalog::{create_request, launch_parameters, remember_dojo},
    sessions::{launch, reopen_recent, run_sessions, select_dojo},
};

pub(crate) async fn run() -> Result<()> {
    let Cli {
        output,
        schema_major,
        timeout_ms,
        command,
    } = Cli::parse();
    if matches!(
        &command,
        Command::Sessions
            | Command::Reopen
            | Command::Window { .. }
            | Command::Launch { .. }
            | Command::Consent
            | Command::Policy { .. }
            | Command::Relay { .. }
            | Command::Reset { .. }
    ) && (output.is_some() || schema_major.is_some() || timeout_ms.is_some())
    {
        usage_error(
            "automation output, schema, and timeout options are unavailable for graphical, policy, relay, and local service commands",
        );
    }
    if matches!(command, Command::Subscribe { .. }) && output != Some(OutputMode::Ndjson) {
        usage_error("subscriptions require --output ndjson");
    }
    if matches!(command, Command::Focus) && output != Some(OutputMode::Json) {
        usage_error("focus requires --output json");
    }
    if output == Some(OutputMode::Json) {
        let focus_command = matches!(&command, Command::Focus);
        match run_machine_command(
            command,
            schema_major.unwrap_or(2),
            timeout_ms.unwrap_or(5_000),
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(error) => {
                if focus_command {
                    eprintln!("focus request failed");
                } else {
                    eprintln!("{error:#}");
                }
                std::process::exit(machine_exit_code(&error));
            }
        }
    }
    if output == Some(OutputMode::Ndjson) {
        let Command::Subscribe { stream } = command else {
            usage_error("NDJSON output is reserved for subscription commands");
        };
        match run_machine_subscription(
            stream,
            schema_major.unwrap_or(2),
            timeout_ms.unwrap_or(5_000),
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(error) => {
                eprintln!("{error:#}");
                std::process::exit(machine_exit_code(&error));
            }
        }
    }
    if schema_major.is_some() || timeout_ms.is_some() {
        usage_error("--schema-major and --timeout-ms require --output json or ndjson");
    }
    if let Command::Policy { command } = command {
        return run_policy_command(command);
    }
    if let Command::Relay { stdio } = command {
        return run_relay_command(stdio);
    }
    if let Command::Reset { yes } = command {
        return run_reset_command(yes);
    }

    let ConfigLoad {
        config,
        diagnostics,
    } = load_default()?;
    for diagnostic in diagnostics {
        eprintln!("splinterm config: {diagnostic}");
    }
    run_configured_command(command, config).await
}

async fn run_configured_command(command: Command, config: AppConfig) -> Result<()> {
    match command {
        Command::Sessions => run_sessions(config).await,
        Command::Reopen => reopen_recent(config).await,
        Command::Window { lair_id, dojo_id } => {
            let dojo = select_dojo(lair_id.zip(dojo_id)).await?;
            remember_dojo(dojo.id);
            run_live_multipane_window(config, dojo).await
        }
        Command::Launch {
            cwd,
            name,
            splint_id,
            new,
            command,
        } => {
            let cwd =
                cwd.unwrap_or(env::current_dir().context("failed to read current directory")?);
            launch(name, cwd, splint_id, new, command, config).await
        }
        Command::Consent => tokio::task::spawn_blocking(run_consent_client)
            .await
            .context("trusted consent task failed")?,
        command => run_headless(command, &config).await,
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "explicit-ID lifecycle command construction stays adjacent for auditability"
)]
async fn run_headless(command: Command, config: &AppConfig) -> Result<()> {
    let mut connection = Connection::connect().await?;
    match command {
        Command::Sessions
        | Command::Reopen
        | Command::Window { .. }
        | Command::Launch { .. }
        | Command::Consent
        | Command::Policy { .. }
        | Command::Relay { .. }
        | Command::Reset { .. } => {
            unreachable!("graphical, policy, or relay command returned before daemon connection")
        }
        Command::Focus => bail!("focus requires --output json"),
        Command::Ping => print_response(connection.request(Request::Ping).await?),
        Command::List { all } => {
            let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await?
            else {
                anyhow::bail!("splinterd returned an unexpected response to list")
            };
            print_lairs(&lairs, all);
            Ok(())
        }
        Command::Topology => print_response(connection.request(Request::InspectTopology).await?),
        Command::Inspect { splint_id } => print_response(
            connection
                .request(Request::InspectSplint { splint_id })
                .await?,
        ),
        Command::New { name, cwd, command } => {
            let expected = connection.topology_revision().await?;
            print_response(
                connection
                    .request(create_request(
                        expected,
                        name,
                        cwd.unwrap_or(
                            env::current_dir().context("failed to read current directory")?,
                        ),
                        command,
                        config,
                    ))
                    .await?,
            )
        }
        Command::Split {
            target_splint_id,
            axis,
            side,
            ratio,
            expected_incarnation,
            cwd,
            command,
        } => {
            let ratio = SplitRatio::new(ratio)
                .map_err(|_| anyhow::anyhow!("split ratio must be between 1 and 999"))?;
            let Response::Topology { snapshot } =
                connection.request(Request::InspectTopology).await?
            else {
                bail!("splinterd returned an unexpected topology response");
            };
            require_expected_incarnation(&snapshot, target_splint_id, expected_incarnation)?;
            let expected_topology_revision = snapshot.revision;
            print_response(
                connection
                    .request(Request::SplitSplint {
                        expected_topology_revision,
                        target_splint_id,
                        axis: axis.into(),
                        side: side.into(),
                        ratio,
                        launch: launch_parameters(
                            cwd.unwrap_or(
                                env::current_dir().context("failed to read current directory")?,
                            ),
                            command,
                            config,
                        ),
                    })
                    .await?,
            )
        }
        Command::Close { splint_id, .. } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::CloseSplint {
                        expected_topology_revision,
                        splint_id,
                    })
                    .await?,
            )
        }
        Command::Ratio {
            target_splint_id,
            ratio,
        } => {
            let expected_topology_revision = connection.topology_revision().await?;
            let ratio = SplitRatio::new(ratio)
                .map_err(|_| anyhow::anyhow!("split ratio must be between 1 and 999"))?;
            print_response(
                connection
                    .request(Request::SetSplitRatio {
                        expected_topology_revision,
                        target_splint_id,
                        ratio,
                    })
                    .await?,
            )
        }
        Command::NewDojo {
            lair_id,
            name,
            cwd,
            command,
        } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::NewDojo {
                        expected_topology_revision,
                        lair_id,
                        name,
                        launch: launch_parameters(
                            cwd.unwrap_or(
                                env::current_dir().context("failed to read current directory")?,
                            ),
                            command,
                            config,
                        ),
                    })
                    .await?,
            )
        }
        Command::CloseDojo { dojo_id, .. } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::CloseDojo {
                        expected_topology_revision,
                        dojo_id,
                    })
                    .await?,
            )
        }
        Command::RenameLair { lair_id, name } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::RenameLair {
                        expected_topology_revision,
                        lair_id,
                        name,
                    })
                    .await?,
            )
        }
        Command::RenameDojo { dojo_id, name } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::RenameDojo {
                        expected_topology_revision,
                        dojo_id,
                        name,
                    })
                    .await?,
            )
        }
        Command::DojoFocusHint { dojo_id, splint_id } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::SetDojoDefaultFocus {
                        expected_topology_revision,
                        dojo_id,
                        splint_id,
                    })
                    .await?,
            )
        }
        Command::RenameSplint { splint_id, title } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::RenameSplint {
                        expected_topology_revision,
                        splint_id,
                        title,
                    })
                    .await?,
            )
        }
        Command::Relaunch {
            splint_id,
            cwd,
            command,
        } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::RelaunchSplint {
                        expected_topology_revision,
                        splint_id,
                        launch: launch_parameters(
                            cwd.unwrap_or(
                                env::current_dir().context("failed to read current directory")?,
                            ),
                            command,
                            config,
                        ),
                    })
                    .await?,
            )
        }
        Command::Restore { splint_id } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::RestoreSplint {
                        expected_topology_revision,
                        splint_id,
                    })
                    .await?,
            )
        }
        Command::RestoreDojo { dojo_id } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::RestoreDojo {
                        expected_topology_revision,
                        dojo_id,
                    })
                    .await?,
            )
        }
        Command::RestoreLair { lair_id } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::RestoreLair {
                        expected_topology_revision,
                        lair_id,
                    })
                    .await?,
            )
        }
        Command::Authorization { .. } | Command::Audit { .. } | Command::Subscribe { .. } => {
            bail!("authorization, audit, and subscriptions require machine output")
        }
        Command::Snapshot {
            splint_id,
            expected_incarnation,
        } => {
            let incarnation = connection.live_incarnation(splint_id).await?;
            require_incarnation(incarnation, expected_incarnation)?;
            print_response(
                connection
                    .request(Request::Attach {
                        splint_id,
                        incarnation: Some(incarnation),
                        scrollback_rows: 16,
                    })
                    .await?,
            )
        }
        Command::Scrollback { .. } | Command::Search { .. } => {
            bail!("scrollback and search require --output json")
        }
        Command::Send {
            splint_id,
            text,
            expected_incarnation,
        } => {
            let incarnation = connection.live_incarnation(splint_id).await?;
            require_incarnation(incarnation, expected_incarnation)?;
            let controller_id = connection
                .acquire_control(splint_id, incarnation, vec![ControlMode::Input])
                .await?;
            let response = connection
                .request(Request::Input {
                    controller_id,
                    splint_id,
                    incarnation,
                    bytes: text.into_bytes(),
                })
                .await?;
            connection.release_control(controller_id).await?;
            print_response(response)
        }
        Command::Resize {
            splint_id,
            columns,
            rows,
        } => {
            let incarnation = connection.live_incarnation(splint_id).await?;
            let controller_id = connection
                .acquire_control(splint_id, incarnation, vec![ControlMode::Resize])
                .await?;
            let response = connection
                .request(Request::Resize {
                    controller_id,
                    splint_id,
                    incarnation,
                    columns,
                    rows,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .await?;
            connection.release_control(controller_id).await?;
            print_response(response)
        }
        Command::Kill { splint_id, yes } => {
            if !yes && !confirm_kill(splint_id)? {
                println!("Kill cancelled.");
                return Ok(());
            }
            let incarnation = connection.live_incarnation(splint_id).await?;
            print_response(
                connection
                    .request(Request::KillSplint {
                        splint_id,
                        incarnation,
                    })
                    .await?,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use splinterm_protocol::{ActiveScreen, TerminalInputModes, TerminalRow};

    #[test]
    fn reset_requires_explicit_confirmation_for_unattended_use() {
        let guarded = Cli::try_parse_from(["splinterm", "reset"]).unwrap();
        assert!(matches!(guarded.command, Command::Reset { yes: false }));

        let confirmed = Cli::try_parse_from(["splinterm", "reset", "--yes"]).unwrap();
        assert!(matches!(confirmed.command, Command::Reset { yes: true }));
    }

    #[test]
    fn graphical_session_commands_are_explicit() {
        let sessions = Cli::try_parse_from(["splinterm", "sessions"]).unwrap();
        assert!(matches!(sessions.command, Command::Sessions));
        let reopen = Cli::try_parse_from(["splinterm", "reopen"]).unwrap();
        assert!(matches!(reopen.command, Command::Reopen));
    }

    #[test]
    fn list_defaults_to_active_lairs_and_all_is_explicit() {
        let active = Cli::try_parse_from(["splinterm", "list"]).unwrap();
        assert!(matches!(active.command, Command::List { all: false }));

        let all = Cli::try_parse_from(["splinterm", "list", "--all"]).unwrap();
        assert!(matches!(all.command, Command::List { all: true }));
    }

    fn snapshot(revision: u64) -> TerminalSnapshot {
        TerminalSnapshot {
            splint_id: SplintId::new(),
            incarnation: 1,
            revision,
            columns: 1,
            rows: 1,
            cursor_column: 0,
            cursor_row: 0,
            cursor_deferred_wrap: false,
            active_screen: ActiveScreen::Normal,
            input_modes: TerminalInputModes {
                application_cursor: false,
                application_keypad: false,
                focus_reporting: false,
                bracketed_paste: false,
                cursor_visible: true,
                cursor_blink: true,
                mouse_tracking: splinterm_protocol::MouseTracking::None,
                sgr_mouse: false,
            },
            palette: vec![0; 256],
            default_colors: [0x00eb_ebeb, 0x000e_1216, 0x00eb_ebeb],
            title: String::new(),
            visible_rows: vec![TerminalRow {
                row_id: Some(1),
                linebreak: true,
                cells: Vec::new(),
            }],
            history_generation: 1,
            oldest_available_scrollback_row_id: None,
            newest_available_scrollback_row_id: None,
            scrollback_rows: Vec::new(),
            available_scrollback_rows: 0,
            omitted_oldest_scrollback_rows: 0,
            images: None,
            exited_code: None,
            exited_signal: None,
        }
    }

    #[test]
    fn lifecycle_commands_parse_explicit_ids_and_bounded_ratios() {
        let id = SplintId::new();
        let cli = Cli::try_parse_from([
            "splinterm",
            "split",
            &id.to_string(),
            "--axis",
            "vertical",
            "--side",
            "first",
            "--ratio",
            "400",
            "--",
            "printf",
            "ready",
        ])
        .unwrap();
        let Command::Split {
            target_splint_id,
            axis: SplitAxis::Vertical,
            side: NewSplintSide::First,
            ratio: 400,
            command,
            ..
        } = cli.command
        else {
            panic!("expected parsed split command");
        };
        assert_eq!(target_splint_id, id);
        assert_eq!(command, vec!["printf", "ready"]);

        assert!(
            Cli::try_parse_from([
                "splinterm",
                "split",
                &id.to_string(),
                "--axis",
                "horizontal",
                "--side",
                "second",
                "--ratio",
                "0",
            ])
            .is_err()
        );
        assert!(matches!(
            Cli::try_parse_from(["splinterm", "kill", &id.to_string(), "--yes"])
                .unwrap()
                .command,
            Command::Kill {
                splint_id,
                yes: true,
            } if splint_id == id
        ));
    }

    #[test]
    fn relay_requires_explicit_stdio_transport() {
        assert!(matches!(
            Cli::try_parse_from(["splinterm", "relay", "--stdio"])
                .unwrap()
                .command,
            Command::Relay { stdio: true }
        ));
        assert!(Cli::try_parse_from(["splinterm", "relay"]).is_err());
    }

    #[test]
    fn window_command_requires_exact_paired_resource_ids() {
        let lair_id = LairId::new();
        let dojo_id = DojoId::new();
        let parsed = Cli::try_parse_from([
            "splinterm",
            "window",
            "--lair-id",
            &lair_id.to_string(),
            "--dojo-id",
            &dojo_id.to_string(),
        ])
        .unwrap();
        assert!(matches!(
            parsed.command,
            Command::Window {
                lair_id: Some(parsed_lair),
                dojo_id: Some(parsed_dojo),
            } if parsed_lair == lair_id && parsed_dojo == dojo_id
        ));
        assert!(
            Cli::try_parse_from(["splinterm", "window", "--dojo-id", &dojo_id.to_string(),])
                .is_err()
        );
    }

    #[test]
    fn launch_defaults_to_fresh_creation_with_a_collision_resistant_name() {
        let cli = Cli::try_parse_from(["splinterm", "launch"]).unwrap();
        let Command::Launch {
            name,
            splint_id,
            new,
            command,
            ..
        } = cli.command
        else {
            panic!("expected launch command");
        };
        assert!(name.is_none());
        assert!(splint_id.is_none());
        assert!(!new);
        assert!(command.is_empty());
    }

    #[test]
    fn create_request_preserves_direct_argv_without_shell_interpolation() {
        let argv = vec![
            "/usr/bin/printf".to_owned(),
            "%s\\n".to_owned(),
            "$(touch /tmp/must-not-run); spaced argument".to_owned(),
        ];
        let request = create_request(
            TopologyRevision::default(),
            "argv".to_owned(),
            PathBuf::from("/tmp"),
            argv.clone(),
            &AppConfig::default(),
        );
        let Request::CreateLair { launch, .. } = request else {
            panic!("expected create request");
        };
        assert_eq!(launch.command, argv);
    }

    #[test]
    fn aggregate_update_interval_advances_from_published_revision() {
        let update = TerminalUpdate {
            base_revision: 4,
            revision: 29,
            rows: Vec::new(),
            scrolls: Vec::new(),
            cursor: None,
            title: None,
            input_modes: None,
            active_screen: None,
            palette: None,
            default_colors: None,
            columns: None,
            row_count: None,
            scrollback: None,
            images: None,
        };
        assert!(update_advances_from(&update, 4));
        assert!(!update_advances_from(&update, 3));
        let stale = TerminalUpdate {
            revision: 4,
            ..update
        };
        assert!(!update_advances_from(&stale, 4));
    }

    #[test]
    fn subscription_classifier_tracks_order_and_resyncs_gaps() {
        let action = classify_subscription_event(
            9,
            0,
            9,
            1,
            SubscriptionEvent::Snapshot {
                snapshot: snapshot(2),
            },
        );
        assert!(matches!(action, EventAction::Snapshot { sequence: 1, .. }));
        assert_eq!(
            classify_subscription_event(
                9,
                1,
                9,
                3,
                SubscriptionEvent::Snapshot {
                    snapshot: snapshot(3)
                },
            ),
            EventAction::Resynchronize
        );
        assert_eq!(
            classify_subscription_event(
                9,
                1,
                9,
                2,
                SubscriptionEvent::ResyncRequired {
                    current_revision: 4
                },
            ),
            EventAction::Resynchronize
        );
    }

    #[test]
    fn scrollback_page_response_validation_enforces_request_identity_and_cursor() {
        let page = splinterm_protocol::ScrollbackPage {
            splint_id: SplintId::new(),
            incarnation: 2,
            terminal_revision: 4,
            history_generation: 3,
            oldest_available_row_id: Some(1),
            newest_available_row_id: Some(12),
            rows: vec![TerminalRow {
                row_id: Some(8),
                linebreak: false,
                cells: Vec::new(),
            }],
            has_older: true,
        };
        assert!(
            validate_scrollback_page_response(
                &page,
                page.splint_id,
                page.incarnation,
                page.terminal_revision,
                page.history_generation,
                9,
            )
            .is_ok()
        );
        assert!(
            validate_scrollback_page_response(
                &page,
                SplintId::new(),
                page.incarnation,
                page.terminal_revision,
                page.history_generation,
                9,
            )
            .is_err()
        );
        assert!(
            validate_scrollback_page_response(
                &page,
                page.splint_id,
                page.incarnation,
                page.terminal_revision + 1,
                page.history_generation,
                9,
            )
            .is_err()
        );
        assert!(
            validate_scrollback_page_response(
                &page,
                page.splint_id,
                page.incarnation,
                page.terminal_revision,
                page.history_generation,
                8,
            )
            .is_err()
        );
    }

    #[test]
    fn initial_attachment_validation_rejects_malformed_and_wrong_identity_snapshots() {
        let valid = snapshot(1);
        assert!(validate_attached_snapshot(&valid, valid.splint_id, valid.incarnation).is_ok());

        let mut malformed = valid.clone();
        malformed.history_generation = 0;
        assert!(
            validate_attached_snapshot(&malformed, valid.splint_id, valid.incarnation).is_err()
        );
        assert!(validate_attached_snapshot(&valid, SplintId::new(), valid.incarnation).is_err());
        assert!(
            validate_attached_snapshot(&valid, valid.splint_id, valid.incarnation + 1).is_err()
        );
    }

    #[test]
    fn control_conflict_falls_back_to_observer_without_hiding_other_errors() {
        let unavailable = response_protocol_error(splinterm_protocol::ProtocolError::new(
            ErrorCode::ControllerUnavailable,
            "live Splint already has a controller",
        ));
        assert_eq!(optional_pane_controller(Err(unavailable)).unwrap(), None);
        assert_eq!(optional_pane_controller(Ok(42)).unwrap(), Some(42));

        let invalid = response_protocol_error(splinterm_protocol::ProtocolError::new(
            ErrorCode::InvalidArgument,
            "bad control request",
        ));
        assert!(optional_pane_controller(Err(invalid)).is_err());
    }

    #[test]
    fn subscription_classifier_ignores_old_subscription_and_stops_on_exit() {
        assert_eq!(
            classify_subscription_event(
                9,
                0,
                8,
                1,
                SubscriptionEvent::Snapshot {
                    snapshot: snapshot(2)
                },
            ),
            EventAction::Ignore
        );
        assert_eq!(
            classify_subscription_event(
                9,
                0,
                9,
                1,
                SubscriptionEvent::Exited {
                    code: Some(0),
                    signal: None
                },
            ),
            EventAction::Exited
        );
        assert_eq!(
            classify_subscription_event(
                9,
                0,
                9,
                1,
                SubscriptionEvent::AccessRevoked { grant_id: 4 },
            ),
            EventAction::Shutdown
        );
        assert_eq!(
            classify_subscription_event(
                9,
                1,
                9,
                7,
                SubscriptionEvent::Exited {
                    code: Some(0),
                    signal: None
                },
            ),
            EventAction::Exited
        );
        assert_eq!(
            classify_subscription_event(
                9,
                1,
                9,
                7,
                SubscriptionEvent::AccessRevoked { grant_id: 5 },
            ),
            EventAction::Shutdown
        );
    }

    #[test]
    fn resize_is_retained_until_control_is_available_and_uses_existing_control() {
        let splint_id = SplintId::new();
        let resize = (80, 40, 800, 800);
        let mut prepared = None;

        assert!(resolved_resize_request(None, &mut prepared, (splint_id, 3), resize).is_none());
        assert_eq!(prepared, Some(resize));

        let request = resolved_resize_request(
            Some(9),
            &mut prepared,
            (splint_id, 3),
            (100, 50, 1_000, 1_000),
        )
        .unwrap();
        assert!(matches!(
            request,
            Request::Resize {
                controller_id: 9,
                splint_id: requested_splint,
                incarnation: 3,
                columns: 100,
                rows: 50,
                pixel_width: 1_000,
                pixel_height: 1_000,
            } if requested_splint == splint_id
        ));
        assert_eq!(prepared, None);
    }

    #[test]
    fn pane_resize_debounce_keeps_latest_size_any_control_claim_and_idle_deadline() {
        let mut pending = None;
        let mut deadline = None;
        let delay = Duration::from_millis(100);
        let started = tokio::time::Instant::now();
        assert!(
            queue_pane_resize(
                &mut pending,
                &mut deadline,
                PendingPaneResize {
                    size: (80, 24, 800, 480),
                    claim_control: false,
                },
                delay,
                started,
            )
            .is_none()
        );
        assert_eq!(deadline, Some(started + delay));

        let latest_at = started + Duration::from_millis(50);
        assert!(
            queue_pane_resize(
                &mut pending,
                &mut deadline,
                PendingPaneResize {
                    size: (100, 40, 1_000, 800),
                    claim_control: true,
                },
                delay,
                latest_at,
            )
            .is_none()
        );
        assert_eq!(deadline, Some(latest_at + delay));
        assert_eq!(
            pending,
            Some(PendingPaneResize {
                size: (100, 40, 1_000, 800),
                claim_control: true,
            })
        );
    }

    #[test]
    fn zero_resize_delay_returns_immediate_command_without_pending_state() {
        let mut pending = None;
        let mut deadline = None;
        let immediate = queue_pane_resize(
            &mut pending,
            &mut deadline,
            PendingPaneResize {
                size: (80, 24, 800, 480),
                claim_control: true,
            },
            Duration::ZERO,
            tokio::time::Instant::now(),
        );

        assert!(matches!(immediate, Some(WindowCommand::Resize { .. })));
        assert!(pending.is_none());
        assert!(deadline.is_none());
    }

    #[tokio::test]
    async fn granted_control_event_exposes_controller_for_prepared_resize() {
        let (updates, mut receiver) = mpsc::channel(1);
        let mut active_controller = None;
        let acquired = handle_control_event(
            ServerFrame::Event {
                subscription_id: 7,
                sequence: 1,
                event: SubscriptionEvent::ControlTransferResolved {
                    transfer_id: 11,
                    outcome: ControlTransferOutcome::Granted,
                    controller_id: Some(9),
                },
            },
            7,
            &mut active_controller,
            &updates,
        )
        .await
        .unwrap();

        assert!(acquired);
        assert_eq!(active_controller, Some(9));
        assert!(matches!(
            receiver.try_recv(),
            Ok(WindowUpdate::ControlTransferResolved(
                ControlTransferOutcome::Granted
            ))
        ));
        let mut prepared = Some((100, 40, 1_000, 800));
        assert!(matches!(
            resolved_resize_request(
                active_controller,
                &mut prepared,
                (SplintId::new(), 3),
                (100, 40, 1_000, 800),
            ),
            Some(Request::Resize {
                controller_id: 9,
                columns: 100,
                rows: 40,
                ..
            })
        ));
        assert!(prepared.is_none());
    }

    #[test]
    fn window_controller_accepts_only_its_exact_terminal_action_acknowledgement() {
        let splint_id = SplintId::new();
        let response = Response::TerminalActionAcknowledged {
            lair_id: splinterm_core::LairId::new(),
            dojo_id: splinterm_core::DojoId::new(),
            splint_id,
            incarnation: 3,
            terminal_revision: 7,
            history_generation: 2,
        };
        assert!(terminal_action_matches(&response, splint_id, 3));
        assert!(!terminal_action_matches(&response, splint_id, 4));
        assert!(!terminal_action_matches(&response, SplintId::new(), 3));
        assert!(!terminal_action_matches(
            &Response::Acknowledged,
            splint_id,
            3
        ));
    }
}
