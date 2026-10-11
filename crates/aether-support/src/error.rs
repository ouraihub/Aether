//! The single error type every aether crate returns (docs/10 §1, docs/04 §3).

use std::error::Error as StdError;
use std::fmt;

use serde::Serialize;
use ts_rs::TS;

use crate::{id, mask};

/// Stable error codes. The UI translates by code, never by message (docs/00 §7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum KitErrorCode {
    /// Input failed validation.
    BadRequest,
    /// The referenced entity does not exist.
    NotFound,
    /// The request conflicts with current state (revision, name, port).
    Conflict,
    /// The platform or build does not support the operation.
    Unsupported,
    /// The operation was cancelled.
    Cancelled,
    /// Upstream rejected the credentials.
    Unauthorized,
    /// The caller lacks the permission or scope.
    Forbidden,
    /// Upstream asked us to slow down.
    RateLimited,
    /// Upstream failed with an unexpected status.
    Upstream,
    /// A deadline elapsed.
    Timeout,
    /// The network could not be reached.
    Network,
    /// Something must be configured first (missing key, model, ...).
    NeedsConfig,
    /// The request does not fit the token or size budget.
    BudgetExceeded,
    /// A database was written by a newer version.
    IncompatibleDb,
    /// A bug or unexpected failure; details are only in the log.
    Internal,
}

/// Whitelisted context sent to the UI. Anything else stays in the diagnostic log.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, optional_fields)]
pub struct Details {
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<f64>")]
    limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    entity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_id: Option<String>,
}

impl Details {
    /// The offending input field.
    #[must_use]
    pub fn field(&self) -> Option<&str> {
        self.field.as_deref()
    }
    /// A machine-readable sub-reason, e.g. `duplicate_name`.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
    /// The upstream HTTP status.
    #[must_use]
    pub const fn status(&self) -> Option<u16> {
        self.status
    }
    /// The exceeded limit.
    #[must_use]
    pub const fn limit(&self) -> Option<u64> {
        self.limit
    }
    /// The kind of entity that was not found or conflicted.
    #[must_use]
    pub fn entity(&self) -> Option<&str> {
        self.entity.as_deref()
    }
    /// The port that was busy.
    #[must_use]
    pub const fn port(&self) -> Option<u16> {
        self.port
    }
    /// The id to look up in the log for an `internal` error.
    #[must_use]
    pub fn error_id(&self) -> Option<&str> {
        self.error_id.as_deref()
    }
    const fn is_empty(&self) -> bool {
        self.field.is_none()
            && self.reason.is_none()
            && self.status.is_none()
            && self.limit.is_none()
            && self.entity.is_none()
            && self.port.is_none()
            && self.error_id.is_none()
    }
}

/// Longest upstream body excerpt shown to the user, in characters.
const UPSTREAM_EXCERPT_CHARS: usize = 300;

/// A failure crossing a aether boundary. Serializes to `{ code, message, details? }`;
/// the cause chain is logged by whoever handles the error and never serialized.
#[derive(Debug, TS)]
#[ts(as = "KitErrorShape")]
pub struct KitError(Box<Inner>);

/// A failure crossing a aether boundary. Serializes to `{ code, message, details? }`;
/// the cause chain is logged by whoever handles the error and never serialized.
// Same doc as `KitError` on purpose: ts-rs writes KitError.ts from either type (its own
// export, or a dependency export through `as`), and parallel tests race on which one wins.
#[doc(hidden)]
#[derive(TS)]
#[ts(export, rename = "KitError")]
pub struct KitErrorShape {
    /// Error code.
    pub code: KitErrorCode,
    /// Safe English fallback text.
    pub message: String,
    /// Omitted when empty.
    #[ts(optional)]
    pub details: Option<Details>,
}

// Boxed so `Result<T, KitError>` stays one pointer wide on the happy path.
#[derive(Debug)]
struct Inner {
    code: KitErrorCode,
    message: String,
    details: Details,
    cause: Option<Box<dyn StdError + Send + Sync>>,
}

impl KitError {
    /// An error with a code and a safe English fallback message.
    #[must_use]
    pub fn new(code: KitErrorCode, message: impl Into<String>) -> Self {
        Self(Box::new(Inner {
            code,
            message: message.into(),
            details: Details::default(),
            cause: None,
        }))
    }

