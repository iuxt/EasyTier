macro_rules! ohrs_log_error {
    ($($arg:tt)*) => {{
        if $crate::platform::logging::log_manager::app_log_enabled(5) {
            $crate::platform::logging::log_manager::record_app_log(
                5,
                "RustOhrs",
                &std::format!($($arg)*),
            );
        }
    }};
}

macro_rules! ohrs_log_info {
    ($($arg:tt)*) => {{
        if $crate::platform::logging::log_manager::app_log_enabled(4) {
            $crate::platform::logging::log_manager::record_app_log(
                4,
                "RustOhrs",
                &std::format!($($arg)*),
            );
        }
    }};
}

mod exports;
mod kernel_bridge;
mod napi_types;
mod nearby_management;
mod platform;

use config::repository::cache_runtime_config_snapshot;
use config::services::schema_service::{
    get_network_config_field_mappings as build_network_config_field_mappings,
    get_network_config_schema as build_network_config_schema,
};
use config::services::share_link_service::{
    build_config_share_link as build_config_share_link_inner,
    import_config_share_link as import_config_share_link_inner,
    parse_config_share_link as parse_config_share_link_inner,
};
use config::storage::config_meta::get_config_display_name;
use easytier::common::config::NetworkConfigExt;
use easytier::common::config::{ConfigLoader, TomlConfigLoader};
use easytier::common::constants::EASYTIER_VERSION;
use easytier::proto::api::manage::NetworkConfig;
use easytier_ohos_core::runtime;
use easytier_ohos_core::{ASYNC_RUNTIME, INSTANCE_MANAGER};
use easytier_ohos_features::config;
use kernel_bridge::{
    start_local_socket_server as start_local_socket_server_inner,
    stop_local_socket_server as stop_local_socket_server_inner,
};
use napi_derive_ohos::napi;
use napi_ohos::bindgen_prelude::Uint8Array;
use napi_types::{
    ConfigFieldMapping, KeyValuePair, NetworkConfigSchema, SharedConfigLinkPayload,
    SnapshotImportResult, SocketProtectionRequest,
};
use runtime::state::runtime_state::{RuntimeAggregateState, RuntimeInstanceState};
use std::format;
use uuid::Uuid;

pub(crate) fn feature_log_sink(level: i32, target: &str, message: &str) {
    platform::logging::log_manager::record_app_log(level, target, message);
}

pub(crate) fn feature_log_enabled(level: i32) -> bool {
    platform::logging::log_manager::app_log_enabled(level)
}

fn ensure_local_socket_server_started() -> bool {
    start_local_socket_server_inner()
}

fn maybe_stop_local_socket_server() {
    let no_local_instances = INSTANCE_MANAGER.instance_ids().is_empty();
    if no_local_instances {
        let _ = stop_local_socket_server_inner();
    }
}

fn stop_runtime_inner() -> bool {
    let mut ok = true;
    let ids = INSTANCE_MANAGER.instance_ids();
    if !ids.is_empty() {
        ok = ASYNC_RUNTIME
            .block_on(INSTANCE_MANAGER.delete_network_instances(ids))
            .map(|_| true)
            .unwrap_or_else(|err| {
                ohrs_log_error!("[Rust] stop runtime instances failed {}", err);
                false
            })
            && ok;
    }
    maybe_stop_local_socket_server();
    let _ = nearby_management::stop_runtime_management_server();
    ok
}

fn runtime_instance_label(instance: &RuntimeInstanceState) -> String {
    let display_name = instance.display_name.trim();
    if !display_name.is_empty() {
        return display_name.to_string();
    }
    let instance_id = instance.instance_id.trim();
    if !instance_id.is_empty() {
        return instance_id.to_string();
    }
    instance.config_id.clone()
}

fn runtime_instance_matches(instance: &RuntimeInstanceState, selector: &str) -> bool {
    let target = selector.trim();
    if target.is_empty() {
        return false;
    }
    instance.instance_id == target
        || instance.config_id == target
        || instance.display_name == target
        || runtime_instance_label(instance) == target
}

fn read_json_string_path<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a str> {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(*key)?;
    }
    cursor.as_str().filter(|value| !value.trim().is_empty())
}

fn selected_instance_from_payload(payload_json: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(payload_json).ok()?;
    for path in [
        &["instance", "instance_selector", "name"][..],
        &["instance", "instanceSelector", "name"][..],
        &["instance", "instance_selector", "id"][..],
        &["instance", "instanceSelector", "id"][..],
        &["instance", "name"][..],
        &["instance", "id"][..],
        &["instance_name"][..],
        &["instanceName"][..],
        &["instance_id"][..],
        &["instanceId"][..],
        &["id"][..],
    ] {
        if let Some(value) = read_json_string_path(&value, path) {
            return Some(value.to_string());
        }
    }
    None
}

