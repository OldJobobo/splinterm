use super::{
    CliEnvelopeV2, CliErrorCodeV2, ErrorCode, Result, protocol_error, write_json_document,
};

fn is_default_dojo_name_exhausted(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<splinterm_core::TopologyError>(),
            Some(splinterm_core::TopologyError::DefaultDojoNameExhausted)
        )
    })
}

pub(in crate::app) fn machine_exit_code(error: &anyhow::Error) -> i32 {
    if let Some(protocol) = protocol_error(error) {
        return match protocol.code {
            ErrorCode::ConsentUnavailable | ErrorCode::ConsentDenied | ErrorCode::Unauthorized => 3,
            ErrorCode::AuthenticationFailed
            | ErrorCode::HandshakeRequired
            | ErrorCode::IncompatibleVersion => 4,
            ErrorCode::Cancelled => 6,
            ErrorCode::Internal | ErrorCode::DevelopmentFeatureDisabled => 70,
            _ => 5,
        };
    }
    if is_default_dojo_name_exhausted(error) {
        return 5;
    }
    let message = error.to_string();
    if message.contains("requires --yes") {
        3
    } else if message.contains("timed out") || message.contains("deadline") {
        6
    } else if message.contains("unsupported schema")
        || message.contains("cannot connect")
        || message.contains("XDG_RUNTIME_DIR")
        || message.contains("handshake")
        || message.contains("protocol version")
    {
        4
    } else if message.contains("not found")
        || message.contains("invalid continuation cursor")
        || message.contains("does not match the selected")
        || message.contains("expected incarnation")
        || message.contains("does not have a live process")
        || message.contains("controller")
        || message.contains("resource limit")
    {
        5
    } else {
        70
    }
}

pub(super) fn write_machine_read_failure(
    operation: &'static str,
    code: CliErrorCodeV2,
    message: impl Into<String>,
    retryable: bool,
) -> Result<()> {
    write_json_document(&CliEnvelopeV2::failure(
        operation, code, message, retryable,
    )?)
}

pub(super) fn write_machine_connection_failure(
    operation: &'static str,
    error: &anyhow::Error,
) -> Result<()> {
    if let Some(protocol) = protocol_error(error) {
        return write_json_document(&CliEnvelopeV2::protocol_failure(
            operation,
            protocol,
            bounded_public_message(error),
        )?);
    }
    write_machine_read_failure(
        operation,
        CliErrorCodeV2::Internal,
        bounded_public_message(error),
        true,
    )
}

fn local_machine_failure(error: &anyhow::Error) -> (CliErrorCodeV2, bool) {
    if is_default_dojo_name_exhausted(error) {
        (CliErrorCodeV2::InvalidArgument, false)
    } else if error.to_string().contains("timed out") {
        (CliErrorCodeV2::Timeout, true)
    } else if error.to_string().contains("not found") {
        (CliErrorCodeV2::NotFound, false)
    } else if error.to_string().contains("expected incarnation")
        || error.to_string().contains("does not have a live process")
    {
        (CliErrorCodeV2::StaleIncarnation, false)
    } else {
        (CliErrorCodeV2::Internal, false)
    }
}

pub(super) fn finish_machine_envelope(
    operation: &'static str,
    result: Result<CliEnvelopeV2>,
) -> Result<()> {
    match result {
        Ok(envelope) => write_json_document(&envelope),
        Err(error) => {
            if let Some(protocol) = protocol_error(&error) {
                write_json_document(&CliEnvelopeV2::protocol_failure(
                    operation,
                    protocol,
                    bounded_public_message(&error),
                )?)?;
                return Err(error);
            }
            let (code, retryable) = local_machine_failure(&error);
            write_machine_read_failure(operation, code, bounded_public_message(&error), retryable)?;
            Err(error)
        }
    }
}

pub(super) fn bounded_public_message(error: &anyhow::Error) -> String {
    let message = error.to_string();
    if message.chars().count() <= 1024 {
        return message;
    }
    message
        .chars()
        .take(1023)
        .chain(std::iter::once('…'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhausted_default_dojo_number_is_a_recoverable_invalid_argument() {
        let error = anyhow::Error::new(splinterm_core::TopologyError::DefaultDojoNameExhausted);

        assert_eq!(machine_exit_code(&error), 5);
        assert_eq!(
            local_machine_failure(&error),
            (CliErrorCodeV2::InvalidArgument, false)
        );
    }
}
