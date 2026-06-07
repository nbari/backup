use anyhow::Result;
use tracing_subscriber::{EnvFilter, Registry, fmt, layer::SubscriberExt};

/// Start the telemetry layer
/// # Errors
/// Will return an error if the telemetry layer fails to start
pub fn init() -> Result<()> {
    let fmt_layer = fmt::layer()
        .without_time()
        .with_level(false)
        .with_file(false)
        .with_line_number(false)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_target(false)
        .compact();

    let filter = match EnvFilter::try_from_default_env() {
        Ok(filter) => filter.add_directive("tokio=error".parse()?),
        Err(_) => EnvFilter::try_new("off")?,
    };

    let subscriber = Registry::default().with(fmt_layer).with(filter);

    Ok(tracing::subscriber::set_global_default(subscriber)?)
}
