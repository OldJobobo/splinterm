use super::{
    CliErrorCodeV2, Connection, Request, Response, Result, SplintId, bail, bounded_public_message,
    graphical_focus_envelope, inspect_splint_envelope, inspect_topology_envelope,
    list_lairs_envelope, protocol_error, public_error_code, write_json_document,
    write_machine_connection_failure, write_machine_read_failure,
};

#[derive(Debug, Clone, Copy)]
pub(super) enum MachineRead {
    Focus,
    List,
    Topology,
    Splint(SplintId),
}

impl MachineRead {
    const fn operation(self) -> &'static str {
        match self {
            Self::Focus => "focus",
            Self::List => "list_lairs",
            Self::Topology => "inspect_topology",
            Self::Splint(_) => "inspect_splint",
        }
    }
}

pub(super) async fn run_machine_read(
    command: MachineRead,
    schema_major: u16,
    timeout_ms: u64,
) -> Result<()> {
    let operation = command.operation();
    if schema_major != 2 {
        write_machine_read_failure(
            operation,
            CliErrorCodeV2::UnsupportedSchema,
            format!("unsupported schema major {schema_major}"),
            false,
        )?;
        bail!("unsupported schema major {schema_major}");
    }

    let deadline = std::time::Duration::from_millis(timeout_ms);
    let started = std::time::Instant::now();
    let mut connection =
        match tokio::time::timeout(deadline, Connection::connect_automation()).await {
            Ok(Ok(connection)) => connection,
            Ok(Err(error)) => {
                if matches!(command, MachineRead::Focus) {
                    write_machine_read_failure(
                        operation,
                        CliErrorCodeV2::Internal,
                        "focus service unavailable",
                        true,
                    )?;
                } else {
                    write_machine_connection_failure(operation, &error)?;
                }
                return Err(error);
            }
            Err(_) => {
                write_machine_read_failure(
                    operation,
                    CliErrorCodeV2::Timeout,
                    "connection deadline elapsed",
                    true,
                )?;
                bail!("splinterd connection timed out");
            }
        };
    let remaining = deadline.saturating_sub(started.elapsed());
    let request = if matches!(command, MachineRead::Focus) {
        Request::ReadGraphicalFocus
    } else {
        Request::InspectTopology
    };
    let response = match connection.request_with_deadline(request, remaining).await {
        Ok(response) => response,
        Err(error) => {
            let (code, retryable) = if error.to_string().contains("timed out") {
                (CliErrorCodeV2::Timeout, true)
            } else if let Some(protocol) = protocol_error(&error) {
                public_error_code(protocol.code)
            } else {
                (CliErrorCodeV2::Internal, true)
            };
            let message = if matches!(command, MachineRead::Focus) {
                if code == CliErrorCodeV2::Timeout {
                    "focus request timed out".to_owned()
                } else {
                    "focus request failed".to_owned()
                }
            } else {
                bounded_public_message(&error)
            };
            write_machine_read_failure(operation, code, message, retryable)?;
            return Err(error);
        }
    };
    if let Response::GraphicalFocus {
        focused_splint_id,
        cwd,
    } = response
    {
        if !matches!(command, MachineRead::Focus) {
            write_machine_read_failure(
                operation,
                CliErrorCodeV2::Internal,
                "splinterd returned an unexpected focus response",
                false,
            )?;
            bail!("splinterd returned an unexpected focus response");
        }
        return match graphical_focus_envelope(focused_splint_id, cwd.as_deref()) {
            Ok(envelope) => write_json_document(&envelope),
            Err(error) => {
                write_machine_read_failure(
                    operation,
                    CliErrorCodeV2::Internal,
                    "focus response was invalid",
                    false,
                )?;
                Err(error)
            }
        };
    }
    if matches!(command, MachineRead::Focus) {
        write_machine_read_failure(
            operation,
            CliErrorCodeV2::Internal,
            "splinterd returned an unexpected focus response",
            false,
        )?;
        bail!("splinterd returned an unexpected focus response");
    }
    let Response::Topology { snapshot } = response else {
        write_machine_read_failure(
            operation,
            CliErrorCodeV2::Internal,
            "splinterd returned an unexpected read response",
            false,
        )?;
        bail!("splinterd returned an unexpected read response");
    };
    if let MachineRead::Splint(splint_id) = command
        && snapshot.topology.find_splint(splint_id).is_none()
    {
        write_machine_read_failure(
            operation,
            CliErrorCodeV2::NotFound,
            "requested Splint was not found",
            false,
        )?;
        bail!("requested Splint was not found");
    }
    let envelope = match command {
        MachineRead::Focus => unreachable!("focus responses return before topology projection"),
        MachineRead::List => list_lairs_envelope(&snapshot),
        MachineRead::Topology => inspect_topology_envelope(&snapshot),
        MachineRead::Splint(splint_id) => inspect_splint_envelope(&snapshot, splint_id),
    };
    match envelope {
        Ok(envelope) => write_json_document(&envelope),
        Err(error) => {
            write_machine_read_failure(
                operation,
                CliErrorCodeV2::Internal,
                bounded_public_message(&error),
                false,
            )?;
            Err(error)
        }
    }
}
