//! Enrollment, short authorization leases, configuration reconciliation and central logging.
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, bail, ensure};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{RwLock, mpsc},
    time::Instant,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    common::{
        config::{ConfigLoader, TomlConfigLoader},
        constants::EASYTIER_VERSION,
        log::{self, RemoteLog},
    },
    instance::factory::{NativeCoreInstance, NativeInstanceManager, native_cli_instance_manager},
};
use easytier_core::instance::CoreInstanceState;

const POLL: Duration = Duration::from_secs(5);
const LEASE: Duration = Duration::from_secs(45);

#[derive(Serialize, Deserialize)]
struct Identity {
    server: String,
    id: String,
    secret: String,
}

fn server_url(value: &str) -> anyhow::Result<String> {
    let u = url::Url::parse(value).context("服务端地址无效")?;
    let local = matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    ensure!(
        u.scheme() == "https" || (u.scheme() == "http" && local),
        "服务端地址必须使用 HTTPS；本机调试可使用 HTTP"
    );
    ensure!(
        u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none()
            && u.path() == "/",
        "服务端地址只填写协议、主机和端口"
    );
    Ok(u.as_str().trim_end_matches('/').to_owned())
}

fn load_identity(server: &str, directory: Option<&Path>) -> anyhow::Result<(Identity, File)> {
    let directory = match directory {
        Some(dir) => dir.to_owned(),
        None => PathBuf::from(
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .context("无法确定用户目录，请使用 --state-dir")?,
        )
        .join(".easytier/managed"),
    };
    std::fs::create_dir_all(&directory).context("无法创建设备身份目录")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let name = Uuid::new_v5(&Uuid::NAMESPACE_URL, server.as_bytes()).to_string();
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(directory.join(format!("{name}.lock")))?;
    lock.try_lock().context("同一设备身份已有客户端运行")?;
    let path = directory.join(format!("{name}.json"));
    let identity = match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice::<Identity>(&bytes).context("设备身份文件损坏，请恢复备份")?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let identity = Identity {
                server: server.to_owned(),
                id: Uuid::new_v4().to_string(),
                secret: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
            };
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            use std::io::Write;
            let mut file = options.open(&path)?;
            file.write_all(&serde_json::to_vec(&identity)?)?;
            file.sync_all()?;
            identity
        }
        Err(e) => return Err(e.into()),
    };
    ensure!(
        identity.server == server
            && Uuid::parse_str(&identity.id).is_ok()
            && identity.secret.len() >= 32,
        "设备身份文件无效"
    );
    Ok((identity, lock))
}

#[derive(Deserialize)]
struct SyncResponse {
    status: String,
    config: String,
    revision: i64,
    lease_seconds: u64,
}

struct Runtime {
    manager: Arc<NativeInstanceManager>,
    instance: Option<Arc<NativeCoreInstance>>,
    applied: i64,
    state: &'static str,
    error: String,
}
impl Runtime {
    async fn stop(&mut self, state: &'static str) -> anyhow::Result<()> {
        self.manager
            .delete_network_instances(self.manager.instance_ids())
            .await?;
        self.instance = None;
        self.applied = 0;
        self.state = state;
        Ok(())
    }
    async fn apply(&mut self, config: &str, revision: i64, id: &str) -> anyhow::Result<()> {
        if self.applied == revision
            && self
                .instance
                .as_ref()
                .is_some_and(|i| i.state() == CoreInstanceState::Running)
        {
            return Ok(());
        }
        // Validate before stopping the previous revision, but never extend its authorization on error.
        let cfg = managed_config(config, id)?;
        self.stop("stopped").await?;
        let instance = self.manager.create(cfg, ())?;
        self.instance = Some(instance.clone());
        instance.start().await?;
        ensure!(
            instance.state() == CoreInstanceState::Running,
            "网络实例未正常运行"
        );
        self.applied = revision;
        self.state = "running";
        self.error.clear();
        log::info!(revision, "成功应用服务端配置");
        println!("成功拉取配置，网络启动成功");
        Ok(())
    }
}
fn managed_config(text: &str, id: &str) -> anyhow::Result<TomlConfigLoader> {
    let doc: toml::Table =
        toml::from_str(text).map_err(|_| anyhow::anyhow!("服务端 TOML 配置格式错误"))?;
    for key in ["core", "console_logger", "file_logger", "credential_file"] {
        ensure!(!doc.contains_key(key), "托管配置不允许设置 {key}");
    }
    let cfg = TomlConfigLoader::new_from_str(text)
        .map_err(|_| anyhow::anyhow!("网络配置校验失败，请检查字段类型及地址格式"))?;
    let identity = cfg.get_network_identity();
    ensure!(
        !identity.network_name.is_empty()
            && identity
                .network_secret
                .as_deref()
                .is_some_and(|s| s.len() >= 16),
        "网络名称不能为空，网络密钥至少 16 字符"
    );
    cfg.set_id(Uuid::parse_str(id)?);
    Ok(cfg)
}

