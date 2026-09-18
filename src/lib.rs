//! Cohesive modular monolith for the device-local community backend.
//!
//! Dependency direction is enforced by convention and imports:
//! `domain <- ports <- application <- adapters <- composition root`.

#[cfg(not(target_os = "linux"))]
compile_error!(
    "the native community-stack core currently supports Linux only; use a reviewed platform IPC adapter for other targets"
);

pub mod adapters;
pub mod application;
pub mod config;
pub mod domain;
pub mod ports;

pub mod recovery;
