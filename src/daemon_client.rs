use crate::config::{debug, get_config_hash, get_socket_dir, get_socket_path, ServerConfig};
#[cfg(windows)]
use crate::daemon::get_daemon_addr;
use crate::daemon::{
    is_process_running, kill_process, read_pid_file, remove_pid_file, remove_socket_file,
    DaemonRequest, DaemonResponse,
};
use crate::errors::CliError;
use std::fs;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
#[cfg(windows)]
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;

// ============================================================================
// Daemon Connection
// ============================================================================

pub struct DaemonConnection {
    pub server_name: String,
    pub socket_path: PathBuf,
    pub config: ServerConfig,
}

fn generate_request_id() -> String {
    let now = chrono::Utc::now().timestamp_millis();
    let rand_val: u32 = rand::random();
    format!("{}-{}", now, rand_val)
}

async fn send_request(
    server_name: &str,
    config: &ServerConfig,
    socket_path: &std::path::Path,
    request: DaemonRequest,
) -> Result<DaemonResponse, CliError> {
    #[cfg(unix)]
    {
        let _ = server_name;
        let _ = config;
    }
    #[cfg(windows)]
    {
        let _ = socket_path;
    }

    #[cfg(unix)]
    let connect_fut = UnixStream::connect(socket_path);
    #[cfg(windows)]
    let connect_fut = TcpStream::connect(get_daemon_addr(server_name, &get_config_hash(config)));

    let mut stream = tokio::select! {
        res = connect_fut => {
            res.map_err(|e| crate::errors::CliError {
                code: crate::errors::ErrorCode::NetworkError,
                error_type: "DAEMON_CONNECT_FAILED".to_string(),
                message: format!("Failed to connect to daemon socket: {}", e),
                details: None,
                suggestion: None,
            })?
        }
        _ = tokio::time::sleep(Duration::from_secs(5)) => {
            return Err(crate::errors::CliError {
                code: crate::errors::ErrorCode::NetworkError,
                error_type: "DAEMON_TIMEOUT".to_string(),
                message: "Daemon connection timeout".to_string(),
                details: None,
                suggestion: None,
            });
        }
    };

    let serialized = serde_json::to_vec(&request).map_err(|e| crate::errors::CliError {
        code: crate::errors::ErrorCode::ClientError,
        error_type: "DAEMON_SERIALIZE_FAILED".to_string(),
        message: format!("Failed to serialize request: {}", e),
        details: None,
        suggestion: None,
    })?;

    if let Err(e) = stream.write_all(&serialized).await {
        return Err(crate::errors::CliError {
            code: crate::errors::ErrorCode::NetworkError,
            error_type: "DAEMON_WRITE_FAILED".to_string(),
            message: format!("Failed to write to daemon: {}", e),
            details: None,
            suggestion: None,
        });
    }
    if let Err(e) = stream.flush().await {
        return Err(crate::errors::CliError {
            code: crate::errors::ErrorCode::NetworkError,
            error_type: "DAEMON_WRITE_FAILED".to_string(),
            message: format!("Failed to flush daemon connection: {}", e),
            details: None,
            suggestion: None,
        });
    }

    let mut buffer = Vec::new();
    let read_fut = stream.read_to_end(&mut buffer);

    tokio::select! {
        res = read_fut => {
            res.map_err(|e| crate::errors::CliError {
                code: crate::errors::ErrorCode::NetworkError,
                error_type: "DAEMON_READ_FAILED".to_string(),
                message: format!("Failed to read from daemon: {}", e),
                details: None,
                suggestion: None,
            })?;
        }
        _ = tokio::time::sleep(Duration::from_secs(5)) => {
            return Err(crate::errors::CliError {
                code: crate::errors::ErrorCode::NetworkError,
                error_type: "DAEMON_TIMEOUT".to_string(),
                message: "Daemon response timeout".to_string(),
                details: None,
                suggestion: None,
            });
        }
    }

    let response: DaemonResponse =
        serde_json::from_slice(&buffer).map_err(|e| crate::errors::CliError {
            code: crate::errors::ErrorCode::ServerError,
            error_type: "DAEMON_RESPONSE_INVALID".to_string(),
            message: format!("Daemon returned invalid JSON: {}", e),
            details: Some(String::from_utf8_lossy(&buffer).to_string()),
            suggestion: None,
        })?;

    Ok(response)
}