fn find_runtime_instance<'a>(
    state: &'a RuntimeAggregateState,
    selector: Option<&str>,
) -> Option<&'a RuntimeInstanceState> {
    if let Some(selector) = selector
        && let Some(instance) = state
            .instances
            .iter()
            .find(|instance| runtime_instance_matches(instance, selector))
    {
        return Some(instance);
    }
    state.instances.iter().find(|instance| instance.running)
}

fn list_instances_json_inner(state: &RuntimeAggregateState) -> String {
    let mut instances = serde_json::Map::new();
    for instance in state.instances.iter().filter(|instance| instance.running) {
        let label = runtime_instance_label(instance);
        if !label.trim().is_empty() {
            instances.insert(
                label,
                serde_json::Value::String(instance.instance_id.clone()),
            );
        }
    }
    serde_json::Value::Object(instances).to_string()
}

fn call_json_rpc_inner(service_name: &str, method_name: &str, payload_json: &str) -> String {
    let state = collect_runtime_state_inner();
    let selector = selected_instance_from_payload(payload_json);
    let Some(instance) = find_runtime_instance(&state, selector.as_deref()) else {
        return "{}".to_string();
    };

    let method = method_name.trim();
    let service = service_name.trim();
    let response = match (service, method) {
        (_, "show_node_info") => serde_json::json!({
            "node_info": instance.my_node_info,
        }),
        (_, "list_route") => serde_json::json!({
            "routes": instance.routes,
        }),
        (_, "list_peer") => serde_json::json!({
            "my_info": instance.my_node_info,
            "peer_infos": instance.peers,
        }),
        (_, "get_stats") => {
            let mut rx_bytes = 0_i64;
            let mut tx_bytes = 0_i64;
            for peer in &instance.peers {
                for conn in &peer.conns {
                    if let Some(stats) = &conn.stats {
                        rx_bytes = rx_bytes.saturating_add(stats.rx_bytes);
                        tx_bytes = tx_bytes.saturating_add(stats.tx_bytes);
                    }
                }
            }
            let network_name = runtime_instance_label(instance);
            serde_json::json!({
                "metrics": [
                    {
                        "name": "traffic_bytes_self_rx",
                        "labels": { "network_name": network_name },
                        "value": rx_bytes,
                    },
                    {
                        "name": "traffic_bytes_self_tx",
                        "labels": { "network_name": network_name },
                        "value": tx_bytes,
                    }
                ]
            })
        }
        _ => serde_json::json!({}),
    };
    response.to_string()
}

fn resolve_instance_id_from_state(
    state: &RuntimeAggregateState,
    instance_name: &str,
) -> Option<String> {
    let instance = state
        .instances
        .iter()
        .find(|instance| runtime_instance_matches(instance, instance_name))?;
    Some(instance.instance_id.clone())
}

fn resolve_instance_id_inner(instance_name: &str) -> Option<String> {
    resolve_instance_id_from_state(&collect_runtime_state_inner(), instance_name)
}

