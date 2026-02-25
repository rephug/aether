pub mod config;
pub mod health;
pub mod server;

pub type DynError = Box<dyn std::error::Error + Send + Sync>;
