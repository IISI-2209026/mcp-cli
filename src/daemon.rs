use crate::config::{
    debug, get_config_hash, get_daemon_timeout_ms, get_pid_path, get_socket_dir, get_socket_path,
    ServerConfig,
};
use crate::errors::{CliError, ErrorCode};
use std::fs;
#[cfg(windows)]
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(windows)]
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::net::UnixListener;
use tokio::sync::mpsc;

// ============================================================================
// Types
// ============================================================================

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DaemonRequest {
    pub id: String,
    #[serde(rename = "type")]
    pub req_type: String,
    pub tool_name: Option<String>,
    pub args: Option<serde_json::Value>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct DaemonResponse {
    pub id: String,
    pub success: bool,
    pub data: Option<serde_json::Value>,
    pub error: Option<DaemonErrorDetail>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct DaemonErrorDetail {
    pub code: String,
    pub message: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PidFileContent {
    pub pid: i32,
    pub config_hash: String,
    pub started_at: String,
}

// ============================================================================
// PID File Management
// ============================================================================

pub fn write_pid_file(server_name: &str, config_hash: &str) {
    let pid_path = get_pid_path(server_name);
    let dir = pid_path.parent().unwrap();

    if !dir.exists() {
        let _ = fs::create_dir_all(dir);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
    }

    let content = PidFileContent {
        pid: std::process::id() as i32,
        config_hash: config_hash.to_string(),
        started_at: chrono::Utc::now().to_rfc3339(),
    };

    if let Ok(serialized) = serde_json::to_string(&content) {
        let _ = fs::write(&pid_path, serialized);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&pid_path, fs::Permissions::from_mode(0o600));
        }
    }
}

pub fn read_pid_file(server_name: &str) -> Option<PidFileContent> {
    let pid_path = get_pid_path(server_name);
    if !pid_path.exists() {
        return None;
    }

    let content = fs::read_to_string(pid_path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn remove_pid_file(server_name: &str) {
    let pid_path = get_pid_path(server_name);
    let _ = fs::remove_file(pid_path);
}

pub fn remove_socket_file(server_name: &str) {
    let socket_path = get_socket_path(server_name);
    let _ = fs::remove_file(socket_path);
}

#[cfg(windows)]
pub fn get_daemon_addr(server_name: &str, config_hash: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    server_name.hash(&mut hasher);
    config_hash.hash(&mut hasher);
    let port = 40000 + (hasher.finish() % 20000);
    format!("127.0.0.1:{}", port)
}

/// 業務動機：檢測指定的 PID 程序是否仍在背景執行中。
/// Unix 平台透過向進程發送信號 0 (libc::kill(pid, 0)) 進行非侵入式檢測；
/// Windows 平台透過 OpenProcess 與 GetExitCodeProcess 檢查進程退出代碼是否為 STILL_ACTIVE。
#[cfg(unix)]
pub fn is_process_running(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(windows)]
pub fn is_process_running(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    use windows_sys::Win32::Foundation::{CloseHandle, FALSE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, STILL_ACTIVE,
    };

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid as u32);
        if handle == 0 {
            return false;
        }
        let mut exit_code: u32 = 0;
        let success = GetExitCodeProcess(handle, &mut exit_code);
        CloseHandle(handle);
        success != 0 && exit_code == STILL_ACTIVE as u32
    }
}

#[cfg(not(any(unix, windows)))]
pub fn is_process_running(_pid: i32) -> bool {
    false
}

/// 業務動機：強制終止指定的 PID 背景程序。
/// Unix 平台發送 SIGTERM 信號進行優雅終止；
/// Windows 平台使用 TerminateProcess 進行強制終止，確保配置雜湊不符或殘留進程能被確實清理。
#[cfg(unix)]
pub fn kill_process(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    unsafe { libc::kill(pid, libc::SIGTERM) == 0 }
}

#[cfg(windows)]
pub fn kill_process(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    use windows_sys::Win32::Foundation::{CloseHandle, FALSE};
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, FALSE, pid as u32);
        if handle == 0 {
            return false;
        }
        let success = TerminateProcess(handle, 1);
        CloseHandle(handle);
        success != 0
    }
}

#[cfg(not(any(unix, windows)))]
pub fn kill_process(_pid: i32) -> bool {
    false
}

// ============================================================================
// Daemon Connection Handler
// ============================================================================

async fn handle_request(
    data: &[u8],
    mcp_connection: &crate::client::McpConnection,
    shutdown_tx: &mpsc::Sender<()>,
) -> DaemonResponse {
    let req: DaemonRequest = match serde_json::from_slice(data) {
        Ok(r) => r,
        Err(_) => {
            return DaemonResponse {
                id: "unknown".to_string(),
                success: false,
                data: None,
                error: Some(DaemonErrorDetail {
                    code: "INVALID_REQUEST".to_string(),
                    message: "Invalid JSON".to_string(),
                }),
            };
        }
    };

    debug(&format!("Daemon Request: {} ({})", req.req_type, req.id));

    let result = match req.req_type.as_str() {
        "ping" => Ok(serde_json::json!("pong")),
        "listTools" => match mcp_connection.list_tools().await {
            Ok(tools) => serde_json::to_value(tools).map_err(|e| e.to_string()),
            Err(e) => Err(e.message),
        },
        "callTool" => {
            if let Some(ref name) = req.tool_name {
                let args = req.args.clone().unwrap_or(serde_json::json!({}));
                match mcp_connection.call_tool(name, args).await {
                    Ok(res) => Ok(res),
                    Err(e) => Err(e.message),
                }
            } else {
                Err("toolName required".to_string())
            }
        }
        "getInstructions" => match mcp_connection.get_instructions().await {
            Ok(inst) => Ok(serde_json::json!(inst)),
            Err(e) => Err(e.message),
        },
        "close" => {
            let tx = shutdown_tx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let _ = tx.send(()).await;
            });
            Ok(serde_json::json!("closing"))
        }
        _ => Err(format!("Unknown request type: {}", req.req_type)),
    };

    match result {
        Ok(val) => DaemonResponse {
            id: req.id,
            success: true,
            data: Some(val),
            error: None,
        },
        Err(err) => DaemonResponse {
            id: req.id,
            success: false,
            data: None,
            error: Some(DaemonErrorDetail {
                code: "EXECUTION_ERROR".to_string(),
                message: err,
            }),
        },
    }
}