fn is_daemon_valid(server_name: &str, config: &ServerConfig) -> bool {
    #[cfg(unix)]
    let socket_path = get_socket_path(server_name);
    let pid_info = read_pid_file(server_name);

    if pid_info.is_none() {
        debug(&format!("[daemon-client] No PID file for {}", server_name));
        return false;
    }
    let pid_info = pid_info.unwrap();

    if !is_process_running(pid_info.pid) {
        debug(&format!(
            "[daemon-client] Process {} not running, cleaning up",
            pid_info.pid
        ));
        remove_pid_file(server_name);
        remove_socket_file(server_name);
        return false;
    }

    let current_hash = get_config_hash(config);
    if pid_info.config_hash != current_hash {
        debug(&format!(
            "[daemon-client] Config hash mismatch for {}, killing old daemon",
            server_name
        ));
        kill_process(pid_info.pid);
        remove_pid_file(server_name);
        remove_socket_file(server_name);
        return false;
    }

    #[cfg(unix)]
    if !socket_path.exists() {
        debug(&format!(
            "[daemon-client] Socket missing for {}, cleaning up",
            server_name
        ));
        kill_process(pid_info.pid);
        remove_pid_file(server_name);
        return false;
    }

    true
}

async fn spawn_daemon(server_name: &str, config: &ServerConfig) -> bool {
    debug(&format!(
        "[daemon-client] Spawning daemon for {}",
        server_name
    ));

    let current_exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(e) => {
            debug(&format!("[daemon-client] Failed to get current_exe: {}", e));
            return false;
        }
    };

    let config_json = match serde_json::to_string(config) {
        Ok(json) => json,
        Err(e) => {
            debug(&format!(
                "[daemon-client] Failed to serialize config: {}",
                e
            ));
            return false;
        }
    };

    // Spawn detached process
    let mut cmd = tokio::process::Command::new(current_exe);
    cmd.args(["--daemon", server_name, &config_json])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null()); // Don't forward stderr to console for daemon

    // 業務動機：在 Windows 平台上啟動 Daemon 時，設定 CREATE_NO_WINDOW (0x08000000) 旗標，
    // 避免每次啟動背景 Daemon 時彈出黑色的命令提示字元主控台視窗，並使其脫離前台主控台干擾。
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            debug(&format!(
                "[daemon-client] Failed to spawn child process: {}",
                e
            ));
            return false;
        }
    };

    let stdout = child.stdout.take().unwrap();
    let mut reader = tokio::io::BufReader::new(stdout);
    let mut line = String::new();

    let read_ready = async {
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => return false, // EOF
                Ok(_) => {
                    if line.contains("DAEMON_READY") {
                        return true;
                    }
                }
                Err(_) => return false,
            }
        }
    };

    let spawned = tokio::select! {
        ready = read_ready => ready,
        _ = tokio::time::sleep(Duration::from_secs(5)) => {
            debug(&format!("[daemon-client] Daemon spawn timeout for {}", server_name));
            false
        }
    };

    if spawned {
        debug(&format!(
            "[daemon-client] Daemon successfully spawned for {}",
            server_name
        ));
        true
    } else {
        let _ = child.kill().await;
        false
    }
}