fn convert_toml_to_network_config_inner(toml_text: &str) -> Result<String, String> {
    let config = NetworkConfig::new_from_config(
        TomlConfigLoader::new_from_str(toml_text).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    serde_json::to_string(&config).map_err(|e| e.to_string())
}

fn parse_network_config_inner(cfg_json: &str) -> bool {
    serde_json::from_str::<NetworkConfig>(cfg_json)
        .ok()
        .and_then(|cfg| cfg.gen_config().ok())
        .is_some()
}

pub(crate) fn run_network_instance_from_json(cfg_json: &str) -> bool {
    let config = match serde_json::from_str::<NetworkConfig>(cfg_json) {
        Ok(cfg) => cfg,
        Err(e) => {
            ohrs_log_error!("[Rust] parse config failed {}", e);
            return false;
        }
    };

    let cfg = match config.gen_config() {
        Ok(toml) => toml,
        Err(e) => {
            ohrs_log_error!("[Rust] parse config failed {}", e);
            return false;
        }
    };

    if !INSTANCE_MANAGER.instance_ids().is_empty() {
        ohrs_log_error!("[Rust] there is a running instance!");
        return false;
    }

    if !ensure_local_socket_server_started() {
        return false;
    }

    let inst_id = cfg.get_id();
    if INSTANCE_MANAGER.instance_ids().contains(&inst_id) {
        ohrs_log_error!("[Rust] instance {} already exists", inst_id);
        return false;
    }

    let config_control = nearby_management::runtime_management_config_control(inst_id);
    if !nearby_management::ensure_runtime_management_server_started() {
        return false;
    }

    match INSTANCE_MANAGER.run_network_instance(cfg, config_control) {
        Ok(_) => {
            cache_runtime_config_snapshot(inst_id.to_string(), inst_id.to_string(), config);
            true
        }
        Err(err) => {
            ohrs_log_error!("[Rust] start_kernel failed for {}: {}", inst_id, err);
            false
        }
    }
}

fn start_kernel_with_config_id(config_id: &str) -> bool {
    let Some(raw) = config::repository::load_config_json(config_id) else {
        return false;
    };
    let display_name = get_config_display_name(config_id).unwrap_or_else(|| config_id.to_string());
    let started = run_network_instance_from_json(&raw);
    if started && let Ok(config) = serde_json::from_str::<NetworkConfig>(&raw) {
        cache_runtime_config_snapshot(config_id.to_string(), display_name, config);
    }
    started
}

fn parse_instance_uuid(config_id: &str) -> Option<Uuid> {
    match Uuid::parse_str(config_id) {
        Ok(uuid) => Some(uuid),
        Err(err) => {
            ohrs_log_error!("[Rust] invalid config_id {}: {}", config_id, err);
            None
        }
    }
}

#[napi]
pub fn init_config_store(root_dir: String) -> bool {
    exports::config_api::init_config_store(root_dir)
}

#[napi]
pub fn reset_config_store() -> bool {
    exports::config_api::reset_config_store()
}

#[napi]
pub fn list_configs() -> String {
    exports::config_api::list_configs()
}

#[napi]
pub fn get_config_display_name_by_id(config_id: String) -> Option<String> {
    get_config_display_name(&config_id)
}

#[napi]
pub fn save_config(config_id: String, display_name: String, config_json: String) -> bool {
    exports::config_api::save_config(config_id, display_name, config_json)
}

#[napi]
pub fn create_config(config_id: String, display_name: String) -> bool {
    exports::config_api::create_config(config_id, display_name)
}

#[napi]
pub fn rename_stored_config(config_id: String, display_name: String) -> bool {
    config::storage::config_meta::set_config_display_name(config_id, display_name).is_some()
}

#[napi]
pub fn delete_stored_config_meta(config_id: String) -> bool {
    exports::config_api::delete_stored_config_meta(config_id)
}

#[napi]
pub fn get_config(config_id: String) -> Option<String> {
    exports::config_api::get_config(config_id)
}

#[napi]
pub fn get_default_config() -> Option<String> {
    exports::config_api::get_default_config()
}

#[napi]
pub fn get_config_field(config_id: String, field: String) -> Option<String> {
    exports::config_api::get_config_field(config_id, field)
}

#[napi]
pub fn set_config_field(config_id: String, field: String, json_value: String) -> bool {
    exports::config_api::set_config_field(config_id, field, json_value)
}

#[napi]
pub fn set_config_favorite(config_id: String, favorite: bool) -> bool {
    exports::config_api::set_config_favorite(config_id, favorite)
}

#[napi]
pub fn import_toml(toml_text: String, display_name: Option<String>) -> Option<String> {
    exports::config_api::import_toml(toml_text, display_name)
}

#[napi]
pub fn export_toml(config_id: String) -> Option<String> {
    exports::config_api::export_toml(config_id)
}

#[napi]
pub fn export_config_store_snapshot(target_path: String) -> bool {
    exports::config_api::export_config_store_snapshot(target_path)
}

#[napi]
pub fn import_config_store_snapshot(source_path: String) -> bool {
    exports::config_api::import_config_store_snapshot(source_path)
}

#[napi]
pub fn import_config_store_snapshot_with_result(source_path: String) -> SnapshotImportResult {
    exports::config_api::import_config_store_snapshot_with_result(source_path).into()
}

#[napi]
pub fn start_kernel(config_id: String) -> bool {
    exports::runtime_api::start_kernel(config_id, start_kernel_with_config_id)
}

#[napi]
pub fn stop_kernel(config_id: String) -> bool {
    exports::runtime_api::stop_kernel(
        config_id,
        parse_instance_uuid,
        maybe_stop_local_socket_server,
    )
}

#[napi]
pub fn stop_network_instance(config_ids: Vec<String>) -> bool {
    exports::runtime_api::stop_network_instance(config_ids, stop_kernel)
}

#[napi]
pub fn stop_runtime() -> bool {
    stop_runtime_inner()
}

#[napi]
pub fn collect_runtime_state_json() -> String {
    serde_json::to_string(&collect_runtime_state_inner()).unwrap_or_else(|_| "{}".to_string())
}

#[napi]
pub fn list_instances_json() -> String {
    list_instances_json_inner(&collect_runtime_state_inner())
}

#[napi]
pub fn call_json_rpc(service_name: String, method_name: String, payload_json: String) -> String {
    call_json_rpc_inner(&service_name, &method_name, &payload_json)
}

#[napi]
pub fn resolve_instance_id(instance_name: String) -> Option<String> {
    resolve_instance_id_inner(&instance_name)
}

#[napi]
pub fn easytier_version() -> String {
    EASYTIER_VERSION.to_string()
}

#[napi]
pub fn default_network_config() -> String {
    get_default_config().unwrap_or_else(|| "{}".to_string())
}

#[napi]
pub fn convert_toml_to_network_config(toml_text: String) -> String {
    convert_toml_to_network_config_inner(&toml_text).unwrap_or_else(|err| format!("ERROR: {err}"))
}

#[napi]
pub fn parse_network_config(cfg_json: String) -> bool {
    parse_network_config_inner(&cfg_json)
}

#[napi]
pub fn run_network_instance(cfg_json: String) -> bool {
    run_network_instance_from_json(&cfg_json)
}

/// Starts the management server in the VPN Extension process even when no
/// network instance is active, allowing a nearby controller to deploy a
/// one-shot config through the canonical Core RPC surface.
#[napi]
pub fn start_nearby_management_host() -> bool {
    nearby_management::ensure_runtime_management_server_started()
}

#[napi]
pub fn stop_nearby_management_host() -> bool {
    nearby_management::stop_runtime_management_server()
}

#[napi]
pub fn drain_nearby_host_commands() -> Vec<nearby_management::NearbyHostCommand> {
    nearby_management::drain_nearby_host_commands()
}

#[napi]
pub fn complete_nearby_host_command(
    request_id: String,
    success: bool,
    error: Option<String>,
) -> bool {
    nearby_management::complete_nearby_host_command(request_id, success, error)
}

/// Returns 1 for canonical Core RPC packets, 2 for the OHOS-private settings
/// envelope, and 0 for malformed or unsupported data.
#[napi]
pub fn nearby_management_packet_kind(packet: Uint8Array) -> i32 {
    nearby_management::nearby_management_packet_kind(packet)
}

#[napi]
pub fn encode_nearby_ohos_packet(envelope_json: String) -> Option<Uint8Array> {
    nearby_management::encode_nearby_ohos_packet(envelope_json)
}

#[napi]
pub fn decode_nearby_ohos_packet(packet: Uint8Array) -> Option<String> {
    nearby_management::decode_nearby_ohos_packet(packet)
}

/// Opens one Core RPC endpoint for a HarmonyOS collaboration session.
///
/// The Harmony layer transports the returned native packets verbatim with
/// `abilityConnectionManager.sendData`; all RPC framing stays inside Core.
#[napi]
pub fn open_nearby_management_session(session_key: String, host: bool) -> bool {
    nearby_management::open_nearby_management_session(session_key, host)
}

#[napi]
pub fn close_nearby_management_session(session_key: String) -> bool {
    nearby_management::close_nearby_management_session(session_key)
}

#[napi]
pub fn push_nearby_management_packet(session_key: String, packet: Uint8Array) -> bool {
    nearby_management::push_nearby_management_packet(session_key, packet)
}

#[napi]
pub fn drain_nearby_management_packets(session_key: String) -> Vec<Uint8Array> {
    nearby_management::drain_nearby_management_packets(session_key)
}

#[napi]
pub async fn call_nearby_management_json_rpc(
    session_key: String,
    service_name: String,
    method_name: String,
    domain_name: Option<String>,
    payload_json: String,
) -> String {
    nearby_management::call_nearby_management_json_rpc(
        session_key,
        service_name,
        method_name,
        domain_name,
        payload_json,
    )
    .await
}

#[napi]
pub fn collect_network_infos() -> Vec<KeyValuePair> {
    exports::runtime_api::collect_network_infos()
        .into_iter()
        .map(Into::into)
        .collect()
}

#[napi]
pub fn set_tun_fd(config_id: String, fd: i32) -> bool {
    exports::runtime_api::set_tun_fd(config_id, fd, parse_instance_uuid)
}

#[napi]
pub fn enable_socket_protection() -> bool {
    easytier_ohos_core::socket_protection::enable_socket_protection()
}

#[napi]
pub async fn next_socket_protection_request() -> Option<SocketProtectionRequest> {
    easytier_ohos_core::socket_protection::SOCKET_PROTECTION_MANAGER
        .next_request()
        .await
        .map(Into::into)
}

#[napi]
pub fn complete_socket_protection(
    request_id: String,
    success: bool,
    error: Option<String>,
) -> bool {
    let Ok(request_id) = request_id.parse::<u64>() else {
        return false;
    };
    easytier_ohos_core::socket_protection::SOCKET_PROTECTION_MANAGER
        .complete_request(request_id, success, error)
}

#[napi]
pub fn disable_socket_protection() -> bool {
    easytier_ohos_core::socket_protection::disable_socket_protection()
}

#[napi]
pub fn fail_socket_protection() -> bool {
    easytier_ohos_core::socket_protection::fail_socket_protection()
}

#[napi]
pub fn get_network_config_schema() -> NetworkConfigSchema {
    build_network_config_schema().into()
}

#[napi]
pub fn get_network_config_field_mappings() -> Vec<ConfigFieldMapping> {
    build_network_config_field_mappings()
        .into_iter()
        .map(Into::into)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exported_plain_object_schema_contains_core_networkconfig_metadata() {
        let schema = get_network_config_schema();
        assert_eq!(schema.name, "NetworkConfig");
        assert_eq!(schema.node_kind, "schema");
        assert!(
            schema
                .children
                .iter()
                .any(|field| field.name == "network_name")
        );
        let secure_mode = schema
            .children
            .iter()
            .find(|field| field.name == "secure_mode")
            .expect("secure_mode field");
        assert!(
            secure_mode
                .children
                .iter()
                .any(|field| field.name == "enabled")
        );
    }

    fn local_test_state() -> RuntimeAggregateState {
        RuntimeAggregateState {
            instances: vec![
                RuntimeInstanceState {
                    config_id: "0c4b33ba-4ed5-42d8-9095-21b786c66e94".to_string(),
                    instance_id: "0c4b33ba-4ed5-42d8-9095-21b786c66e94".to_string(),
                    display_name: "0c4b33ba-4ed5-42d8-9095-21b786c66e94".to_string(),
                    running: true,
                    tun_required: false,
                    tun_attached: false,
                    magic_dns_enabled: false,
                    need_exit_node: false,
                    error_message: None,
                    my_node_info: None,
                    events: vec![],
                    routes: vec![],
                    peers: vec![],
                    manual_routes: vec![],
                },
                RuntimeInstanceState {
                    config_id: "ec7b6a3c-aeae-4c0e-844e-f7ec2dbdc2ce".to_string(),
                    instance_id: "ec7b6a3c-aeae-4c0e-844e-f7ec2dbdc2ce".to_string(),
                    display_name: "ec7b6a3c-aeae-4c0e-844e-f7ec2dbdc2ce".to_string(),
                    running: true,
                    tun_required: false,
                    tun_attached: false,
                    magic_dns_enabled: false,
                    need_exit_node: false,
                    error_message: None,
                    my_node_info: None,
                    events: vec![],
                    routes: vec![],
                    peers: vec![],
                    manual_routes: vec![],
                },
            ],
            tun: runtime::state::runtime_state::TunAggregateState {
                active: false,
                attached_instance_ids: vec![],
                aggregated_routes: vec![],
                dns_servers: vec![],
                need_rebuild: false,
            },
            running_instance_count: 2,
        }
    }

    #[test]
    fn resolve_instance_id_does_not_fall_back_for_unknown_selector() {
        let state = local_test_state();
        assert_eq!(
            resolve_instance_id_from_state(&state, "0c4b33ba-4ed5-42d8-9095-21b786c66e94"),
            Some("0c4b33ba-4ed5-42d8-9095-21b786c66e94".to_string()),
        );
        assert_eq!(resolve_instance_id_from_state(&state, "stale-name"), None);
    }
}

pub(crate) fn collect_runtime_state_inner() -> RuntimeAggregateState {
    exports::runtime_api::collect_runtime_state()
}

#[napi]
pub fn build_config_share_link(config_id: String, only_start: Option<bool>) -> Option<String> {
    build_config_share_link_inner(&config_id, None, only_start.unwrap_or(false))
}

#[napi]
pub fn parse_config_share_link(share_link: String) -> Option<SharedConfigLinkPayload> {
    parse_config_share_link_inner(&share_link).map(Into::into)
}

#[napi]
pub fn import_config_share_link(
    share_link: String,
    display_name_override: Option<String>,
) -> Option<String> {
    import_config_share_link_inner(&share_link, display_name_override)
}
