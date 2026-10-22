use crate::{
    state::{ffi_context, find_instance_id_by_name},
    *,
};
use easytier::common::config::{ConfigFileControl, ConfigLoader as _, TomlConfigLoader};
use serde_json::Value;
use std::ffi::{CStr, CString, c_char, c_int};
use std::{sync::mpsc, time::Duration};
use uuid::Uuid;

#[test]
fn test_parse_config() {
    let cfg_str = r#"
            inst_name = "test"
            network = "test_network"
        "#;
    let cstr = std::ffi::CString::new(cfg_str).unwrap();
    unsafe {
        assert_eq!(parse_config(cstr.as_ptr()), 0);
    }
}

#[test]
fn test_run_network_instance() {
    let cfg_str = r#"
            inst_name = "test"
            network = "test_network"
        "#;
    let cstr = std::ffi::CString::new(cfg_str).unwrap();
    unsafe {
        assert_eq!(run_network_instance(cstr.as_ptr()), 0);
    }
}

fn take_last_error() -> Option<String> {
    unsafe {
        let mut error_ptr: *const c_char = std::ptr::null();
        get_error_msg(&mut error_ptr);
        if error_ptr.is_null() {
            None
        } else {
            let error = CStr::from_ptr(error_ptr).to_string_lossy().into_owned();
            free_string(error_ptr);
            Some(error)
        }
    }
}

fn free_key_value_pairs(infos: &[KeyValuePair]) {
    for info in infos {
        free_string(info.key);
        free_string(info.value);
    }
}

#[test]
fn list_instance_returns_instance_names_and_ids() {
    let instance_id = Uuid::new_v4();
    let instance_name = format!("list-instance-{}", instance_id);
    let cfg = TomlConfigLoader::default();
    cfg.set_id(instance_id);
    cfg.set_inst_name(instance_name.clone());
    ffi_context()
        .manager
        .run_network_instance(cfg, ConfigFileControl::STATIC_CONFIG)
        .unwrap();

    let mut infos = vec![
        KeyValuePair {
            key: std::ptr::null(),
            value: std::ptr::null(),
        };
        16
    ];
    let count = unsafe { list_instance(infos.as_mut_ptr(), infos.len()) };
    assert!(count > 0);

    let mut found = false;
    for info in infos.iter().take(count as usize) {
        let key = unsafe { CStr::from_ptr(info.key) }.to_string_lossy();
        let value = unsafe { CStr::from_ptr(info.value) }.to_string_lossy();
        if key == instance_name {
            assert_eq!(value, instance_id.to_string());
            found = true;
        }
    }

    free_key_value_pairs(&infos[..count as usize]);
    ffi_context()
        .runtime
        .block_on(
            ffi_context()
                .manager
                .delete_network_instances([instance_id]),
        )
        .unwrap();
    assert!(found);
}

#[test]
fn list_instance_allows_zero_length() {
    assert_eq!(unsafe { list_instance(std::ptr::null_mut(), 0) }, 0);
}

#[test]
fn list_instance_rejects_null_output_pointer() {
    assert_eq!(unsafe { list_instance(std::ptr::null_mut(), 1) }, -1);
    assert!(take_last_error().unwrap().contains("infos is null"));
}

#[test]
fn call_json_rpc_returns_logger_response() {
    let service = CString::new("api.logger.LoggerRpcService").unwrap();
    let method = CString::new("get_logger_config").unwrap();
    let payload = CString::new("{}").unwrap();
    let mut response_ptr: *const c_char = std::ptr::null();

    assert_eq!(
        unsafe {
            call_json_rpc(
                service.as_ptr(),
                method.as_ptr(),
                std::ptr::null(),
                payload.as_ptr(),
                &mut response_ptr,
            )
        },
        0
    );
    assert!(!response_ptr.is_null());
    let response = unsafe { CStr::from_ptr(response_ptr) }
        .to_string_lossy()
        .into_owned();
    free_string(response_ptr);
    let response: Value = serde_json::from_str(&response).unwrap();
    assert!(response.get("level").is_some());
}

#[test]
fn call_json_rpc_rejects_instance_management_service() {
    let service = CString::new("api.manage.ProcessManageRpc").unwrap();
    let method = CString::new("list_network_instance").unwrap();
    let payload = CString::new("{}").unwrap();
    let mut response_ptr: *const c_char = std::ptr::null();

    assert_eq!(
        unsafe {
            call_json_rpc(
                service.as_ptr(),
                method.as_ptr(),
                std::ptr::null(),
                payload.as_ptr(),
                &mut response_ptr,
            )
        },
        -1
    );
    assert!(response_ptr.is_null());
    assert!(take_last_error().unwrap().contains("not exposed"));
}

#[test]
fn call_json_rpc_rejects_malformed_payload_json() {
    let service = CString::new("api.logger.LoggerRpcService").unwrap();
    let method = CString::new("get_logger_config").unwrap();
    let payload = CString::new("{").unwrap();
    let mut response_ptr: *const c_char = std::ptr::null();

    assert_eq!(
        unsafe {
            call_json_rpc(
                service.as_ptr(),
                method.as_ptr(),
                std::ptr::null(),
                payload.as_ptr(),
                &mut response_ptr,
            )
        },
        -1
    );
    assert!(response_ptr.is_null());
    assert!(
        take_last_error()
            .unwrap()
            .contains("failed to parse payload_json")
    );
}

