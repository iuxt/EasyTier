pub mod config;
pub mod constants;
#[cfg(feature = "management")]
pub mod credential_manager;
pub mod dns;
pub mod error;
pub mod global_ctx;
pub mod ifcfg;
#[cfg(feature = "logging")]
pub mod log;
pub mod netns;
pub mod network;
pub mod stun;
#[cfg(feature = "management")]
pub mod tracing_rolling_appender;
#[cfg(feature = "upnp")]
pub mod upnp;

pub fn shrink_dashmap<K: Eq + std::hash::Hash, V>(
    map: &dashmap::DashMap<K, V>,
    threshold: Option<usize>,
) {
    let threshold = threshold.unwrap_or(16);
    if map.capacity() - map.len() > threshold {
        map.shrink_to_fit();
    }
}
