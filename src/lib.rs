//! Shared application models and feature-gated device driver runtime.
pub mod adapters;
pub mod audio_fx;
pub mod audio_lighting;
pub mod backend;
#[cfg(feature = "desktop")]
pub mod configuration;
pub mod controls;
pub mod devices;
pub mod hid;
#[cfg(feature = "driver-runtime")]
pub mod keyboard;
#[cfg(feature = "driver-runtime")]
pub mod keyboard_controls;
pub mod model;
#[cfg(feature = "driver-runtime")]
pub mod plugin_entry;
#[cfg(feature = "desktop")]
pub mod plugin_manager;
pub mod profiles;
pub mod protocol;
pub mod storage;

#[cfg(feature = "desktop")]
pub mod core_update;

#[cfg(feature = "desktop")]
pub mod resident_macros;
