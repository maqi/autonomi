use std::io;
use tracing_appender::rolling;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub fn setup_logging(log_to_file: bool) {
    // Load log level from RUST_LOG
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(""));

    // Create stdout logging layer
    let stdout_layer = fmt::layer().with_writer(io::stdout);

    let subscriber = tracing_subscriber::registry()
        .with(env_filter)
        .with(stdout_layer);

    if log_to_file {
        // Set up file appender with daily log rotation
        let file_appender = rolling::daily("logs", "app.log");
        let (file_writer, _guard) = tracing_appender::non_blocking(file_appender);
        let file_layer = fmt::layer().with_writer(file_writer);
        subscriber.with(file_layer).init();
        Box::leak(Box::new(_guard));
    } else {
        subscriber.init();
    }
}
