//! Aether Core Library
//! 
//! This crate provides shared types, traits, and utilities used across
//! the Aether project for dynamically exposing REST APIs as MCP endpoints.

pub mod error;
pub mod types;

pub use error::{AetherError, Result};