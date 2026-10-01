//! Launch-time clanker session metadata inherited by a harness process.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};

use serde::Serialize;

use crate::config::{DomainName, InvalidName};
use crate::launch::{
    SESSION_ACTIVE_ENVIRONMENT, SESSION_CONTEXT_ENVIRONMENT, SESSION_CONTEXT_SOURCE_ENVIRONMENT,
    SESSION_DOMAIN_SOURCE_ENVIRONMENT, SESSION_DOMAINS_ENVIRONMENT, SESSION_FAMILY_ENVIRONMENT,
    SESSION_HARNESS_ENVIRONMENT, SESSION_ID_ENVIRONMENT, SESSION_INVOCATION_ENVIRONMENT,
    SESSION_MARKER_VALUE, SESSION_MODEL_ENVIRONMENT, SESSION_MODEL_SOURCE_ENVIRONMENT,
    SESSION_PROJECT_ENVIRONMENT, SESSION_PROJECT_SOURCE_ENVIRONMENT, SESSION_REMOTE_ENVIRONMENT,
    SESSION_VERSION, SESSION_VERSION_ENVIRONMENT,
};

/// Launch metadata reported by `clanker current`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CurrentSession {
    /// Whether marker-capable clanker launched the current process.
    pub active: bool,
    /// Marker contract version.
    pub version: Option<String>,
    /// Identifier minted for this launch.
    pub id: Option<String>,
    /// Invocation string used to create the launch plan.
    pub invocation: Option<String>,
    /// Selected harness name.
    pub harness: Option<String>,
    /// Resolved context name.
    pub context: Option<String>,
    /// Precedence tier that selected the context.
    pub context_source: Option<String>,
    /// Resolved domains, when command mode selected them.
    pub domains: Option<Vec<String>>,
    /// Precedence tier that selected the domain stack.
    pub domain_source: Option<String>,
    /// Resolved model, when one was applied.
    pub model: Option<String>,
    /// Precedence tier that selected the model.
    pub model_source: Option<String>,
    /// Resolved prompt family, when command mode selected one.
    pub family: Option<String>,
    /// Resolved project slug, when one was resolved.
    pub project: Option<String>,
    /// Precedence tier that supplied the project slug.
    pub project_source: Option<String>,
    /// Normalized origin remote of the launch directory, when one exists.
    pub remote: Option<String>,
}

impl CurrentSession {
    /// Parse inherited clanker markers without consulting current configuration.
    ///
    /// # Errors
    /// Returns [`SessionError`] when present markers are malformed, unsupported,
    /// or not valid UTF-8.
    pub fn from_environment(
        environment: &BTreeMap<OsString, OsString>,
    ) -> Result<Self, SessionError> {
        let marker = optional_value(environment, SESSION_ACTIVE_ENVIRONMENT)?;
        let Some(marker) = marker else {
            return Ok(Self::inactive());
        };
        if marker != SESSION_MARKER_VALUE {
            return Err(SessionError::InvalidMarker(marker));
        }

        let version = required_value(environment, SESSION_VERSION_ENVIRONMENT)?;
        if version != SESSION_VERSION {
            return Err(SessionError::UnsupportedVersion(version));
        }

        Ok(Self {
            active: true,
            version: Some(version),
            id: Some(required_value(environment, SESSION_ID_ENVIRONMENT)?),
            invocation: Some(required_value(environment, SESSION_INVOCATION_ENVIRONMENT)?),
            harness: Some(required_value(environment, SESSION_HARNESS_ENVIRONMENT)?),
            context: Some(required_value(environment, SESSION_CONTEXT_ENVIRONMENT)?),
            context_source: Some(required_value(
                environment,
                SESSION_CONTEXT_SOURCE_ENVIRONMENT,
            )?),
            domains: optional_domains(environment)?,
            domain_source: optional_value(environment, SESSION_DOMAIN_SOURCE_ENVIRONMENT)?,
            model: optional_value(environment, SESSION_MODEL_ENVIRONMENT)?,
            model_source: optional_value(environment, SESSION_MODEL_SOURCE_ENVIRONMENT)?,
            family: optional_value(environment, SESSION_FAMILY_ENVIRONMENT)?,
            project: optional_value(environment, SESSION_PROJECT_ENVIRONMENT)?,
            project_source: optional_value(environment, SESSION_PROJECT_SOURCE_ENVIRONMENT)?,
            remote: optional_value(environment, SESSION_REMOTE_ENVIRONMENT)?,
        })
    }

    const fn inactive() -> Self {
        Self {
            active: false,
            version: None,
            id: None,
            invocation: None,
            harness: None,
            context: None,
            context_source: None,
            domains: None,
            domain_source: None,
            model: None,
            model_source: None,
            family: None,
            project: None,
            project_source: None,
            remote: None,
        }
    }
}

