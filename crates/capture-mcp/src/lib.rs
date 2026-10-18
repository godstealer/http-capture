//! MCP adapters share the application's existing capture runtime.
pub mod server;
pub mod client;
pub use server::{ServerConfig, ServerManager, ServerStatus};
pub use client::{ClientInput, ClientManager, ClientStatus};