pub async fn get_daemon_connection(
    server_name: &str,
    config: &ServerConfig,
) -> Option<DaemonConnection> {
    let socket_path = get_socket_path(server_name);

    if !is_daemon_valid(server_name, config) {
        let spawned = spawn_daemon(server_name, config).await;
        if !spawned {
            debug(&format!(
                "[daemon-client] Failed to spawn daemon for {}",
                server_name
            ));
            return None;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    if !socket_path.exists() {
        debug(&format!(
            "[daemon-client] Socket not found after spawn for {}",
            server_name
        ));
        return None;
    }

    // Ping test
    match send_request(
        server_name,
        config,
        &socket_path,
        DaemonRequest {
            id: generate_request_id(),
            req_type: "ping".to_string(),
            tool_name: None,
            args: None,
        },
    )
    .await
    {
        Ok(resp) => {
            if !resp.success {
                debug(&format!("[daemon-client] Ping failed for {}", server_name));
                return None;
            }
        }
        Err(e) => {
            debug(&format!(
                "[daemon-client] Connection test failed for {}: {}",
                server_name, e
            ));
            return None;
        }
    }

    debug(&format!(
        "[daemon-client] Connected to daemon for {}",
        server_name
    ));
    Some(DaemonConnection {
        server_name: server_name.to_string(),
        socket_path,
        config: config.clone(),
    })
}

pub async fn cleanup_orphaned_daemons() {
    let socket_dir = get_socket_dir();
    if !socket_dir.exists() {
        return;
    }

    if let Ok(entries) = fs::read_dir(socket_dir) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("pid") {
                if let Some(file_name) = path.file_stem().and_then(|s| s.to_str()) {
                    let server_name = file_name;
                    if let Some(pid_info) = read_pid_file(server_name) {
                        if !is_process_running(pid_info.pid) {
                            debug(&format!(
                                "[daemon-client] Cleaning up orphaned daemon: {}",
                                server_name
                            ));
                            remove_pid_file(server_name);
                            remove_socket_file(server_name);
                        }
                    }
                }
            }
        }
    }
}

impl DaemonConnection {
    pub async fn close(self) -> Result<(), CliError> {
        debug(&format!(
            "[daemon-client] Disconnecting from {} daemon",
            self.server_name
        ));
        Ok(())
    }

    pub async fn list_tools(&self) -> Result<serde_json::Value, CliError> {
        let response = send_request(
            &self.server_name,
            &self.config,
            &self.socket_path,
            DaemonRequest {
                id: generate_request_id(),
                req_type: "listTools".to_string(),
                tool_name: None,
                args: None,
            },
        )
        .await?;

        if !response.success {
            return Err(crate::errors::CliError {
                code: crate::errors::ErrorCode::ServerError,
                error_type: "DAEMON_ERROR".to_string(),
                message: response
                    .error
                    .map(|e| e.message)
                    .unwrap_or_else(|| "listTools failed".to_string()),
                details: None,
                suggestion: None,
            });
        }

        response.data.ok_or_else(|| crate::errors::CliError {
            code: crate::errors::ErrorCode::ServerError,
            error_type: "DAEMON_ERROR".to_string(),
            message: "Daemon listTools returned no data".to_string(),
            details: None,
            suggestion: None,
        })
    }

    pub async fn call_tool(
        &self,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, CliError> {
        let response = send_request(
            &self.server_name,
            &self.config,
            &self.socket_path,
            DaemonRequest {
                id: generate_request_id(),
                req_type: "callTool".to_string(),
                tool_name: Some(tool_name.to_string()),
                args: Some(args),
            },
        )
        .await?;

        if !response.success {
            return Err(crate::errors::CliError {
                code: crate::errors::ErrorCode::ServerError,
                error_type: "DAEMON_ERROR".to_string(),
                message: response
                    .error
                    .map(|e| e.message)
                    .unwrap_or_else(|| "callTool failed".to_string()),
                details: None,
                suggestion: None,
            });
        }

        response.data.ok_or_else(|| crate::errors::CliError {
            code: crate::errors::ErrorCode::ServerError,
            error_type: "DAEMON_ERROR".to_string(),
            message: "Daemon call_tool returned no data".to_string(),
            details: None,
            suggestion: None,
        })
    }

    pub async fn get_instructions(&self) -> Result<Option<String>, CliError> {
        let response = send_request(
            &self.server_name,
            &self.config,
            &self.socket_path,
            DaemonRequest {
                id: generate_request_id(),
                req_type: "getInstructions".to_string(),
                tool_name: None,
                args: None,
            },
        )
        .await?;

        if !response.success {
            return Err(crate::errors::CliError {
                code: crate::errors::ErrorCode::ServerError,
                error_type: "DAEMON_ERROR".to_string(),
                message: response
                    .error
                    .map(|e| e.message)
                    .unwrap_or_else(|| "getInstructions failed".to_string()),
                details: None,
                suggestion: None,
            });
        }

        if let Some(data) = response.data {
            if data.is_null() {
                Ok(None)
            } else if let Some(s) = data.as_str() {
                Ok(Some(s.to_string()))
            } else {
                Ok(Some(data.to_string()))
            }
        } else {
            Ok(None)
        }
    }
}
