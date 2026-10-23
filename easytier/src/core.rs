use crate::{
    ShellType,
    common::{constants::EASYTIER_VERSION, log},
    utils::panic::setup_panic_handler,
};
use clap::{CommandFactory, FromArgMatches, Parser};
#[cfg(feature = "jemalloc-prof")]
use jemalloc_ctl::{Access as _, AsName as _, epoch, stats};
use std::{
    path::PathBuf,
    process::ExitCode,
    sync::{Arc, atomic::AtomicBool},
};

mod config;
mod managed;
use config::StartupConfig;

#[cfg(target_os = "windows")]
windows_service::define_windows_service!(ffi_service_main, win_service_main);

fn set_prof_active(_active: bool) {
    #[cfg(feature = "jemalloc-prof")]
    {
        const PROF_ACTIVE: &[u8] = b"prof.active\0";
        let name = PROF_ACTIVE.name();
        name.write(_active).expect("Should succeed to set prof");
    }
}

#[cfg(feature = "jemalloc-prof")]
fn get_dump_profile_path(cur_allocated: usize, suffix: &str) -> String {
    format!(
        "profile-{}-{}.{}",
        cur_allocated,
        chrono::Local::now().format("%Y-%m-%d-%H-%M-%S"),
        suffix
    )
}

#[cfg(feature = "jemalloc-prof")]
fn dump_profile(_cur_allocated: usize) {
    #[cfg(feature = "jemalloc-prof")]
    {
        const PROF_DUMP: &[u8] = b"prof.dump\0";
        static mut PROF_DUMP_FILE_NAME: [u8; 128] = [0; 128];
        let file_name_str = get_dump_profile_path(_cur_allocated, "out");
        // copy file name to PROF_DUMP
        let file_name = file_name_str.as_bytes();
        let len = file_name.len();
        if len > 127 {
            panic!("file name too long");
        }
        unsafe {
            PROF_DUMP_FILE_NAME[..len].copy_from_slice(file_name);
            // set the last byte to 0
            PROF_DUMP_FILE_NAME[len] = 0;

            let name = PROF_DUMP.name();
            name.write(&PROF_DUMP_FILE_NAME[..len + 1])
                .expect("Should succeed to dump profile");
            println!("dump profile to: {}", file_name_str);
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "easytier-core", author, version = EASYTIER_VERSION, disable_help_flag = true,
    about = "EasyTier：注册到统一管理服务，审批后自动获取网络配置")]
struct Cli {
    #[arg(long, action = clap::ArgAction::Help, help = "显示帮助")]
    help: Option<bool>,

    #[arg(short = 'h', long = "server", requires = "token", conflicts_with_all = ["config_file", "check_config", "gen_autocomplete"], help = "统一管理服务地址（HTTPS）")]
    server: Option<String>,

    #[arg(short = 'k', long, requires = "server", help = "设备注册 token")]
    token: Option<String>,

    #[arg(
        long,
        requires = "server",
        help = "设备身份存储目录，默认 ~/.easytier/managed"
    )]
    state_dir: Option<PathBuf>,
    #[arg(
        short = 'c',
        long = "config-file",
        value_name = "FILE",
        requires = "check_config",
        help = "离线校验的配置文件路径（须配合 --check-config）"
    )]
    config_file: Option<PathBuf>,

    #[arg(long, help = "校验配置文件并退出，不启动网络")]
    check_config: bool,

    #[arg(long, help = "生成命令行补全脚本")]
    gen_autocomplete: Option<ShellType>,

    #[cfg(feature = "cli")]
    #[command(flatten)]
    management_options: crate::cli::ManagementOptions,

    #[cfg(feature = "cli")]
    #[command(subcommand)]
    command: Option<crate::cli::ManagementCommand>,
}