#[test]
fn call_json_rpc_rejects_null_output_pointer() {
    let service = CString::new("api.logger.LoggerRpcService").unwrap();
    let method = CString::new("get_logger_config").unwrap();
    let payload = CString::new("{}").unwrap();

    assert_eq!(
        unsafe {
            call_json_rpc(
                service.as_ptr(),
                method.as_ptr(),
                std::ptr::null(),
                payload.as_ptr(),
                std::ptr::null_mut(),
            )
        },
        -1
    );
    assert!(
        take_last_error()
            .unwrap()
            .contains("out_response_json is null")
    );
}

#[test]
fn find_instance_id_by_name_resolves_uncommitted_manager_instance_name() {
    let instance_id = Uuid::new_v4();
    let inst_name = format!("test-{}", instance_id);
    let cfg = TomlConfigLoader::default();
    cfg.set_id(instance_id);
    cfg.set_inst_name(inst_name.clone());
    ffi_context()
        .manager
        .run_network_instance(cfg, ConfigFileControl::STATIC_CONFIG)
        .unwrap();

    assert_eq!(find_instance_id_by_name(&inst_name), Some(instance_id));
    ffi_context()
        .runtime
        .block_on(
            ffi_context()
                .manager
                .delete_network_instances([instance_id]),
        )
        .unwrap();
}

#[test]
fn delete_network_instance_removes_only_named_instances() {
    let keep_id = Uuid::new_v4();
    let delete_id = Uuid::new_v4();
    let keep_name = format!("keep-{}", keep_id);
    let delete_name = format!("delete-{}", delete_id);

    for (id, name) in [
        (keep_id, keep_name.clone()),
        (delete_id, delete_name.clone()),
    ] {
        let cfg = TomlConfigLoader::default();
        cfg.set_id(id);
        cfg.set_inst_name(name.clone());
        ffi_context()
            .manager
            .run_network_instance(cfg, ConfigFileControl::STATIC_CONFIG)
            .unwrap();
    }

    let delete_name = CString::new(delete_name.clone()).unwrap();
    let inst_names = [delete_name.as_ptr()];
    assert_eq!(
        unsafe { delete_network_instance(inst_names.as_ptr(), inst_names.len()) },
        0
    );

    assert_eq!(find_instance_id_by_name(&keep_name), Some(keep_id));
    assert!(find_instance_id_by_name(delete_name.to_str().unwrap()).is_none());

    ffi_context()
        .runtime
        .block_on(ffi_context().manager.delete_network_instances([keep_id]))
        .unwrap();
}

#[test]
fn retain_and_delete_network_instance_reject_invalid_name_pointers() {
    assert_eq!(unsafe { retain_network_instance(std::ptr::null(), 1) }, -1);
    assert_eq!(unsafe { delete_network_instance(std::ptr::null(), 1) }, -1);

    let inst_names = [std::ptr::null()];
    assert_eq!(
        unsafe { retain_network_instance(inst_names.as_ptr(), inst_names.len()) },
        -1
    );
    assert_eq!(
        unsafe { delete_network_instance(inst_names.as_ptr(), inst_names.len()) },
        -1
    );
}

#[test]
fn ffi_process_management_uses_manager_mutation_lock() {
    let manager_guard = ffi_context().manager.mutation_lock().blocking_lock_owned();
    let (done_tx, done_rx) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        ffi_context()
            .runtime
            .block_on(
                ffi_context()
                    .process_management
                    .delete_owned_network_instances(Vec::new()),
            )
            .unwrap();
        done_tx.send(()).unwrap();
    });

    assert!(done_rx.recv_timeout(Duration::from_millis(100)).is_err());
    drop(manager_guard);
    done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    waiter.join().unwrap();
}

#[test]
fn delete_network_instance_rejects_an_ambiguous_name() {
    let duplicate_name = format!("duplicate-{}", Uuid::new_v4());
    let instance_ids = [Uuid::new_v4(), Uuid::new_v4()];
    for instance_id in instance_ids {
        let config = TomlConfigLoader::default();
        config.set_id(instance_id);
        config.set_inst_name(duplicate_name.clone());
        ffi_context()
            .manager
            .run_network_instance(config, ConfigFileControl::STATIC_CONFIG)
            .unwrap();
    }

    let duplicate_name = CString::new(duplicate_name).unwrap();
    let names = [duplicate_name.as_ptr()];
    assert_eq!(
        unsafe { delete_network_instance(names.as_ptr(), names.len()) },
        -1
    );
    assert!(take_last_error().unwrap().contains("2 instances match"));
    assert!(
        instance_ids
            .iter()
            .all(|id| ffi_context().manager.instance(*id).is_some())
    );

    ffi_context()
        .runtime
        .block_on(
            ffi_context()
                .process_management
                .delete_owned_network_instances(instance_ids.to_vec()),
        )
        .unwrap();
}

#[cfg(feature = "ffi-dataplane")]
#[test]
fn data_plane_invalid_handle_errors_are_stable() {
    let closed = -(easytier_core::gateway::DataPlaneErrorKind::HandleClosed as c_int);
    assert_eq!(data_plane_completion_wait(u64::MAX, 0), closed);
    assert_eq!(data_plane_operation_cancel(u64::MAX, 1), closed);
    assert_eq!(data_plane_operation_free(u64::MAX, 1), closed);
    assert_eq!(data_plane_resource_close(u64::MAX, 1), closed);
    assert_eq!(
        data_plane_resource_deadline_set(u64::MAX, 1, DATA_PLANE_DEADLINE_READ, 0),
        closed
    );
}