    /// Invalid input in `field`; `reason` is a stable machine-readable token.
    #[must_use]
    pub fn bad_request(field: &str, reason: &str) -> Self {
        Self::new(
            KitErrorCode::BadRequest,
            format!("invalid {field}: {reason}"),
        )
        .with_field(field)
        .with_reason(reason)
    }

    /// `entity` does not exist.
    #[must_use]
    pub fn not_found(entity: &str) -> Self {
        Self::new(KitErrorCode::NotFound, format!("{entity} not found")).with_entity(entity)
    }

    /// The request conflicts with current state.
    #[must_use]
    pub fn conflict(reason: &str) -> Self {
        Self::new(KitErrorCode::Conflict, format!("conflict: {reason}")).with_reason(reason)
    }

    /// Something must be configured before this can run.
    #[must_use]
    pub fn needs_config(reason: &str) -> Self {
        Self::new(
            KitErrorCode::NeedsConfig,
            format!("needs configuration: {reason}"),
        )
        .with_reason(reason)
    }

    /// The current platform or build cannot do this.
    #[must_use]
    pub fn unsupported(reason: &str) -> Self {
        Self::new(KitErrorCode::Unsupported, format!("unsupported: {reason}")).with_reason(reason)
    }

    /// The operation was cancelled.
    #[must_use]
    pub fn cancelled() -> Self {
        Self::new(KitErrorCode::Cancelled, "cancelled")
    }

    /// An upstream non-success response. `body` is masked, whitespace-folded and
    /// truncated, and `known_secrets` are replaced, so it is safe to show (docs/10 §1).
    #[must_use]
    pub fn upstream(status: u16, body: &str, known_secrets: &[&str]) -> Self {
        let code = match status {
            401 | 403 => KitErrorCode::Unauthorized,
            429 => KitErrorCode::RateLimited,
            _ => KitErrorCode::Upstream,
        };
        let excerpt = upstream_excerpt(body, known_secrets);
        let message = if excerpt.is_empty() {
            format!("upstream returned {status}")
        } else {
            format!("{status}: {excerpt}")
        };
        Self::new(code, message).with_status(status)
    }

    /// A bug or unexpected failure. Logs the cause under a fresh error id and keeps
    /// only that id in the user-visible message.
    #[must_use]
    pub fn internal(cause: impl StdError + Send + Sync + 'static) -> Self {
        let error_id = id::short_id();
        tracing::error!(error_id = %error_id, cause = %cause, "aether.internal");
        let mut error = Self::new(KitErrorCode::Internal, format!("internal error {error_id}"));
        error.0.details.error_id = Some(error_id);
        error.0.cause = Some(Box::new(cause));
        error
    }

    /// Attaches the underlying cause for diagnostics. Never serialized.
    #[must_use]
    pub fn with_cause(mut self, cause: impl StdError + Send + Sync + 'static) -> Self {
        self.0.cause = Some(Box::new(cause));
        self
    }

    /// Sets `details.field`.
    #[must_use]
    pub fn with_field(mut self, field: &str) -> Self {
        self.0.details.field = Some(field.to_owned());
        self
    }

    /// Sets `details.reason`.
    #[must_use]
    pub fn with_reason(mut self, reason: &str) -> Self {
        self.0.details.reason = Some(reason.to_owned());
        self
    }

    /// Sets `details.status`.
    #[must_use]
    pub fn with_status(mut self, status: u16) -> Self {
        self.0.details.status = Some(status);
        self
    }

    /// Sets `details.limit`.
    #[must_use]
    pub fn with_limit(mut self, limit: u64) -> Self {
        self.0.details.limit = Some(limit);
        self
    }

    /// Sets `details.entity`.
    #[must_use]
    pub fn with_entity(mut self, entity: &str) -> Self {
        self.0.details.entity = Some(entity.to_owned());
        self
    }

    /// Sets `details.port`.
    #[must_use]
    pub fn with_port(mut self, port: u16) -> Self {
        self.0.details.port = Some(port);
        self
    }

    /// The stable code.
    #[must_use]
    pub fn code(&self) -> KitErrorCode {
        self.0.code
    }