impl Cli {
    fn try_parse_from<I, T>(args: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let mut command = cli_command();
        let matches = command.try_get_matches_from_mut(args)?;
        #[cfg(feature = "cli")]
        {
            use clap::{error::ErrorKind, parser::ValueSource};
            let explicit = |id| matches.value_source(id) == Some(ValueSource::CommandLine);
            if matches.subcommand_name().is_some() {
                if [
                    "config_file",
                    "check_config",
                    "gen_autocomplete",
                    "server",
                    "token",
                    "state_dir",
                ]
                .into_iter()
                .any(explicit)
                {
                    return Err(command.error(
                        ErrorKind::ArgumentConflict,
                        "startup options cannot be used with management subcommands",
                    ));
                }
            } else if ["verbose", "output_format", "no_trunc", "id"]
                .into_iter()
                .any(explicit)
            {
                return Err(command.error(
                    ErrorKind::MissingSubcommand,
                    "management options require a subcommand (for example, peer or node)",
                ));
            }
        }
        Self::from_arg_matches(&matches)
    }

    fn config_path(&self) -> anyhow::Result<PathBuf> {
        match &self.config_file {
            Some(path) if path.as_os_str() == "-" => {
                anyhow::bail!("stdin configuration is not supported; use -c FILE.toml")
            }
            Some(path) => Ok(path.clone()),
            None => Ok(std::env::current_exe()?.with_file_name("config.toml")),
        }
    }
}

fn cli_command() -> clap::Command {
    Cli::command()
}

fn parse_cli() -> Cli {
    Cli::try_parse_from(std::env::args_os()).unwrap_or_else(|error| error.exit())
}

#[cfg(all(test, feature = "cli"))]
pub(crate) fn parse_management_command(
    args: &[&str],
) -> (crate::cli::ManagementOptions, crate::cli::ManagementCommand) {
    let cli = Cli::try_parse_from(args).expect("failed to parse core management command");
    (
        cli.management_options,
        cli.command.expect("missing subcommand"),
    )
}

#[cfg(target_os = "windows")]
fn win_service_set_work_dir(service_name: &std::ffi::OsString) -> anyhow::Result<()> {
    use crate::common::constants::WIN_SERVICE_WORK_DIR_REG_KEY;
    use winreg::{RegKey, enums::*};

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = hklm.open_subkey_with_flags(WIN_SERVICE_WORK_DIR_REG_KEY, KEY_READ)?;
    let dir_pat_str = key.get_value::<std::ffi::OsString, _>(service_name)?;
    let dir_path = std::fs::canonicalize(dir_pat_str)?;

    std::env::set_current_dir(dir_path)?;

    Ok(())
}

#[cfg(target_os = "windows")]
fn win_service_event_loop(
    stop_notify: std::sync::Arc<tokio::sync::Notify>,
    cli: Cli,
    status_handle: windows_service::service_control_handler::ServiceStatusHandle,
) {
    use std::time::Duration;
    use tokio::runtime::Runtime;
    use windows_service::service::*;

    let normal_status = ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    };
    let error_status = ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::ServiceSpecific(1u32),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    };

    std::thread::spawn(move || {
        let rt = Runtime::new().unwrap();
        rt.block_on(async move {
            tokio::select! {
                res = run_main(cli) => {
                    match res {
                        Ok(_) => {
                            status_handle.set_service_status(normal_status).unwrap();
                            std::process::exit(0);
                        }
                        Err(error) => {
                            status_handle.set_service_status(error_status).unwrap();
                            log::error!(?error);
                            // exit with non-zero code so the SCM treats this as
                            // a non-crash failure and applies the configured
                            // failure actions; staying alive would leave a
                            // zombie process until the SCM force-kills it.
                            std::process::exit(1);
                        }
                    }
                },
                _ = stop_notify.notified() => {
                    _ = status_handle.set_service_status(normal_status);
                    std::process::exit(0);
                }
            }
        });
    });
}

