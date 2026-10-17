//! `splinterm agent-status`: explicit agent display status for one Splint.
//!
//! The target defaults to the daemon-injected `SPLINTERM_SPLINT_ID` and
//! `SPLINTERM_SPLINT_INCARNATION` discovery hints. They are never authority:
//! the daemon accepts a report only from a process inside the target Splint or
//! from a peer holding an `agent_status_report` grant.

use std::{ffi::OsString, fmt::Write as _};

use anyhow::{Result, bail};
use splinterm::{automation::Connection, endpoint::ConnectionFactory, remote::RemoteCatalog};
use splinterm_core::SplintId;
use splinterm_protocol::{AgentState, AgentStatusReport, AgentStatusSummary, Request, Response};

use super::commands::{AgentStatusCommand, AgentStatusTarget};

pub(in crate::app) const SPLINT_ID_ENV: &str = "SPLINTERM_SPLINT_ID";
pub(in crate::app) const SPLINT_INCARNATION_ENV: &str = "SPLINTERM_SPLINT_INCARNATION";

pub(in crate::app) const NOT_INSIDE_SPLINT_REPORT: &str = "not inside a Splint: SPLINTERM_SPLINT_ID and SPLINTERM_SPLINT_INCARNATION are unset; pass --splint ID --incarnation N";
pub(in crate::app) const NOT_INSIDE_SPLINT_GET: &str =
    "not inside a Splint: SPLINTERM_SPLINT_ID is unset; pass a SPLINT_ID";
const MALFORMED_SPLINT_ENVIRONMENT: &str = "the Splint environment is incomplete or malformed: SPLINTERM_SPLINT_ID must be a Splint UUID and SPLINTERM_SPLINT_INCARNATION a positive integer; pass --splint ID --incarnation N";

/// One resolved agent status operation with an explicit target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) enum AgentStatusRequest {
    Report {
        splint_id: SplintId,
        incarnation: u64,
        status: Option<AgentStatusReport>,
    },
    Get {
        splint_id: SplintId,
    },
}

impl AgentStatusRequest {
    pub(in crate::app) const fn operation(&self) -> &'static str {
        match self {
            Self::Report {
                status: Some(_), ..
            } => "agent_status_set",
            Self::Report { status: None, .. } => "agent_status_clear",
            Self::Get { .. } => "agent_status_get",
        }
    }
}

fn nonempty(value: Option<OsString>) -> Option<OsString> {
    value.filter(|value| !value.is_empty())
}

fn parse_splint_id(value: &OsString) -> Option<SplintId> {
    value.to_str()?.parse().ok()
}

fn parse_incarnation(value: &OsString) -> Option<u64> {
    value
        .to_str()?
        .parse::<u64>()
        .ok()
        .filter(|incarnation| *incarnation > 0)
}

fn resolve_report_target(
    target: AgentStatusTarget,
    environment: &impl Fn(&str) -> Option<OsString>,
) -> Result<(SplintId, u64), &'static str> {
    if let (Some(splint_id), Some(incarnation)) = (target.splint, target.incarnation) {
        return Ok((splint_id, incarnation));
    }
    match (
        nonempty(environment(SPLINT_ID_ENV)),
        nonempty(environment(SPLINT_INCARNATION_ENV)),
    ) {
        (None, None) => Err(NOT_INSIDE_SPLINT_REPORT),
        (Some(splint_id), Some(incarnation)) => parse_splint_id(&splint_id)
            .zip(parse_incarnation(&incarnation))
            .ok_or(MALFORMED_SPLINT_ENVIRONMENT),
        _ => Err(MALFORMED_SPLINT_ENVIRONMENT),
    }
}

/// Resolves the command's explicit or environment-default target.
///
/// # Errors
///
/// Returns a usage message when neither flags nor the environment name a
/// target, or the environment names one ambiguously.
pub(in crate::app) fn resolve_agent_status_request(
    command: AgentStatusCommand,
    environment: &impl Fn(&str) -> Option<OsString>,
) -> Result<AgentStatusRequest, &'static str> {
    match command {
        AgentStatusCommand::Set {
            state,
            detail,
            ttl_seconds,
            target,
        } => {
            let (splint_id, incarnation) = resolve_report_target(target, environment)?;
            Ok(AgentStatusRequest::Report {
                splint_id,
                incarnation,
                status: Some(AgentStatusReport {
                    state: state.into(),
                    detail,
                    ttl_seconds,
                }),
            })
        }
        AgentStatusCommand::Clear { target } => {
            let (splint_id, incarnation) = resolve_report_target(target, environment)?;
            Ok(AgentStatusRequest::Report {
                splint_id,
                incarnation,
                status: None,
            })
        }
        AgentStatusCommand::Get {
            splint_id: Some(splint_id),
        } => Ok(AgentStatusRequest::Get { splint_id }),
        AgentStatusCommand::Get { splint_id: None } => {
            let value = nonempty(environment(SPLINT_ID_ENV)).ok_or(NOT_INSIDE_SPLINT_GET)?;
            parse_splint_id(&value)
                .map(|splint_id| AgentStatusRequest::Get { splint_id })
                .ok_or(MALFORMED_SPLINT_ENVIRONMENT)
        }
    }
}

