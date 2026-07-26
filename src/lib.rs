//! Cohesive modular monolith for the device-local community backend.
//!
//! Dependency direction is enforced by convention and imports:
//! `domain <- ports <- application <- adapters <- composition root`.

pub mod adapters;
pub mod application;
pub mod config;
pub mod domain;
pub mod ports;