async fn upload_logs(
    client: Client,
    identity: Arc<Identity>,
    mut receiver: mpsc::Receiver<RemoteLog>,
    secrets: Arc<RwLock<Vec<String>>>,
    stop: CancellationToken,
) {
    let mut batch = Vec::new();
    loop {
        tokio::select! { _ = tokio::time::sleep(Duration::from_secs(2)) => {}, _ = stop.cancelled() => {} }
        while batch.len() < 100 {
            match receiver.try_recv() {
                Ok(e) => batch.push(e),
                Err(_) => break,
            }
        }
        let dropped = log::take_dropped_remote_logs();
        if dropped > 0 {
            log::warn!(dropped, "集中日志队列已满，部分日志被丢弃");
        }
        if !batch.is_empty() {
            let mut safe = batch.clone();
            let redactions = secrets.read().await;
            for entry in &mut safe {
                for secret in redactions.iter().filter(|s| !s.is_empty()) {
                    entry.message = entry.message.replace(secret, "[REDACTED]");
                }
                entry.message = log_message(&entry.message, 8192);
            }
            drop(redactions);
            let result = client
                .post(format!(
                    "{}/api/client/{}/logs",
                    identity.server, identity.id
                ))
                .bearer_auth(&identity.secret)
                .json(&serde_json::json!({"logs":safe}))
                .send()
                .await;
            if result.is_ok_and(|r| r.status().is_success()) {
                batch.clear();
            }
        }
        if stop.is_cancelled() {
            return;
        }
    }
}
fn remember_secrets(text: &str, secrets: &mut Vec<String>) {
    fn visit(value: &toml::Value, secrets: &mut Vec<String>) {
        match value {
            toml::Value::Table(table) => {
                for (key, value) in table {
                    if (key.contains("secret") || key.contains("token") || key.contains("password"))
                        && value.as_str().is_some_and(|s| !s.is_empty())
                    {
                        let s = value.as_str().unwrap().to_owned();
                        if !secrets.contains(&s) {
                            secrets.push(s);
                        }
                    } else {
                        visit(value, secrets);
                    }
                }
            }
            toml::Value::Array(array) => {
                for v in array {
                    visit(v, secrets);
                }
            }
            _ => {}
        }
    }
    if let Ok(value) = toml::from_str::<toml::Value>(text) {
        visit(&value, secrets);
    }
}
async fn reconcile(
    client: &Client,
    identity: &Identity,
    token: &str,
    runtime: &mut Runtime,
    secrets: Arc<RwLock<Vec<String>>>,
) -> anyhow::Result<()> {
    let mut registered = false;
    let mut lease_deadline = Instant::now();
    let mut shown = String::new();
    let mut interval = tokio::time::interval(POLL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = interval.tick() => {},
            _ = tokio::time::sleep_until(lease_deadline), if runtime.instance.is_some() => {
                runtime.stop("offline").await?;
                println!("服务端失联，已停止网络，等待重新授权");
                log::warn!("授权租约过期，已停止网络");
                continue;
            }
        }
        if !registered {
            if shown != "registering" && shown != "retry" {
                println!("正在注册");
                shown = "registering".into();
            }
            let result = client.post(format!("{}/api/client/register",identity.server)).bearer_auth(token)
                .json(&serde_json::json!({"id":identity.id,"secret":identity.secret,"hostname":gethostname::gethostname().to_string_lossy(),"platform":std::env::consts::OS,"version":EASYTIER_VERSION})).send().await;
            match result {
                Ok(response) if response.status().is_success() => {
                    registered = true;
                    println!("注册成功，等待服务端审批");
                    log::info!("设备注册成功");
                }
                Ok(response)
                    if matches!(
                        response.status(),
                        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
                    ) =>
                {
                    bail!("注册被拒绝，请检查 token 或设备身份")
                }
                _ => {
                    if shown != "retry" {
                        println!("服务端暂不可用，正在重试注册");
                        shown = "retry".into();
                    }
                    continue;
                }
            }
        }
        let request = async {
            let response = client.post(format!("{}/api/client/{}/sync",identity.server,identity.id)).bearer_auth(&identity.secret)
                .json(&serde_json::json!({"applied_revision":runtime.applied,"runtime_status":runtime.state,"runtime_error":runtime.error})).send().await?;
            if response.status() == StatusCode::NOT_FOUND {
                registered = false;
                bail!("设备需要重新注册");
            }
            let response = response.error_for_status()?;
            ensure!(
                response.content_length().unwrap_or(0) <= 256 * 1024,
                "响应过大"
            );
            let mut bytes = Vec::new();
            let mut response = response;
            while let Some(chunk) = response.chunk().await? {
                ensure!(bytes.len() + chunk.len() <= 256 * 1024, "响应过大");
                bytes.extend_from_slice(&chunk);
            }
            Ok::<SyncResponse, anyhow::Error>(serde_json::from_slice(&bytes)?)
        };
        // The lease also bounds response-body reads, including stalled servers.
        let result = tokio::select! {
            response = request => response,
            _ = tokio::time::sleep_until(lease_deadline), if runtime.instance.is_some() => {
                runtime.stop("offline").await?;
                println!("服务端失联，已停止网络，等待重新授权");
                log::warn!("授权租约过期，已停止网络");
                continue;
            }
        };
        match result {
            Ok(response) => {
                if runtime.instance.is_some() && Instant::now() >= lease_deadline {
                    runtime.stop("offline").await?;
                }
                match response.status.as_str() {
                    "approved" => {
                        ensure!(
                            response.revision > 0
                                && response.lease_seconds > 0
                                && !response.config.is_empty(),
                            "服务端授权响应无效"
                        );
                        let new_deadline =
                            Instant::now() + Duration::from_secs(response.lease_seconds).min(LEASE);
                        remember_secrets(&response.config, &mut *secrets.write().await);
                        let result = tokio::time::timeout_at(
                            new_deadline.min(Instant::now() + Duration::from_secs(15)),
                            runtime.apply(&response.config, response.revision, &identity.id),
                        )
                        .await;
                        match result {
                            Ok(Ok(())) => {
                                lease_deadline = new_deadline;
                                shown = "running".into();
                            }
                            other => {
                                runtime.stop("error").await?;
                                runtime.error = match other {
                                    Ok(Err(e)) => format!("{e:#}"),
                                    _ => "应用配置超时".into(),
                                };
                                runtime.error = log_message(&runtime.error, 8192);
                                log::error!(error = %runtime.error,"应用配置失败");
                                if shown != "error" {
                                    println!("配置应用失败，请在服务端查看日志");
                                    shown = "error".into();
                                }
                            }
                        }
                    }
                    "pending" | "rejected" | "revoked" => {
                        if runtime.instance.is_some() {
                            runtime.stop("stopped").await?;
                        }
                        runtime.error.clear();
                        if shown != response.status {
                            println!(
                                "{}",
                                match response.status.as_str() {
                                    "pending" => "等待服务端审批",
                                    "rejected" => "服务端已拒绝接入",
                                    _ => "接入已撤销，网络已停止",
                                }
                            );
                            shown = response.status;
                        }
                    }
                    _ => {
                        runtime.stop("error").await?;
                        bail!("服务端返回未知审批状态");
                    }
                }
            }
            Err(_) => {
                if Instant::now() >= lease_deadline && runtime.instance.is_some() {
                    runtime.stop("offline").await?;
                    println!("服务端失联，已停止网络，等待重新授权");
                }
            }
        }
        if let Some(instance) = &runtime.instance {
            if instance.state() != CoreInstanceState::Running {
                runtime.stop("error").await?;
                runtime.error = "网络实例意外停止，正在重新拉取配置".into();
            }
        }
    }
}
fn log_message(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].into()
}

