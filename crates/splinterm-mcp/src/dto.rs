use serde::Serialize;

/// Closed, sanitized MCP v1 tool failure envelope.
#[derive(Debug, Serialize)]
pub(crate) struct ToolFailure<'a> {
    schema: &'static str,
    tool: &'a str,
    ok: bool,
    error: ToolError,
    truncated: bool,
    content_trust: &'static str,
}

#[derive(Debug, Serialize)]
struct ToolError {
    code: &'static str,
    message: &'static str,
    retryable: bool,
}

impl<'a> ToolFailure<'a> {
    pub(crate) fn unavailable(tool: &'a str) -> Self {
        Self::new(
            tool,
            "internal",
            "tool dispatch is not implemented in this server slice",
        )
    }

    pub(crate) fn invalid_argument(tool: &'a str) -> Self {
        Self::new(
            tool,
            "invalid_argument",
            "tool arguments do not match the advertised input schema",
        )
    }

    pub(crate) fn confirmation_required(tool: &'a str) -> Self {
        Self::new(
            tool,
            "confirmation_required",
            "explicit confirmation is required for this destructive tool",
        )
    }

    fn new(tool: &'a str, code: &'static str, message: &'static str) -> Self {
        Self {
            schema: "splinterm.mcp.v1",
            tool,
            ok: false,
            error: ToolError {
                code,
                message,
                retryable: false,
            },
            truncated: false,
            content_trust: "trusted_metadata",
        }
    }
}
