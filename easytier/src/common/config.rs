//! Native Adapters around the core-owned TOML configuration model.

use std::path::PathBuf;

use anyhow::Context as _;
use strum::VariantArray as _;

use easytier_core::config::MappedListenerPolicy;
#[cfg(feature = "management")]
pub use easytier_core::config::api_input::{
    NetworkConfig, NetworkConfigExt, NetworkingMethod, add_proxy_network_to_config,
};
pub use easytier_core::config::toml::*;

use crate::tunnel::IpScheme;

#[cfg(feature = "management")]
pub use easytier_core::management::{config_source_from_rpc, config_source_to_rpc};

pub fn parse_mapped_listener_urls(
    mapped_listeners: &[String],
) -> Result<Vec<url::Url>, anyhow::Error> {
    MappedListenerPolicy::new(IpScheme::VARIANTS.iter().map(ToString::to_string))
        .parse_urls(mapped_listeners)
}

pub fn load_toml_config_from_path(path: &PathBuf) -> Result<TomlConfigLoader, anyhow::Error> {
    let config = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file: {}", path.display()))?;
    load_toml_config_from_str_with_source(&path.display().to_string(), &config)
}

pub fn load_toml_config_from_str_with_source(
    source_name: &str,
    config_str: &str,
) -> Result<TomlConfigLoader, anyhow::Error> {
    TomlConfigLoader::new_from_str_with_source(source_name, config_str)
}

#[cfg(feature = "management-rpc")]
pub use easytier_core::management::{ConfigFileControl, ConfigFilePermission};

#[cfg(feature = "management")]
pub async fn config_file_control_from_path(path: PathBuf) -> ConfigFileControl {
    let read_only = tokio::fs::metadata(&path)
        .await
        .map(|metadata| metadata.permissions().readonly())
        .unwrap_or(true);
    ConfigFileControl::new(
        Some(path),
        if read_only {
            ConfigFilePermission::from(ConfigFilePermission::READ_ONLY)
        } else {
            ConfigFilePermission::default()
        },
    )
}

#[cfg(feature = "management")]
pub async fn load_config_from_file(
    config_file: &PathBuf,
    config_dir: Option<&PathBuf>,
) -> Result<(TomlConfigLoader, ConfigFileControl), anyhow::Error> {
    let config_str = tokio::fs::read_to_string(config_file)
        .await
        .with_context(|| format!("failed to read config file: {}", config_file.display()))?;
    let config =
        load_toml_config_from_str_with_source(&config_file.display().to_string(), &config_str)?;
    let mut control = config_file_control_from_path(config_file.clone()).await;
    if control.is_read_only() {
        control.set_no_delete(true);
    } else if let Some(config_dir) = config_dir {
        let is_managed_file = config_file.parent() == Some(config_dir.as_path())
            && config_file.file_stem() == Some(config.get_id().to_string().as_ref())
            && config_file.extension() == Some(std::ffi::OsStr::new("toml"));
        control.set_no_delete(!is_managed_file);
    } else {
        control.set_no_delete(true);
    }
    Ok((config, control))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_adapter_preserves_file_name_in_parse_error() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "dhcp = \"yes\"").unwrap();
        let error = load_toml_config_from_path(&file.path().to_path_buf())
            .unwrap_err()
            .to_string();
        assert!(error.contains(file.path().to_string_lossy().as_ref()));
    }

    #[tokio::test]
    async fn file_values_are_literal_and_keep_file_permissions() {
        let file = tempfile::NamedTempFile::new().unwrap();
        // HOME is normally defined; it must still remain literal.
        std::fs::write(file.path(), "hostname = '$HOME/${HOME}'").unwrap();
        let (config, control) = load_config_from_file(&file.path().to_path_buf(), None)
            .await
            .unwrap();
        assert_eq!(config.get_hostname(), "$HOME/${HOME}");
        assert_eq!(control.path.as_deref(), Some(file.path()));
        assert!(!control.is_read_only());
        assert!(control.is_no_delete());
    }
}