// ============================================================================
// Daemon Loop
// ============================================================================

pub async fn run_daemon(server_name: &str, config: ServerConfig) -> Result<(), CliError> {
    #[cfg(unix)]
    let socket_path = get_socket_path(server_name);
    let config_hash = get_config_hash(&config);
    let timeout_ms = get_daemon_timeout_ms();

    // 1. Ensure socket dir exists
    let socket_dir = get_socket_dir();
    if !socket_dir.exists() {
        let _ = fs::create_dir_all(&socket_dir);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&socket_dir, fs::Permissions::from_mode(0o700));
        }
    }

    // 2. Cleanup stale files
    remove_socket_file(server_name);

    // 3. Connect to MCP server
    debug(&format!(
        "[daemon:{}] Connecting to MCP server...",
        server_name
    ));

    // Direct connection without daemon caching
    let mcp_conn = match &config {
        ServerConfig::Stdio(sc) => {
            let client = crate::client::StdioClient::connect(server_name, sc).await?;
            crate::client::McpConnection::Stdio(client)
        }
        ServerConfig::Http(hc) => {
            let client = crate::client::HttpClient::connect(server_name, hc).await?;
            crate::client::McpConnection::Http(client)
        }
    };
    debug(&format!("[daemon:{}] Connected to MCP server", server_name));

    // 4. Start daemon listener
    #[cfg(unix)]
    let listener = UnixListener::bind(&socket_path).map_err(|e| crate::errors::CliError {
        code: ErrorCode::ClientError,
        error_type: "DAEMON_BIND_FAILED".to_string(),
        message: format!("Failed to bind Unix socket: {}", e),
        details: None,
        suggestion: Some("Check socket directory permissions".to_string()),
    })?;

    #[cfg(windows)]
    let listener = TcpListener::bind(get_daemon_addr(server_name, &config_hash))
        .await
        .map_err(|e| crate::errors::CliError {
            code: ErrorCode::ClientError,
            error_type: "DAEMON_BIND_FAILED".to_string(),
            message: format!("Failed to bind daemon TCP listener: {}", e),
            details: None,
            suggestion: Some("Check whether the daemon port is already in use".to_string()),
        })?;

    // 5. Write PID File
    write_pid_file(server_name, &config_hash);

    // 6. Signal Parent: DAEMON_READY
    println!("DAEMON_READY");

    // Channels for signals and connection processing
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
    let (activity_tx, mut activity_rx) = mpsc::channel::<()>(100);

    let mcp_conn_arc = Arc::new(mcp_conn);

    // background loop for socket connections
    let mcp_conn_clone = Arc::clone(&mcp_conn_arc);
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut stream, _)) => {
                    let mcp_conn_ref = Arc::clone(&mcp_conn_clone);
                    let sht_tx = shutdown_tx_clone.clone();
                    let act_tx = activity_tx.clone();

                    tokio::spawn(async move {
                        let _ = act_tx.send(()).await; // Signal activity to reset idle timer

                        let mut buffer = vec![0u8; 65536];
                        match stream.read(&mut buffer).await {
                            Ok(n) => {
                                if n > 0 {
                                    let response =
                                        handle_request(&buffer[..n], &mcp_conn_ref, &sht_tx).await;
                                    if let Ok(serialized) = serde_json::to_string(&response) {
                                        let _ = stream.write_all(serialized.as_bytes()).await;
                                        let _ = stream.write_all(b"\n").await;
                                        let _ = stream.flush().await;
                                    }
                                }
                            }
                            Err(e) => {
                                debug(&format!("Socket connection read error: {}", e));
                            }
                        }
                    });
                }
                Err(e) => {
                    debug(&format!("Socket accept error: {}", e));
                }
            }
        }
    });

    // Idle Timer Management
    let idle_duration = Duration::from_millis(timeout_ms);
    let mut idle_timer = tokio::time::interval(idle_duration);
    idle_timer.reset();

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                debug(&format!("[daemon:{}] Graceful shutdown requested", server_name));
                break;
            }
            _ = activity_rx.recv() => {
                // Reset idle timer
                idle_timer.reset();
            }
            _ = idle_timer.tick() => {
                // Since tick fires immediately on reset sometimes, we verify elapsed
                debug(&format!("[daemon:{}] Idle timeout reached, shutting down", server_name));
                break;
            }
            _ = tokio::signal::ctrl_c() => {
                debug(&format!("[daemon:{}] SIGINT received, shutting down", server_name));
                break;
            }
        }
    }

    // Cleanup
    remove_socket_file(server_name);
    remove_pid_file(server_name);
    if let Ok(conn) = Arc::try_unwrap(mcp_conn_arc) {
        let _ = conn.close().await;
    }
    debug(&format!("[daemon:{}] Cleanup complete", server_name));

    Ok(())
}