    /// The safe English fallback message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.0.message
    }

    /// The whitelisted details.
    #[must_use]
    pub fn details(&self) -> &Details {
        &self.0.details
    }
}

impl fmt::Display for KitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.message)
    }
}

impl StdError for KitError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.0
            .cause
            .as_deref()
            .map(|cause| cause as &(dyn StdError + 'static))
    }
}

impl Serialize for KitError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let has_details = !self.0.details.is_empty();
        let mut state = serializer.serialize_struct("KitError", if has_details { 3 } else { 2 })?;
        state.serialize_field("code", &self.0.code)?;
        state.serialize_field("message", &self.0.message)?;
        if has_details {
            state.serialize_field("details", &self.0.details)?;
        }
        state.end()
    }
}

impl From<std::io::Error> for KitError {
    fn from(error: std::io::Error) -> Self {
        Self::internal(error)
    }
}

impl From<rusqlite::Error> for KitError {
    fn from(error: rusqlite::Error) -> Self {
        Self::internal(error)
    }
}

/// `body` whitespace-folded, with `known_secrets` and credential-looking tokens masked,
/// cut to 300 characters: safe to show as upstream text (docs/00 §8).
#[must_use]
pub fn upstream_excerpt(body: &str, known_secrets: &[&str]) -> String {
    let folded = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let redacted = mask::redact_secrets(&mask::redact(&folded, known_secrets));
    match redacted.char_indices().nth(UPSTREAM_EXCERPT_CHARS) {
        Some((cut, _)) => format!("{}…", &redacted[..cut]),
        None => redacted,
    }
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]
    use super::*;

    #[test]
    fn serializes_code_message_and_only_set_details() {
        let json = serde_json::to_value(KitError::bad_request("baseUrl", "not_https")).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "code": "bad_request",
                "message": "invalid baseUrl: not_https",
                "details": { "field": "baseUrl", "reason": "not_https" }
            })
        );
        let json = serde_json::to_value(KitError::cancelled()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "code": "cancelled", "message": "cancelled" })
        );
    }

    #[test]
    fn cause_is_kept_for_diagnostics_but_never_serialized() {
        let io = std::io::Error::other("C:\\Users\\alice\\secret.db is locked");
        let error = KitError::not_found("model").with_cause(io);
        assert!(error.source().is_some());
        let text = serde_json::to_string(&error).unwrap();
        assert!(!text.contains("alice"), "{text}");
    }

    #[test]
    fn internal_exposes_only_an_error_id() {
        let error = KitError::internal(std::io::Error::other("disk path D:\\private"));
        let id = error.details().error_id().unwrap().to_owned();
        assert_eq!(error.code(), KitErrorCode::Internal);
        assert_eq!(error.message(), format!("internal error {id}"));
        assert!(!serde_json::to_string(&error).unwrap().contains("private"));
    }

    #[test]
    fn upstream_maps_status_and_scrubs_the_body() {
        let body = "{\"error\":\n  \"bad key sk-ant-REALSECRETVALUE1234567890 for user\"}";
        let error = KitError::upstream(401, body, &[]);
        assert_eq!(error.code(), KitErrorCode::Unauthorized);
        assert_eq!(error.details().status(), Some(401));
        assert!(
            !error.message().contains("REALSECRET"),
            "{}",
            error.message()
        );
        assert!(!error.message().contains('\n'));

        assert_eq!(
            KitError::upstream(429, "", &[]).code(),
            KitErrorCode::RateLimited
        );
        assert_eq!(
            KitError::upstream(503, "", &[]).message(),
            "upstream returned 503"
        );
        let known = KitError::upstream(500, "token abc-relay-key rejected", &["abc-relay-key"]);
        assert_eq!(known.message(), "500: token *** rejected");
    }

    #[test]
    fn upstream_excerpt_is_truncated_on_a_char_boundary() {
        let error = KitError::upstream(500, &"错".repeat(400), &[]);
        let excerpt = error.message().trim_start_matches("500: ");
        assert_eq!(excerpt.chars().count(), UPSTREAM_EXCERPT_CHARS + 1);
        assert!(excerpt.ends_with('…'));
    }
}
