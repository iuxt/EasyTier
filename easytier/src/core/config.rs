use std::path::{Path, PathBuf};

use anyhow::Context as _;
use cidr::IpCidr;
use serde::Deserialize;

use crate::common::config::{
    ConfigFileControl, ConfigLoader, ConsoleLoggerConfig, FileLoggerConfig, LoggingConfigLoader,
    TomlConfigLoader, config_file_control_from_path, load_config_from_file,
};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct CoreOptions {
    pub config_dir: Option<PathBuf>,
    pub rpc_portal: Option<String>,
    pub rpc_portal_whitelist: Option<Vec<IpCidr>>,
    pub daemon: bool,
}

// Process settings belong to the native executable, not the portable network model.
#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct StartupConfig {
    pub core: CoreOptions,
    console_logger: Option<ConsoleLoggerConfig>,
    file_logger: Option<FileLoggerConfig>,
}

type LoadedNetworks = Vec<(TomlConfigLoader, ConfigFileControl)>;

impl StartupConfig {
    pub async fn load(path: &Path) -> anyhow::Result<(Self, LoadedNetworks)> {
        let path = tokio::fs::canonicalize(path)
            .await
            .with_context(|| format!("failed to open config file: {}", path.display()))?;
        let contents = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("failed to read config file: {}", path.display()))?;
        let network =
            TomlConfigLoader::new_from_str_with_source(&path.display().to_string(), &contents)?;
        let mut startup: Self = toml::from_str(&contents)
            .with_context(|| format!("invalid startup configuration in {}", path.display()))?;
        let base = path
            .parent()
            .context("config file has no parent directory")?;
        if let Some(dir) = &mut startup.core.config_dir {
            *dir = resolve_path(base, dir);
        }
        let logger = startup
            .file_logger
            .get_or_insert_with(FileLoggerConfig::default);
        logger.dir = Some(
            resolve_path(base, Path::new(logger.dir.as_deref().unwrap_or(".")))
                .to_string_lossy()
                .into_owned(),
        );
        startup.validate_logging()?;
        // A process-only file can host directory-managed instances without
        // creating an extra default network. Explicit network fields always run.
        let document: toml::Table = toml::from_str(&contents)?;
        let has_network = document
            .keys()
            .any(|key| !matches!(key.as_str(), "core" | "console_logger" | "file_logger"));
        let mut configs = Vec::new();
        if has_network || (!startup.core.daemon && startup.core.config_dir.is_none()) {
            resolve_network_paths(&network, base);
            let mut control = config_file_control_from_path(path.clone()).await;
            control.set_no_delete(true);
            configs.push((network, control));
        }
        if let Some(dir) = &startup.core.config_dir {
            let mut entries = tokio::fs::read_dir(dir)
                .await
                .with_context(|| format!("failed to read config directory: {}", dir.display()))?;
            let mut paths = Vec::new();
            while let Some(entry) = entries.next_entry().await? {
                let entry_path = entry.path();
                if entry.file_type().await?.is_file()
                    && entry_path.extension().is_some_and(|ext| ext == "toml")
                {
                    if tokio::fs::canonicalize(&entry_path).await? != path {
                        paths.push(entry_path);
                    }
                }
            }
            paths.sort();
            for path in paths {
                let (network, control) = load_config_from_file(&path, Some(dir)).await?;
                resolve_network_paths(&network, path.parent().unwrap());
                configs.push((network, control));
            }
        }
        Ok((startup, configs))
    }

    fn validate_logging(&self) -> anyhow::Result<()> {
        for (section, level) in [
            (
                "console_logger",
                self.console_logger
                    .as_ref()
                    .and_then(|logger| logger.level.as_deref()),
            ),
            (
                "file_logger",
                self.file_logger
                    .as_ref()
                    .and_then(|logger| logger.level.as_deref()),
            ),
        ] {
            if let Some(level) = level {
                level
                    .parse::<::log::LevelFilter>()
                    .with_context(|| format!("invalid {section}.level: {level:?}"))?;
            }
        }
        if let Some(logger) = &self.file_logger {
            anyhow::ensure!(
                logger.size_mb != Some(0),
                "file_logger.size_mb must be greater than zero"
            );
            anyhow::ensure!(
                logger.count != Some(0),
                "file_logger.count must be greater than zero"
            );
        }
        Ok(())
    }
}

fn resolve_path(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    }
}

fn resolve_network_paths(network: &TomlConfigLoader, base: &Path) {
    if let Some(path) = network.get_credential_file() {
        network.set_credential_file(Some(resolve_path(base, &path)));
    }
}

impl LoggingConfigLoader for &StartupConfig {
    fn get_console_logger_config(&self) -> ConsoleLoggerConfig {
        self.console_logger.clone().unwrap_or_default()
    }

    fn get_file_logger_config(&self) -> FileLoggerConfig {
        self.file_logger.clone().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_template_covers_every_current_flag() {
        let template = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../config.toml"));
        for field in crate::proto::common::Flags::flags() {
            assert!(
                template.contains(&format!("{} =", field.name())),
                "missing flag {}",
                field.name()
            );
        }
    }

    #[tokio::test]
    async fn process_options_and_paths_come_from_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "hostname = '$ET_HOSTNAME'\ncredential_file = 'credential.json'\n[core]\nrpc_portal = '127.0.0.1:0'\n[console_logger]\nlevel = 'warn'\n[file_logger]\nlevel = 'info'\ndir = 'logs'\n").unwrap();
        let (startup, configs) = StartupConfig::load(&path).await.unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(
            (&startup).get_file_logger_config().dir,
            Some(base.join("logs").to_string_lossy().into_owned())
        );
        assert_eq!(configs[0].0.get_hostname(), "$ET_HOSTNAME");
        assert_eq!(
            configs[0].0.get_credential_file(),
            Some(base.join("credential.json"))
        );
        assert!(configs[0].1.is_no_delete());
    }

    #[tokio::test]
    async fn directory_mode_skips_primary_file_and_validates_all_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[core]\nconfig_dir = '.'\ndaemon = true\n").unwrap();
        std::fs::write(dir.path().join("network.toml"), "hostname = 'network'").unwrap();
        let (_, configs) = StartupConfig::load(&path).await.unwrap();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].0.get_hostname(), "network");
        std::fs::write(dir.path().join("invalid.toml"), "dhcp = 'invalid'").unwrap();
        assert!(StartupConfig::load(&path).await.is_err());
    }

    #[tokio::test]
    async fn invalid_startup_settings_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for contents in [
            "[core]\nunknown = true",
            "[console_logger]\nlevel = 'verbose'",
            "[file_logger]\nsize_mb = 0",
        ] {
            std::fs::write(&path, contents).unwrap();
            assert!(StartupConfig::load(&path).await.is_err());
        }
        assert!(
            StartupConfig::load(&dir.path().join("missing.toml"))
                .await
                .is_err()
        );
    }
}
