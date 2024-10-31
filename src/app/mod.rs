pub(crate) mod analyze;
pub(crate) mod checkpoint;
pub(crate) mod log;
pub(crate) mod out;
pub(crate) mod result;
pub(crate) mod run;
pub(crate) mod target;

use std::{path, sync, time};

use std::collections::HashMap;
use std::collections::HashSet;

use std::fs::OpenOptions;
use std::io::{BufReader, BufWriter};
use std::os::unix::fs::PermissionsExt;

use std::result::Result;

use serde::{Deserialize, Serialize};
use sha2::Digest;

use tracing::{debug, error, info, instrument};
use tracing_subscriber::filter::{EnvFilter, LevelFilter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{fmt, Registry};

use crate::core::error::{GraphError, MonorailError};
use crate::core::{self, file, git, tracking, ChangeProviderKind, Target};

// Custom formatter to match chrono's strict RFC3339 compliance.
struct UtcTimestampWithOffset;

impl fmt::time::FormatTime for UtcTimestampWithOffset {
    fn format_time(&self, w: &mut fmt::format::Writer<'_>) -> std::fmt::Result {
        let now = chrono::Utc::now();
        // Format the timestamp as "YYYY-MM-DDTHH:MM:SS.f+00:00"
        write!(w, "{}", now.format("%Y-%m-%dT%H:%M:%S%.f+00:00"))
    }
}

pub(crate) fn setup_tracing(format: &str, level: u8) -> Result<(), MonorailError> {
    let env_filter = EnvFilter::default();
    let level_filter = match level {
        0 => LevelFilter::OFF,
        1 => LevelFilter::INFO,
        2 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };

    let registry = Registry::default().with(env_filter.add_directive(level_filter.into()));
    match format {
        "json" => {
            let json_fmt_layer = tracing_subscriber::fmt::layer()
                .json()
                .with_current_span(false)
                .with_span_list(false)
                .with_target(false)
                .flatten_event(true)
                .with_timer(UtcTimestampWithOffset);
            tracing::subscriber::set_global_default(registry.with(json_fmt_layer))
                .map_err(|e| MonorailError::from(e.to_string()))?;
        }
        _ => {
            let plain_fmt_layer = tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_thread_names(true)
                .with_timer(UtcTimestampWithOffset);
            tracing::subscriber::set_global_default(registry.with(plain_fmt_layer))
                .map_err(|e| MonorailError::from(e.to_string()))?;
        }
    }
    Ok(())
}
