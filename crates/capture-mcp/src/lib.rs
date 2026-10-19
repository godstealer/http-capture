//! MCP adapters share the application's existing capture runtime.
pub mod server;
pub mod client;
pub mod bridge;
pub mod connections;
pub mod workflows;
pub mod rules;
pub use server::{ServerConfig, ServerManager, ServerStatus};
pub use client::{ClientInput, ClientManager, ClientStatus};
