use crate::errors::CliError;
use std::io::IsTerminal;
use std::time::Duration;
use tokio::io::AsyncReadExt;

pub struct CallOptions {
    pub target: String,       // "server/tool"
    pub args: Option<String>, // JSON arguments
    pub config_path: Option<String>,
}

fn parse_target(target: &str) -> Result<(String, String), CliError> {
    let slash_index = target.find('/');
    if let Some(idx) = slash_index {
        Ok((target[..idx].to_string(), target[idx + 1..].to_string()))
    } else {
        Err(crate::errors::invalid_target_error(target))
    }
}

async fn parse_args(args_string: Option<&str>) -> Result<serde_json::Value, CliError> {
    let json_string = if let Some(args) = args_string {
        // 業務動機：過濾傳入參數開頭可能帶有的 UTF-8 BOM 與空白
        args.trim_start_matches('\u{feff}').trim().to_string()
    } else if !std::io::stdin().is_terminal() {
        let mut timeout_ms = crate::config::get_timeout_ms();
        if std::env::var("ANTIGRAVITY_AGENT").is_ok() {
            timeout_ms = 50;
        }
        let mut buffer = Vec::new();
        let mut stdin = tokio::io::stdin();

        let read_fut = stdin.read_to_end(&mut buffer);
        match tokio::time::timeout(Duration::from_millis(timeout_ms), read_fut).await {
            Ok(Ok(_)) => {
                // 業務動機：PowerShell 或某些 Windows 工具管線輸出時，可能包含 UTF-8 BOM (\u{feff}) 或 CRLF (\r\n)，
                // 若未剔除會導致 serde_json 解析無效。在此自動過濾 BOM 並去除頭尾空白字元。
                let s = String::from_utf8_lossy(&buffer);
                s.trim_start_matches('\u{feff}').trim().to_string()
            }
            Ok(Err(e)) => {
                return Err(crate::errors::CliError {
                    code: crate::errors::ErrorCode::ClientError,
                    error_type: "STDIN_READ_FAILED".to_string(),
                    message: format!("Failed to read from stdin: {}", e),
                    details: None,
                    suggestion: None,
                });
            }
            Err(_) => {
                return Err(crate::errors::CliError {
                    code: crate::errors::ErrorCode::ClientError,
                    error_type: "STDIN_TIMEOUT".to_string(),
                    message: format!("stdin read timed out after {}ms", timeout_ms),
                    details: None,
                    suggestion: None,
                });
            }
        }
    } else {
        return Ok(serde_json::json!({}));
    };

    if json_string.is_empty() {
        return Ok(serde_json::json!({}));
    }

    serde_json::from_str(&json_string)
        .map_err(|e| crate::errors::invalid_json_args_error(&json_string, Some(&e.to_string())))
}

pub async fn call_command(options: CallOptions) -> Result<(), CliError> {
    let config = crate::config::load_config(options.config_path.as_deref())?;

    let (server_name, tool_name) = parse_target(&options.target)?;

    let server_config = crate::config::get_server_config(&config, &server_name)?;

    let args = parse_args(options.args.as_deref()).await?;

    let connection = crate::client::get_connection(&server_name, &server_config)
        .await
        .map_err(|e| crate::errors::server_connection_error(&server_name, &e.message))?;

    match connection.call_tool(&tool_name, args).await {
        Ok(result) => {
            println!("{}", crate::output::format_tool_result(&result));
            let _ = connection.close().await;
            Ok(())
        }
        Err(error) => {
            let mut available_tools = None;
            if let Ok(tools) = connection.list_tools().await {
                available_tools = Some(tools.into_iter().map(|t| t.name).collect::<Vec<String>>());
            }

            let _ = connection.close().await;

            let err_msg = error.message;
            let err = if err_msg.contains("not found") || err_msg.contains("unknown tool") {
                crate::errors::tool_not_found_error(
                    &tool_name,
                    &server_name,
                    available_tools.as_deref(),
                )
            } else {
                crate::errors::tool_execution_error(&tool_name, &server_name, &err_msg)
            };

            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_parse_args_with_bom() {
        // 業務動機：驗證來自 PowerShell 或特定 Windows 編輯器的 UTF-8 BOM 開頭 JSON 能正常被解析
        let bom_json = "\u{feff}{\"key\": \"value\"}";
        let parsed = parse_args(Some(bom_json)).await.unwrap();
        assert_eq!(parsed["key"], "value");
    }

    #[tokio::test]
    async fn test_parse_args_empty_and_normal() {
        let empty = parse_args(Some("")).await.unwrap();
        assert_eq!(empty, serde_json::json!({}));

        let normal = parse_args(Some("{\"count\": 42}")).await.unwrap();
        assert_eq!(normal["count"], 42);
    }
}
