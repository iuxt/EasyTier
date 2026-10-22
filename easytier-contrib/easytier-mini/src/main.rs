use anyhow::Context as _;
use easytier::{
    common::config::{ConfigFileControl, load_toml_config_from_path},
    instance::factory::native_compact_instance_manager_with_runtime,
    rpc_service::ReadOnlyApiRpcServer,
};
use std::{ffi::OsString, path::PathBuf, sync::Arc};

fn config_path(mut args: impl Iterator<Item = OsString>) -> anyhow::Result<Option<PathBuf>> {
    let mut config = None;
    while let Some(arg) = args.next() {
        if arg == "-h" || arg == "--help" {
            println!("easytier-mini [-c FILE.toml]\nDefault: config.toml next to the executable");
            return Ok(None);
        }
        if arg == "-V" || arg == "--version" {
            println!("easytier-mini {}", env!("CARGO_PKG_VERSION"));
            return Ok(None);
        }
        if arg == "-c" || arg == "--config" {
            anyhow::ensure!(config.is_none(), "-c may only be specified once");
            config = Some(PathBuf::from(
                args.next().context("-c requires a file path")?,
            ));
        } else {
            anyhow::bail!("unknown argument {arg:?}; use -c FILE.toml");
        }
    }
    Ok(Some(config.unwrap_or(
        std::env::current_exe()?.with_file_name("config.toml"),
    )))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let Some(path) = config_path(std::env::args_os().skip(1))? else {
        return Ok(());
    };
    let local_config = load_toml_config_from_path(&path)?;
    easytier::common::log::init_console()?;
    let instances = Arc::new(native_compact_instance_manager_with_runtime(
        tokio::runtime::Handle::current(),
    ));
    instances.run_network_instance(local_config, ConfigFileControl::STATIC_CONFIG)?;
    let _rpc_server =
        ReadOnlyApiRpcServer::new(Some("127.0.0.1:15888".to_owned()), None, instances.clone())?
            .serve()
            .await?;
    let stopped_unexpectedly = tokio::select! {
        signal = tokio::signal::ctrl_c() => { signal.context("failed to listen for Ctrl-C")?; false },
        _ = instances.wait() => true,
    };
    for instance in instances.instances() {
        instance.stop().await;
    }
    anyhow::ensure!(
        !stopped_unexpectedly,
        "EasyTier instance stopped unexpectedly"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use easytier::common::config::{ConfigLoader as _, TomlConfigLoader};

    #[test]
    fn only_accepts_config_files() {
        assert_eq!(
            config_path([OsString::from("-c"), OsString::from("mini.toml")].into_iter()).unwrap(),
            Some(PathBuf::from("mini.toml"))
        );
        assert!(
            config_path([OsString::from("--config-server"), OsString::from("url")].into_iter())
                .is_err()
        );
    }

    #[tokio::test]
    async fn compact_factory_accepts_unsupported_config_without_changing_it() {
        let config = TomlConfigLoader::new_from_str(
            r#"
dhcp = true
listeners = ["quic://127.0.0.1:11010"]
proxy_network = [{ cidr = "10.20.0.0/16" }]

[flags]
encryption_algorithm = "chacha20"
data_compress_algo = "Zstd"
"#,
        )
        .unwrap();
        config.get_id();
        let before = config.dump();
        let manager =
            native_compact_instance_manager_with_runtime(tokio::runtime::Handle::current());

        let instance = manager.create(config, ()).unwrap();

        assert_eq!(instance.toml_config().unwrap().dump(), before);
    }
}