pub(super) async fn run(server: &str, token: &str, directory: Option<&Path>) -> anyhow::Result<()> {
    let server = server_url(server)?;
    let (identity, _lock) = load_identity(&server, directory)?;
    let identity = Arc::new(identity);
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()?;
    let receiver = log::init_remote()?;
    let secrets = Arc::new(RwLock::new(vec![token.to_owned(), identity.secret.clone()]));
    let stop = CancellationToken::new();
    let uploader = tokio::spawn(upload_logs(
        client.clone(),
        identity.clone(),
        receiver,
        secrets.clone(),
        stop.clone(),
    ));
    let mut runtime = Runtime {
        manager: Arc::new(native_cli_instance_manager()),
        instance: None,
        applied: 0,
        state: "waiting",
        error: String::new(),
    };
    println!("客户端启动成功");
    #[cfg(unix)]
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let signal = async {
        #[cfg(unix)]
        tokio::select! { _ = tokio::signal::ctrl_c()=>{}, _ = sigterm.recv()=>{} }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
    };
    let result = tokio::select! { result = reconcile(&client,&identity,token,&mut runtime,secrets) => result, _ = signal => Ok(()) };
    let stopped = runtime.stop("stopped").await;
    if result.is_err() {
        log::error!("托管客户端停止，请检查注册 token、服务端响应或设备凭据");
    }
    log::info!("客户端已停止");
    stop.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), uploader).await;
    println!("客户端已停止");
    result.and(stopped)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn managed_addresses_require_tls_and_no_embedded_credentials() {
        assert_eq!(
            server_url("https://example.com").unwrap(),
            "https://example.com"
        );
        assert!(server_url("http://127.0.0.1:8080").is_ok());
        for u in [
            "http://example.com",
            "https://user:pass@example.com",
            "https://example.com/?token=x",
            "https://example.com/prefix",
            "https://example.com/#fragment",
        ] {
            assert!(server_url(u).is_err(), "{u}");
        }
    }
    #[test]
    fn identity_survives_restart_and_prevents_parallel_use() {
        let dir = tempfile::tempdir().unwrap();
        let (first, lock) = load_identity("https://example.com", Some(dir.path())).unwrap();
        assert!(load_identity("https://example.com", Some(dir.path())).is_err());
        drop(lock);
        let (second, _lock) = load_identity("https://example.com", Some(dir.path())).unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(first.secret, second.secret);
    }
    #[test]
    fn managed_configuration_blocks_local_overrides_and_sets_stable_id() {
        let text = "dhcp=true\n[network_identity]\nnetwork_name='test'\nnetwork_secret='a-long-test-secret'\n";
        let id = Uuid::new_v4().to_string();
        assert_eq!(managed_config(text, &id).unwrap().get_id().to_string(), id);
        assert!(managed_config(&format!("{text}\n[core]\ndaemon=true"), &id).is_err());
        let mut secrets = vec![];
        remember_secrets(text, &mut secrets);
        assert!(secrets.contains(&"a-long-test-secret".into()));
    }
}
