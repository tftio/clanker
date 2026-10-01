//! One precedence rule shared by every launch selection axis.
//!
//! Context, domain, and model all resolve the same way: an explicit flag, then
//! the nearest `.clanker` at or above the working directory, then a
//! `CLANKER_<AXIS>` environment variable, then a configured default. Each axis
//! reports which tier supplied its value so a launch can be explained after the
//! fact rather than re-derived.

use crate::config::InvalidName;

/// Static naming for one selection axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Axis {
    /// Axis label used in error messages.
    pub name: &'static str,
    /// Command-line flag that selects this axis.
    pub flag: &'static str,
    /// Environment variable consulted for this axis.
    pub environment: &'static str,
    /// Label describing where the configured default comes from.
    pub configured_default: &'static str,
}

/// The context axis.
pub const CONTEXT: Axis = Axis {
    name: "context",
    flag: "--context",
    environment: "CLANKER_CONTEXT",
    configured_default: "none",
};

/// The domain axis.
pub const DOMAIN: Axis = Axis {
    name: "domain",
    flag: "--domain",
    environment: "CLANKER_DOMAIN",
    configured_default: "defaults.domain",
};

/// The model axis.
pub const MODEL: Axis = Axis {
    name: "model",
    flag: "--model",
    environment: "CLANKER_MODEL",
    configured_default: "domain.default_model",
};

/// The precedence tier that supplied an axis value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisSource {
    /// An explicit command-line flag.
    CommandLine,
    /// The nearest `.clanker` at or above the working directory.
    ClankerFile,
    /// The axis environment variable.
    Environment,
    /// A default drawn from runtime configuration.
    ConfiguredDefault,
}

impl AxisSource {
    /// Stable label used in session markers, JSON output, and errors.
    #[must_use]
    pub const fn label(self, axis: Axis) -> &'static str {
        match self {
            Self::CommandLine => axis.flag,
            Self::ClankerFile => ".clanker",
            Self::Environment => axis.environment,
            Self::ConfiguredDefault => axis.configured_default,
        }
    }
}

/// An axis value paired with the tier that supplied it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved<T> {
    /// The resolved value.
    pub value: T,
    /// The winning precedence tier.
    pub source: AxisSource,
}

/// An axis environment variable holds a value that is not a valid name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {axis} from {origin}: {error}")]
pub struct InvalidAxisValue {
    /// Axis label.
    pub axis: &'static str,
    /// Variable that supplied the value.
    pub origin: &'static str,
    /// Name validation failure.
    pub error: InvalidName,
}

/// Resolve one axis through the shared precedence chain.
///
/// Tiers are tried in order: `command_line`, `directory`, `environment`, then
/// `configured_default`. An empty or whitespace-only environment value falls
/// through rather than resolving. Returns `Ok(None)` when no tier supplies a
/// value, which the caller may treat as a default or as an error.
///
/// # Errors
/// Returns [`InvalidAxisValue`] when the environment tier holds a value that is
/// not a valid name for this axis.
pub fn resolve<T, F>(
    axis: Axis,
    command_line: Option<T>,
    directory: Option<T>,
    environment: Option<&str>,
    parse: F,
    configured_default: Option<T>,
) -> Result<Option<Resolved<T>>, InvalidAxisValue>
where
    F: FnOnce(String) -> Result<T, InvalidName>,
{
    if let Some(value) = command_line {
        return Ok(Some(Resolved {
            value,
            source: AxisSource::CommandLine,
        }));
    }
    if let Some(value) = directory {
        return Ok(Some(Resolved {
            value,
            source: AxisSource::ClankerFile,
        }));
    }
    if let Some(text) = environment.map(str::trim).filter(|text| !text.is_empty()) {
        let value = parse(text.to_string()).map_err(|error| InvalidAxisValue {
            axis: axis.name,
            origin: axis.environment,
            error,
        })?;
        return Ok(Some(Resolved {
            value,
            source: AxisSource::Environment,
        }));
    }
    Ok(configured_default.map(|value| Resolved {
        value,
        source: AxisSource::ConfiguredDefault,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ContextName;

    fn parse(value: String) -> Result<ContextName, InvalidName> {
        ContextName::new(value)
    }

    fn name(value: &str) -> ContextName {
        ContextName::new(value).unwrap()
    }

    #[test]
    fn each_tier_wins_in_order_and_reports_itself() {
        let all = resolve(
            CONTEXT,
            Some(name("flag")),
            Some(name("file")),
            Some("environment"),
            parse,
            Some(name("default")),
        )
        .unwrap()
        .unwrap();
        assert_eq!(all.value.as_str(), "flag");
        assert_eq!(all.source, AxisSource::CommandLine);
        assert_eq!(all.source.label(CONTEXT), "--context");

        let file = resolve(
            CONTEXT,
            None,
            Some(name("file")),
            Some("environment"),
            parse,
            Some(name("default")),
        )
        .unwrap()
        .unwrap();
        assert_eq!(file.value.as_str(), "file");
        assert_eq!(file.source.label(CONTEXT), ".clanker");

        let environment = resolve(
            CONTEXT,
            None,
            None,
            Some("environment"),
            parse,
            Some(name("default")),
        )
        .unwrap()
        .unwrap();
        assert_eq!(environment.value.as_str(), "environment");
        assert_eq!(environment.source.label(CONTEXT), "CLANKER_CONTEXT");

        let configured = resolve(CONTEXT, None, None, None, parse, Some(name("default")))
            .unwrap()
            .unwrap();
        assert_eq!(configured.value.as_str(), "default");
        assert_eq!(configured.source, AxisSource::ConfiguredDefault);
    }

    #[test]
    fn an_empty_environment_value_falls_through() {
        let resolved = resolve(
            CONTEXT,
            None,
            None,
            Some("   "),
            parse,
            Some(name("fallback")),
        )
        .unwrap()
        .unwrap();
        assert_eq!(resolved.value.as_str(), "fallback");
        assert_eq!(resolved.source, AxisSource::ConfiguredDefault);
    }

    #[test]
    fn no_tier_and_no_default_resolves_to_nothing() {
        assert!(
            resolve(CONTEXT, None, None, None, parse, None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn an_invalid_environment_value_names_the_axis_and_variable() {
        let error = resolve(DOMAIN, None, None, Some("not a name"), parse, None).unwrap_err();
        assert_eq!(error.axis, "domain");
        assert_eq!(error.origin, "CLANKER_DOMAIN");
        assert!(
            error
                .to_string()
                .contains("invalid domain from CLANKER_DOMAIN")
        );
    }

    #[test]
    fn model_axis_labels_its_configured_default() {
        assert_eq!(
            AxisSource::ConfiguredDefault.label(MODEL),
            "domain.default_model"
        );
        assert_eq!(AxisSource::CommandLine.label(MODEL), "--model");
    }
}
