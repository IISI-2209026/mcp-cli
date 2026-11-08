//! Rust library for connecting to MCP servers and invoking MCP tools.
//!
//! The crate also ships the `mcp-cli` binary. Library consumers can use
//! [`McpClient`] for the common workflow or the lower-level modules for direct
//! control over configuration, connections, and output formatting.

pub mod api;
pub mod client;
pub mod commands;
pub mod config;
pub mod daemon;
pub mod daemon_client;
pub mod errors;
pub mod output;

pub use api::McpClient;
pub use client::{get_connection, HttpClient, McpConnection, StdioClient};
pub use config::{HttpServerConfig, McpServersConfig, ServerConfig, StdioServerConfig};
pub use errors::{CliError, ErrorCode};
pub use output::{ServerInfo, ToolInfo};

/// 編譯期內嵌的 Agent Skill 定義檔案內容（SKILL.md）。
///
/// 業務動機：
/// 1. 讓 CLI 工具在任何環境（例如透過 cargo install 安裝至系統全域路徑後）
///    都能獨立取得自身的 Skill 說明文件，無須依賴當前工作目錄是否存在 SKILL.md。
/// 2. 提供給外部程式庫使用者，可直接讀取本工具所規範的 Agent 指令與說明文件。
pub const SKILL_MD: &str = include_str!("../SKILL.md");

/// Convenient imports for common library usage.
pub mod prelude {
    pub use crate::{
        CliError, ErrorCode, HttpServerConfig, McpClient, McpConnection, McpServersConfig,
        ServerConfig, StdioServerConfig, ToolInfo, SKILL_MD,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 驗證內嵌的 SKILL.md 是否正確載入且具備完整的 Skill Frontmatter。
    ///
    /// 業務動機：
    /// 確保編譯期讀取的 SKILL.md 內容不為空，且包含必要的 metadata（如 name 與 description），
    /// 避免發布時意外破壞 Agent Skill 規格。
    #[test]
    fn test_skill_md_embedded_content() {
        assert!(!SKILL_MD.is_empty(), "SKILL.md 內容不應為空");
        assert!(
            SKILL_MD.contains("name: mcp-cli"),
            "SKILL.md 應包含 name: mcp-cli frontmatter"
        );
        assert!(
            SKILL_MD.contains("description:"),
            "SKILL.md 應包含 description frontmatter"
        );
    }
}
