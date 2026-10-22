use anyhow::Context as _;

use crate::common::config::LoggingConfigLoader;

use super::{FileSink, Logger, TargetFilter, install, parse_level};

pub fn init(config: impl LoggingConfigLoader, reload: bool) -> anyhow::Result<()> {
    init_with_default_console_targets(config, reload, &[super::LOG_TARGET])
}

pub fn init_with_default_console_targets(
    config: impl LoggingConfigLoader,
    reload: bool,
    default_targets: &[&str],
) -> anyhow::Result<()> {
    init_with_environment(config, reload, default_targets, true)
}

/// Initialize the native core logger using only the selected TOML file.
pub fn init_from_config_file(
    config: impl LoggingConfigLoader,
    reload: bool,
    default_targets: &[&str],
) -> anyhow::Result<()> {
    init_with_environment(config, reload, default_targets, false)
}

fn init_with_environment(
    config: impl LoggingConfigLoader,
    reload: bool,
    default_targets: &[&str],
    use_environment: bool,
) -> anyhow::Result<()> {
    let console_config = config.get_console_logger_config();
    let console_level = console_config
        .level
        .as_deref()
        .map(parse_level)
        .transpose()
        .context("invalid console log level")?;
    let console = if use_environment {
        TargetFilter::console_with_default_targets(console_level, default_targets)?
    } else {
        TargetFilter::console_defaults(console_level, default_targets)
    };
    let file = FileSink::from_config_with_environment(
        config.get_file_logger_config(),
        reload,
        use_environment,
    )?;

    install(Logger::new(console, file))
}
