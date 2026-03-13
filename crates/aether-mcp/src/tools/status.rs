use aether_core::normalize_path;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{AetherMcpServer, MCP_SCHEMA_VERSION, current_unix_timestamp};
use crate::AetherMcpError;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherStatusResponse {
    pub schema_version: u32,
    pub generated_at: i64,
    pub workspace: String,
    pub store_present: bool,
    pub sqlite_path: String,
    pub sir_dir: String,
    pub symbol_count: i64,
    pub sir_count: i64,
    pub sir_coverage: SirCoverageStats,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SirCoverageStats {
    pub total_symbols: u64,
    pub symbols_with_sir: u64,
    pub percentage: f64,
}

impl AetherMcpServer {
    pub fn aether_status_logic(&self) -> Result<AetherStatusResponse, AetherMcpError> {
        let sqlite_path = self.sqlite_path();
        let sir_dir = self.sir_dir();
        let store_present = sqlite_path.exists() && sir_dir.is_dir();

        let (symbol_count, sir_count, sir_coverage) = if store_present {
            let (total_symbols, symbols_with_sir) = self.state.store.count_symbols_with_sir()?;
            let percentage = if total_symbols > 0 {
                (symbols_with_sir as f64 / total_symbols as f64).clamp(0.0, 1.0)
            } else {
                0.0
            };
            (
                total_symbols as i64,
                symbols_with_sir as i64,
                SirCoverageStats {
                    total_symbols: total_symbols as u64,
                    symbols_with_sir: symbols_with_sir as u64,
                    percentage,
                },
            )
        } else {
            (
                0,
                0,
                SirCoverageStats {
                    total_symbols: 0,
                    symbols_with_sir: 0,
                    percentage: 0.0,
                },
            )
        };

        Ok(AetherStatusResponse {
            schema_version: MCP_SCHEMA_VERSION,
            generated_at: current_unix_timestamp(),
            workspace: normalize_path(&self.state.workspace.to_string_lossy()),
            store_present,
            sqlite_path: normalize_path(&sqlite_path.to_string_lossy()),
            sir_dir: normalize_path(&sir_dir.to_string_lossy()),
            symbol_count,
            sir_count,
            sir_coverage,
        })
    }
}
