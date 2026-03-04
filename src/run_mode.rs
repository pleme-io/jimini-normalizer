use std::fmt;
use std::str::FromStr;

/// Run mode for the jimini-normalizer binary.
///
/// Set via the `RUN_MODE` environment variable (case-insensitive).
/// Follows the pleme-io convention from pleme-config::RunMode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// API server mode — serve HTTP requests, publish to NATS (default)
    Api,
    /// Migration mode — run database migrations and exit
    Migrate,
    /// Worker mode — consume NATS streams, persist to database
    Worker,
    /// Audit checker mode — query DB for anomalies, log findings, exit
    AuditChecker,
    /// Retention mode — purge expired records and exit
    Retention,
}

impl Default for RunMode {
    fn default() -> Self {
        Self::Api
    }
}

impl FromStr for RunMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "api" => Ok(Self::Api),
            "migrate" => Ok(Self::Migrate),
            "worker" => Ok(Self::Worker),
            "audit_checker" | "audit-checker" => Ok(Self::AuditChecker),
            "retention" => Ok(Self::Retention),
            _ => Err(format!(
                "invalid RUN_MODE '{}'. Valid values: api, migrate, worker, audit_checker, retention",
                s
            )),
        }
    }
}

impl fmt::Display for RunMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api => write!(f, "api"),
            Self::Migrate => write!(f, "migrate"),
            Self::Worker => write!(f, "worker"),
            Self::AuditChecker => write!(f, "audit_checker"),
            Self::Retention => write!(f, "retention"),
        }
    }
}

impl RunMode {
    pub fn from_env() -> Self {
        std::env::var("RUN_MODE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default()
    }
}
