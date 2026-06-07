use anyhow::Result;
use tracing::Level;
use tracing_subscriber::{EnvFilter, Registry, fmt, layer::SubscriberExt};

/// Start the telemetry layer
/// # Errors
/// Will return an error if the telemetry layer fails to start
pub fn init(verbosity_level: Option<Level>) -> Result<()> {
    let fmt_layer = fmt::layer()
        .without_time()
        .with_level(false)
        .with_file(false)
        .with_line_number(false)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_target(false)
        .compact();

    let filter = if let Some(verbosity_level) = verbosity_level {
        EnvFilter::builder()
            .with_default_directive(verbosity_level.into())
            .from_env_lossy()
            .add_directive("tokio=error".parse()?)
    } else {
        EnvFilter::try_new("off")?
    };

    let subscriber = Registry::default().with(fmt_layer).with(filter);

    Ok(tracing::subscriber::set_global_default(subscriber)?)
}