#[cfg(target_os = "windows")]
fn win_service_main(arg: Vec<std::ffi::OsString>) {
    use std::{sync::Arc, time::Duration};
    use tokio::sync::Notify;
    use windows_service::{service::*, service_control_handler::*};

    _ = win_service_set_work_dir(&arg[0]);

    let cli = parse_cli();

    let stop_notify_send = Arc::new(Notify::new());
    let stop_notify_recv = Arc::clone(&stop_notify_send);
    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            ServiceControl::Stop => {
                stop_notify_send.notify_one();
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };
    let status_handle = register(String::new(), event_handler).expect("register service fail");
    let next_status = ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    };
    status_handle
        .set_service_status(next_status)
        .expect("set service status fail");

    win_service_event_loop(stop_notify_recv, cli, status_handle);
}

async fn run_main(cli: Cli) -> anyhow::Result<()> {
    let server = cli.server.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "请使用 easytier-core -h https://服务端地址 -k 注册token 启动；使用 --help 查看帮助"
        )
    })?;
    managed::run(
        server,
        cli.token.as_deref().unwrap(),
        cli.state_dir.as_deref(),
    )
    .await
}

fn memory_monitor(_force_dump: Arc<AtomicBool>) {
    #[cfg(feature = "jemalloc-prof")]
    {
        let mut last_peak_size = 0;
        let e = epoch::mib().unwrap();
        let allocated_stats = stats::allocated::mib().unwrap();

        loop {
            e.advance().unwrap();
            let new_heap_size = allocated_stats.read().unwrap();

            log::debug!("heap size: {} bytes", new_heap_size);

            // dump every 75MB
            if (last_peak_size > 0
                && new_heap_size > last_peak_size
                && new_heap_size - last_peak_size > 10 * 1024 * 1024)
                || _force_dump.load(std::sync::atomic::Ordering::Relaxed)
            {
                log::debug!(
                    "heap size increased: {} bytes",
                    new_heap_size - last_peak_size,
                );
                dump_profile(new_heap_size);
                last_peak_size = new_heap_size;
                if _force_dump.load(std::sync::atomic::Ordering::Relaxed) {
                    // also dump whole jemalloc stats
                    use jemalloc_ctl::stats_print::stats_print;
                    let tmp_file = get_dump_profile_path(new_heap_size, "stats");
                    let mut file = std::fs::File::create(tmp_file).unwrap();
                    let _ = stats_print(&mut file, Default::default());
                    _force_dump.store(false, std::sync::atomic::Ordering::Relaxed);
                }
            }

            if last_peak_size == 0 {
                last_peak_size = new_heap_size;
            }

            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    }
}

pub async fn main() -> ExitCode {
    let locale = sys_locale::get_locale().unwrap_or_else(|| String::from("en-US"));
    rust_i18n::set_locale(&locale);
    setup_panic_handler();
    crate::utils::init_crypto_provider();

    let cli = parse_cli();
    #[cfg(feature = "cli")]
    if let Some(command) = cli.command {
        return match crate::cli::run(cli.management_options, command, cli_command()).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error:#}");
                ExitCode::FAILURE
            }
        };
    }

    #[cfg(target_os = "windows")]
    match windows_service::service_dispatcher::start(String::new(), ffi_service_main) {
        Ok(_) => std::thread::park(),
        Err(e) => {
            let should_panic = if let windows_service::Error::Winapi(ref io_error) = e {
                io_error.raw_os_error() != Some(0x427) // ERROR_FAILED_SERVICE_CONTROLLER_CONNECT
            } else {
                true
            };

            if should_panic {
                panic!("SCM start an error: {}", e);
            }
        }
    };

    set_prof_active(true);
    // register a signal handler to set force dump when signal usr1 is received
    let force_dump = Arc::new(AtomicBool::new(false));
    #[cfg(all(feature = "jemalloc-prof", not(target_os = "windows")))]
    {
        let force_dump_clone = force_dump.clone();
        let mut sigusr1 =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1()).unwrap();
        tokio::task::spawn(async move {
            while sigusr1.recv().await.is_some() {
                force_dump_clone.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        });
    }
    let _monitor = std::thread::spawn(move || memory_monitor(force_dump));

    if let Some(shell) = cli.gen_autocomplete {
        let mut cmd = cli_command();
        if let Some(shell) = shell.to_shell() {
            crate::print_completions(shell, &mut cmd, "easytier-core");
        } else {
            // Handle Nushell
            crate::print_nushell_completions(&mut cmd, "easytier-core");
        }
        return ExitCode::SUCCESS;
    }

    if cli.check_config {
        match cli.config_path() {
            Ok(path) => match StartupConfig::load(&path).await {
                Ok(_) => {
                    println!("Configuration is valid: {}", path.display());
                    return ExitCode::SUCCESS;
                }
                Err(error) => eprintln!("Config validation failed: {error:#}"),
            },
            Err(error) => eprintln!("Config validation failed: {error:#}"),
        }
        return ExitCode::FAILURE;
    }

    let mut ret_code = 0;

    if let Err(error) = run_main(cli).await {
        eprintln!("Failed to start easytier-core: {error:#}");
        ret_code = 1;
    }

    #[cfg(feature = "jemalloc-prof")]
    dump_profile(0);
    log::info!("Stopping easytier...");
    set_prof_active(false);

    ExitCode::from(ret_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_startup_arguments_are_paired_and_exclusive() {
        let cli =
            Cli::try_parse_from(["core", "-h", "https://example.com", "-k", "token"]).unwrap();
        assert_eq!(cli.server.as_deref(), Some("https://example.com"));
        assert!(
            !Cli::try_parse_from(["core", "--help"])
                .unwrap_err()
                .use_stderr()
        );
        for args in [
            vec!["core", "-h", "https://example.com"],
            vec!["core", "-k", "token"],
            vec!["core", "-c", "config.toml"],
            vec!["core", "-h", "https://example.com", "-k", "token", "peer"],
            vec![
                "core",
                "-h",
                "https://example.com",
                "-k",
                "token",
                "--check-config",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn only_file_selection_and_utility_arguments_are_accepted() {
        assert_eq!(
            Cli::try_parse_from(["core", "-c", "custom.toml", "--check-config"])
                .unwrap()
                .config_path()
                .unwrap(),
            PathBuf::from("custom.toml")
        );
        for args in [
            vec!["core", "--network-name", "test"],
            vec!["core", "--console-log-level", "debug"],
            vec!["core", "--config-dir", "."],
            vec!["core", "--daemon"],
            vec!["core", "-c", "a.toml", "b.toml"],
            vec!["core", "-c", "a.toml", "-c", "b.toml"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
        assert!(
            Cli::try_parse_from(["core", "-c", "-", "--check-config"])
                .unwrap()
                .config_path()
                .is_err()
        );
    }

    #[cfg(feature = "cli")]
    #[test]
    fn management_commands_share_the_core_entrypoint() {
        cli_command().debug_assert();
        for args in [
            vec!["core", "peer"],
            vec!["core", "route", "dump"],
            vec![
                "core",
                "-o",
                "json",
                "--instance-id",
                "018f4fb1-7a2c-7d1f-9d89-935b0ad7e135",
                "node",
            ],
            vec!["core", "connector", "-p", "tcp://127.0.0.1:11010", "list"],
            vec![
                "core",
                "service",
                "install",
                "--core-args",
                "-c",
                "custom.toml",
            ],
            vec!["core", "gen-autocomplete", "bash"],
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert!(cli.command.is_some());
        }
        for args in [
            vec!["core", "-c", "custom.toml", "peer"],
            vec!["core", "--check-config", "node"],
            vec!["core", "--gen-autocomplete", "bash", "peer"],
            vec!["core", "-p", "127.0.0.1:15888", "peer"],
            vec!["core", "--rpc-portal", "192.0.2.1:15888", "peer"],
            vec!["core", "-o", "json"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn default_config_is_next_to_the_executable() {
        let cli = Cli::try_parse_from(["core"]).unwrap();
        assert_eq!(
            cli.config_path().unwrap(),
            std::env::current_exe()
                .unwrap()
                .with_file_name("config.toml")
        );
    }
}