/// Invalid inherited clanker session metadata.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SessionError {
    /// A marker value is not valid UTF-8.
    #[error("environment variable {0} is not valid UTF-8")]
    NonUtf8(&'static str),
    /// The active marker has an unexpected value.
    #[error("{SESSION_ACTIVE_ENVIRONMENT} must be `{SESSION_MARKER_VALUE}`, not `{0}`")]
    InvalidMarker(String),
    /// An active session lacks a required marker.
    #[error("active clanker session is missing {0}")]
    Missing(&'static str),
    /// The marker contract version is not supported by this binary.
    #[error("unsupported clanker session marker version `{0}`")]
    UnsupportedVersion(String),
    /// The ordered domain marker contains an invalid domain name.
    #[error("invalid domain stack `{value}` from {SESSION_DOMAINS_ENVIRONMENT}: {error}")]
    InvalidDomains {
        /// Full rejected marker value.
        value: String,
        /// Name parse failure.
        error: InvalidName,
    },
}

fn optional_domains(
    environment: &BTreeMap<OsString, OsString>,
) -> Result<Option<Vec<String>>, SessionError> {
    let Some(value) = optional_value(environment, SESSION_DOMAINS_ENVIRONMENT)? else {
        return Ok(None);
    };
    value
        .split(',')
        .map(|domain| {
            DomainName::new(domain)
                .map(|domain| domain.to_string())
                .map_err(|error| SessionError::InvalidDomains {
                    value: value.clone(),
                    error,
                })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn optional_value(
    environment: &BTreeMap<OsString, OsString>,
    name: &'static str,
) -> Result<Option<String>, SessionError> {
    environment
        .get(OsStr::new(name))
        .map(|value| {
            value
                .to_str()
                .map(str::to_string)
                .ok_or(SessionError::NonUtf8(name))
        })
        .transpose()
        .map(|value| value.filter(|value| !value.is_empty()))
}

fn required_value(
    environment: &BTreeMap<OsString, OsString>,
    name: &'static str,
) -> Result<String, SessionError> {
    optional_value(environment, name)?.ok_or(SessionError::Missing(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixed launch identifier used by the marker fixtures below.
    const TEST_SESSION_ID: &str = "01a06ebb-0000-7000-8000-000000000000";

    fn environment(values: &[(&str, &str)]) -> BTreeMap<OsString, OsString> {
        values
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect()
    }

    #[test]
    fn absent_marker_reports_inactive_session() {
        let session = CurrentSession::from_environment(&BTreeMap::new()).unwrap();

        assert!(!session.active);
        assert_eq!(session.context, None);
        assert_eq!(session.domains, None);
    }

    #[test]
    fn parses_active_session_and_empty_optional_markers() {
        let session = CurrentSession::from_environment(&environment(&[
            (SESSION_ACTIVE_ENVIRONMENT, SESSION_MARKER_VALUE),
            (SESSION_VERSION_ENVIRONMENT, SESSION_VERSION),
            (SESSION_ID_ENVIRONMENT, TEST_SESSION_ID),
            (SESSION_INVOCATION_ENVIRONMENT, "clanker claude"),
            (SESSION_HARNESS_ENVIRONMENT, "claude"),
            (SESSION_CONTEXT_ENVIRONMENT, "work"),
            (SESSION_CONTEXT_SOURCE_ENVIRONMENT, ".clanker"),
            (SESSION_DOMAINS_ENVIRONMENT, "eng,research"),
            (SESSION_DOMAIN_SOURCE_ENVIRONMENT, "--domain"),
            (SESSION_MODEL_ENVIRONMENT, ""),
            (SESSION_MODEL_SOURCE_ENVIRONMENT, ""),
            (SESSION_FAMILY_ENVIRONMENT, "claude"),
        ]))
        .unwrap();

        assert!(session.active);
        assert_eq!(session.id.as_deref(), Some(TEST_SESSION_ID));
        assert_eq!(session.context.as_deref(), Some("work"));
        assert_eq!(
            session.domains,
            Some(vec!["eng".to_string(), "research".to_string()])
        );
        assert_eq!(session.domain_source.as_deref(), Some("--domain"));
        assert_eq!(session.model, None);
        assert_eq!(session.model_source, None);
    }

    #[test]
    fn active_session_requires_complete_supported_markers() {
        let missing_id = CurrentSession::from_environment(&environment(&[
            (SESSION_ACTIVE_ENVIRONMENT, SESSION_MARKER_VALUE),
            (SESSION_VERSION_ENVIRONMENT, SESSION_VERSION),
        ]))
        .unwrap_err();
        assert_eq!(missing_id, SessionError::Missing(SESSION_ID_ENVIRONMENT));

        let missing = CurrentSession::from_environment(&environment(&[
            (SESSION_ACTIVE_ENVIRONMENT, SESSION_MARKER_VALUE),
            (SESSION_VERSION_ENVIRONMENT, SESSION_VERSION),
            (SESSION_ID_ENVIRONMENT, TEST_SESSION_ID),
        ]))
        .unwrap_err();
        assert_eq!(
            missing,
            SessionError::Missing(SESSION_INVOCATION_ENVIRONMENT)
        );

        let invalid = CurrentSession::from_environment(&environment(&[
            (SESSION_ACTIVE_ENVIRONMENT, SESSION_MARKER_VALUE),
            (SESSION_VERSION_ENVIRONMENT, "2"),
        ]))
        .unwrap_err();
        assert_eq!(invalid, SessionError::UnsupportedVersion("2".to_string()));
    }

    #[test]
    fn rejects_invalid_domain_stack_markers() {
        let invalid = CurrentSession::from_environment(&environment(&[
            (SESSION_ACTIVE_ENVIRONMENT, SESSION_MARKER_VALUE),
            (SESSION_VERSION_ENVIRONMENT, SESSION_VERSION),
            (SESSION_ID_ENVIRONMENT, TEST_SESSION_ID),
            (SESSION_INVOCATION_ENVIRONMENT, "clanker claude"),
            (SESSION_HARNESS_ENVIRONMENT, "claude"),
            (SESSION_CONTEXT_ENVIRONMENT, "work"),
            (SESSION_CONTEXT_SOURCE_ENVIRONMENT, ".clanker"),
            (SESSION_DOMAINS_ENVIRONMENT, "eng,,research"),
        ]))
        .unwrap_err();

        assert!(matches!(invalid, SessionError::InvalidDomains { .. }));
    }

    #[test]
    fn wrong_marker_value_is_rejected() {
        let error = CurrentSession::from_environment(&environment(&[(
            SESSION_ACTIVE_ENVIRONMENT,
            "counterfeit",
        )]))
        .unwrap_err();
        assert_eq!(
            error,
            SessionError::InvalidMarker("counterfeit".to_string())
        );
    }

    #[test]
    fn parses_resolved_project_markers() {
        let session = CurrentSession::from_environment(&environment(&[
            (SESSION_ACTIVE_ENVIRONMENT, SESSION_MARKER_VALUE),
            (SESSION_VERSION_ENVIRONMENT, SESSION_VERSION),
            (SESSION_ID_ENVIRONMENT, TEST_SESSION_ID),
            (SESSION_INVOCATION_ENVIRONMENT, "clanker claude"),
            (SESSION_HARNESS_ENVIRONMENT, "claude"),
            (SESSION_CONTEXT_ENVIRONMENT, "work"),
            (SESSION_CONTEXT_SOURCE_ENVIRONMENT, ".clanker"),
            (SESSION_PROJECT_ENVIRONMENT, "kb"),
            (SESSION_PROJECT_SOURCE_ENVIRONMENT, "declared"),
            (SESSION_REMOTE_ENVIRONMENT, "github.com/tftio/kb"),
        ]))
        .unwrap();

        assert_eq!(session.project.as_deref(), Some("kb"));
        assert_eq!(session.project_source.as_deref(), Some("declared"));
        assert_eq!(session.remote.as_deref(), Some("github.com/tftio/kb"));
    }

    #[test]
    fn active_session_without_project_markers_reports_no_project() {
        let session = CurrentSession::from_environment(&environment(&[
            (SESSION_ACTIVE_ENVIRONMENT, SESSION_MARKER_VALUE),
            (SESSION_VERSION_ENVIRONMENT, SESSION_VERSION),
            (SESSION_ID_ENVIRONMENT, TEST_SESSION_ID),
            (SESSION_INVOCATION_ENVIRONMENT, "clanker claude"),
            (SESSION_HARNESS_ENVIRONMENT, "claude"),
            (SESSION_CONTEXT_ENVIRONMENT, "work"),
            (SESSION_CONTEXT_SOURCE_ENVIRONMENT, ".clanker"),
        ]))
        .unwrap();

        assert_eq!(session.project, None);
        assert_eq!(session.project_source, None);
        assert_eq!(session.remote, None);
    }

    #[test]
    fn active_session_without_domain_marker_reports_no_domains() {
        let session = CurrentSession::from_environment(&environment(&[
            (SESSION_ACTIVE_ENVIRONMENT, SESSION_MARKER_VALUE),
            (SESSION_VERSION_ENVIRONMENT, SESSION_VERSION),
            (SESSION_ID_ENVIRONMENT, TEST_SESSION_ID),
            (SESSION_INVOCATION_ENVIRONMENT, "claude-launch"),
            (SESSION_HARNESS_ENVIRONMENT, "claude"),
            (SESSION_CONTEXT_ENVIRONMENT, "work"),
            (SESSION_CONTEXT_SOURCE_ENVIRONMENT, "CLANKER_CONTEXT"),
        ]))
        .unwrap();

        assert!(session.active);
        assert_eq!(session.domains, None);
        assert_eq!(session.domain_source, None);
        assert_eq!(session.context_source.as_deref(), Some("CLANKER_CONTEXT"));
    }

    #[test]
    fn a_non_utf8_marker_is_rejected() {
        use std::os::unix::ffi::OsStringExt;
        let mut environment = BTreeMap::new();
        environment.insert(
            OsString::from(SESSION_ACTIVE_ENVIRONMENT),
            OsString::from_vec(vec![0xff]),
        );

        let error = CurrentSession::from_environment(&environment).unwrap_err();
        assert_eq!(error, SessionError::NonUtf8(SESSION_ACTIVE_ENVIRONMENT));
    }
}