pub(in crate::app) const fn state_label(state: AgentState) -> &'static str {
    match state {
        AgentState::Working => "Working",
        AgentState::NeedsInput => "Needs input",
        AgentState::Done => "Done",
        AgentState::Error => "Error",
        AgentState::Idle => "Idle",
    }
}

fn status_line(
    splint_id: SplintId,
    incarnation: Option<u64>,
    status: Option<&AgentStatusSummary>,
) -> String {
    let Some(status) = status else {
        return format!("Splint {splint_id}: no agent status.");
    };
    let mut line = format!(
        "Splint {splint_id} incarnation {}: {}",
        incarnation.unwrap_or_default(),
        state_label(status.state)
    );
    if let Some(detail) = &status.detail {
        line.push_str(" · ");
        line.push_str(detail);
    }
    if status.stale {
        line.push_str(" · stale");
    }
    let _ = write!(line, " (reported {}s ago).", status.reported_age_ms / 1_000);
    line
}

/// Runs one resolved agent status operation with human output.
///
/// Reports always use the automation role: their authority comes from being
/// inside the Splint or from a grant, never from the trusted graphical client.
pub(in crate::app) async fn run_human_agent_status(
    request: AgentStatusRequest,
    remote: Option<String>,
) -> Result<()> {
    match request {
        AgentStatusRequest::Report {
            splint_id,
            incarnation,
            status,
        } => {
            let state = status.as_ref().map(|status| status.state);
            let mut connection = Connection::connect_automation().await?;
            let response = connection
                .request(Request::ReportAgentStatus {
                    splint_id,
                    incarnation,
                    status,
                })
                .await?;
            let Response::AgentStatusAccepted { .. } = response else {
                bail!("splinterd returned an unexpected agent status response");
            };
            match state {
                Some(state) => println!(
                    "Agent status {} reported for Splint {splint_id} incarnation {incarnation}.",
                    state.as_str()
                ),
                None => {
                    println!(
                        "Agent status cleared for Splint {splint_id} incarnation {incarnation}."
                    );
                }
            }
            Ok(())
        }
        AgentStatusRequest::Get { splint_id } => {
            let factory = match remote {
                Some(profile) => {
                    ConnectionFactory::remote(RemoteCatalog::load_default()?.get(&profile)?).await?
                }
                None => ConnectionFactory::local(),
            };
            let mut connection = factory.connect().await?;
            let Response::Splint { runtime, .. } = connection
                .request(Request::InspectSplint { splint_id })
                .await?
            else {
                bail!("splinterd returned an unexpected Splint response");
            };
            if runtime.splint_id != splint_id {
                bail!("splinterd returned a different Splint");
            }
            println!(
                "{}",
                status_line(
                    splint_id,
                    runtime.live_incarnation,
                    runtime.agent_status.as_ref()
                )
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use clap::Parser;

    use super::*;
    use crate::app::commands::{Cli, Command};

    const SPLINT: &str = "018f4d8c-2a18-4b31-8c2f-9e7c5de77103";
    const OTHER: &str = "018f4d8c-2a18-4b31-8c2f-9e7c5de77104";

    fn parse(arguments: &[&str]) -> AgentStatusCommand {
        let mut argv = vec!["splinterm", "agent-status"];
        argv.extend_from_slice(arguments);
        match Cli::try_parse_from(argv).unwrap().command {
            Some(Command::AgentStatus { command }) => command,
            other => panic!("unexpected command {other:?}"),
        }
    }

    fn environment(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let values = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), OsString::from(value)))
            .collect::<HashMap<_, _>>();
        move |key| values.get(key).cloned()
    }

    #[test]
    fn set_defaults_to_the_injected_splint_environment() {
        let inside = environment(&[(SPLINT_ID_ENV, SPLINT), (SPLINT_INCARNATION_ENV, "3")]);
        let request =
            resolve_agent_status_request(parse(&["set", "needs_input"]), &inside).unwrap();
        assert_eq!(
            request,
            AgentStatusRequest::Report {
                splint_id: SPLINT.parse().unwrap(),
                incarnation: 3,
                status: Some(AgentStatusReport {
                    state: AgentState::NeedsInput,
                    detail: None,
                    ttl_seconds: None,
                }),
            }
        );
        assert_eq!(request.operation(), "agent_status_set");
        let request = resolve_agent_status_request(
            parse(&[
                "set",
                "needs-input",
                "--detail",
                "waiting for approval",
                "--ttl",
                "30",
            ]),
            &inside,
        )
        .unwrap();
        let AgentStatusRequest::Report {
            status: Some(status),
            ..
        } = request
        else {
            panic!("set resolves to a report");
        };
        assert_eq!(status.detail.as_deref(), Some("waiting for approval"));
        assert_eq!(status.ttl_seconds, Some(30));
        let clear = resolve_agent_status_request(parse(&["clear"]), &inside).unwrap();
        assert_eq!(clear.operation(), "agent_status_clear");
        let get = resolve_agent_status_request(parse(&["get"]), &inside).unwrap();
        assert_eq!(
            get,
            AgentStatusRequest::Get {
                splint_id: SPLINT.parse().unwrap()
            }
        );
    }

    #[test]
    fn explicit_flags_override_the_environment() {
        let inside = environment(&[(SPLINT_ID_ENV, SPLINT), (SPLINT_INCARNATION_ENV, "3")]);
        let request = resolve_agent_status_request(
            parse(&["set", "done", "--splint", OTHER, "--incarnation", "7"]),
            &inside,
        )
        .unwrap();
        assert!(matches!(
            request,
            AgentStatusRequest::Report { splint_id, incarnation: 7, .. } if splint_id == OTHER.parse().unwrap()
        ));
        let get = resolve_agent_status_request(parse(&["get", OTHER]), &inside).unwrap();
        assert_eq!(
            get,
            AgentStatusRequest::Get {
                splint_id: OTHER.parse().unwrap()
            }
        );
        // Flags work outside a Splint.
        let outside = environment(&[]);
        assert!(
            resolve_agent_status_request(
                parse(&["clear", "--splint", OTHER, "--incarnation", "1"]),
                &outside
            )
            .is_ok()
        );
    }

    #[test]
    fn outside_a_splint_is_a_distinct_usage_error() {
        let outside = environment(&[]);
        assert_eq!(
            resolve_agent_status_request(parse(&["set", "working"]), &outside),
            Err(NOT_INSIDE_SPLINT_REPORT)
        );
        assert_eq!(
            resolve_agent_status_request(parse(&["clear"]), &outside),
            Err(NOT_INSIDE_SPLINT_REPORT)
        );
        assert_eq!(
            resolve_agent_status_request(parse(&["get"]), &outside),
            Err(NOT_INSIDE_SPLINT_GET)
        );
        let empty = environment(&[(SPLINT_ID_ENV, ""), (SPLINT_INCARNATION_ENV, "")]);
        assert_eq!(
            resolve_agent_status_request(parse(&["set", "idle"]), &empty),
            Err(NOT_INSIDE_SPLINT_REPORT)
        );
        for partial in [
            environment(&[(SPLINT_ID_ENV, SPLINT)]),
            environment(&[(SPLINT_INCARNATION_ENV, "3")]),
            environment(&[
                (SPLINT_ID_ENV, "caller-supplied"),
                (SPLINT_INCARNATION_ENV, "3"),
            ]),
            environment(&[(SPLINT_ID_ENV, SPLINT), (SPLINT_INCARNATION_ENV, "0")]),
        ] {
            let error =
                resolve_agent_status_request(parse(&["set", "idle"]), &partial).unwrap_err();
            assert_eq!(error, MALFORMED_SPLINT_ENVIRONMENT);
            assert_ne!(error, NOT_INSIDE_SPLINT_REPORT);
        }
    }

    #[test]
    fn grammar_rejects_unknown_states_partial_targets_and_unsafe_detail() {
        for arguments in [
            vec!["set", "thinking"],
            vec!["set", "working", "--splint", SPLINT],
            vec!["set", "working", "--incarnation", "2"],
            vec!["set", "working", "--incarnation", "0", "--splint", SPLINT],
            vec!["set", "working", "--ttl", "9"],
            vec!["set", "working", "--ttl", "901"],
            vec!["set", "working", "--detail", ""],
            vec!["set", "working", "--detail", "two\nlines"],
            vec!["set", "working", "--detail", "bidi\u{202e}text"],
            vec!["clear", "--splint", SPLINT],
            vec!["clear", "--detail", "x"],
            vec!["get", "not-a-uuid"],
        ] {
            let mut argv = vec!["splinterm", "agent-status"];
            argv.extend(arguments.iter().copied());
            assert!(Cli::try_parse_from(&argv).is_err(), "{arguments:?}");
        }
        let long = "x".repeat(81);
        assert!(
            Cli::try_parse_from([
                "splinterm",
                "agent-status",
                "set",
                "done",
                "--detail",
                &long
            ])
            .is_err()
        );
    }

    #[test]
    fn human_status_line_is_readable_without_colour() {
        let splint_id: SplintId = SPLINT.parse().unwrap();
        assert_eq!(
            status_line(splint_id, Some(2), None),
            format!("Splint {SPLINT}: no agent status.")
        );
        let status = AgentStatusSummary {
            state: AgentState::NeedsInput,
            detail: Some("waiting for approval".into()),
            sequence: 4,
            stale: true,
            reported_age_ms: 121_400,
        };
        assert_eq!(
            status_line(splint_id, Some(2), Some(&status)),
            format!(
                "Splint {SPLINT} incarnation 2: Needs input · waiting for approval · stale (reported 121s ago)."
            )
        );
    }
}
