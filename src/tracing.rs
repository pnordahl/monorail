use crate::common::error::MonorailError;

use tracing_subscriber::filter::{EnvFilter, LevelFilter};
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Registry;

pub fn setup(format: &str, level: u8) -> Result<(), MonorailError> {
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
            let json_fmt_layer = fmt::layer()
                .json()
                .with_current_span(false)
                .with_span_list(false)
                .with_target(false);
            tracing::subscriber::set_global_default(registry.with(json_fmt_layer))
                .map_err(|e| MonorailError::from(e.to_string()))?;
        }
        _ => {
            let plain_fmt_layer = fmt::layer().with_target(true).with_thread_names(true);
            tracing::subscriber::set_global_default(registry.with(plain_fmt_layer))
                .map_err(|e| MonorailError::from(e.to_string()))?;
        }
    }
    Ok(())
}
