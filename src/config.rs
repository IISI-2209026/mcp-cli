use crate::errors::{
    config_invalid_json_error, config_missing_field_error, config_not_found_error,
    config_search_error, ErrorCode,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

// ============================================================================
// Types
// ============================================================================

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum ServerConfig {
    Stdio(StdioServerConfig),
    Http(HttpServerConfig),
}

impl ServerConfig {
    pub fn allowed_tools(&self) -> &Option<Vec<String>> {
        match self {
            ServerConfig::Stdio(c) => &c.allowed_tools,
            ServerConfig::Http(c) => &c.allowed_tools,
        }
    }

    pub fn disabled_tools(&self) -> &Option<Vec<String>> {
        match self {
            ServerConfig::Stdio(c) => &c.disabled_tools,
            ServerConfig::Http(c) => &c.disabled_tools,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StdioServerConfig {
    pub command: String,
    pub args: Option<Vec<String>>,
    pub env: Option<HashMap<String, String>>,
    pub cwd: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
    pub disabled_tools: Option<Vec<String>>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HttpServerConfig {
    pub url: String,
    pub headers: Option<HashMap<String, String>>,
    pub timeout: Option<u64>,
    pub allowed_tools: Option<Vec<String>>,
    pub disabled_tools: Option<Vec<String>>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct McpServersConfig {
    pub mcp_servers: HashMap<String, ServerConfig>,
}

// ============================================================================
// Tool Filtering
// ============================================================================

/// Convert a glob pattern containing * and ? into a regex.
fn matches_pattern(name: &str, pattern: &str) -> bool {
    // Convert glob to regex
    let mut regex_pattern = String::new();
    for c in pattern.chars() {
        match c {
            '.' | '+' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\' => {
                regex_pattern.push('\\');
                regex_pattern.push(c);
            }
            '*' => {
                regex_pattern.push_str(".*");
            }
            '?' => {
                regex_pattern.push('.');
            }
            _ => {
                regex_pattern.push(c);
            }
        }
    }

    if let Ok(re) = regex::RegexBuilder::new(&format!("^{}$", regex_pattern))
        .case_insensitive(true)
        .build()
    {
        re.is_match(name)
    } else {
        false
    }
}

fn matches_any_pattern(name: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pat| matches_pattern(name, pat))
}

pub fn is_tool_allowed(tool_name: &str, config: &ServerConfig) -> bool {
    let disabled_tools = config.disabled_tools();
    if let Some(disabled) = disabled_tools {
        if !disabled.is_empty() && matches_any_pattern(tool_name, disabled) {
            return false;
        }
    }

    let allowed_tools = config.allowed_tools();
    if let Some(allowed) = allowed_tools {
        if !allowed.is_empty() {
            return matches_any_pattern(tool_name, allowed);
        }
    }

    true
}

pub fn filter_tools<T>(tools: &[T], config: &ServerConfig) -> Vec<T>
where
    T: AsRef<str> + Clone,
{
    tools
        .iter()
        .filter(|t| is_tool_allowed(t.as_ref(), config))
        .cloned()
        .collect()
}

// ============================================================================
// Environment & Constants
// ============================================================================

pub const DEFAULT_TIMEOUT_SECONDS: u64 = 1800; // 30 minutes
pub const DEFAULT_TIMEOUT_MS: u64 = DEFAULT_TIMEOUT_SECONDS * 1000;
pub const DEFAULT_CONCURRENCY: usize = 5;
pub const DEFAULT_MAX_RETRIES: u32 = 3;
pub const DEFAULT_RETRY_DELAY_MS: u64 = 1000;
pub const DEFAULT_DAEMON_TIMEOUT_SECONDS: u64 = 60;

pub fn debug(message: &str) {
    if env::var("MCP_DEBUG").is_ok() {
        eprintln!("[mcp-cli] {}", message);
    }
}

pub fn get_timeout_ms() -> u64 {
    if let Ok(val) = env::var("MCP_TIMEOUT") {
        if let Ok(seconds) = val.parse::<u64>() {
            if seconds > 0 {
                return seconds * 1000;
            }
        }
    }
    DEFAULT_TIMEOUT_MS
}

pub fn get_concurrency_limit() -> usize {
    if let Ok(val) = env::var("MCP_CONCURRENCY") {
        if let Ok(limit) = val.parse::<usize>() {
            if limit > 0 {
                return limit;
            }
        }
    }
    DEFAULT_CONCURRENCY
}

pub fn get_max_retries() -> u32 {
    if let Ok(val) = env::var("MCP_MAX_RETRIES") {
        if let Ok(retries) = val.parse::<u32>() {
            return retries;
        }
    }
    DEFAULT_MAX_RETRIES
}

pub fn get_retry_delay_ms() -> u64 {
    if let Ok(val) = env::var("MCP_RETRY_DELAY") {
        if let Ok(delay) = val.parse::<u64>() {
            if delay > 0 {
                return delay;
            }
        }
    }
    DEFAULT_RETRY_DELAY_MS
}

pub fn is_daemon_enabled() -> bool {
    env::var("MCP_NO_DAEMON").unwrap_or_default() != "1"
}

pub fn get_daemon_timeout_ms() -> u64 {
    if let Ok(val) = env::var("MCP_DAEMON_TIMEOUT") {
        if let Ok(seconds) = val.parse::<u64>() {
            if seconds > 0 {
                return seconds * 1000;
            }
        }
    }
    DEFAULT_DAEMON_TIMEOUT_SECONDS * 1000
}

pub fn get_socket_dir() -> PathBuf {
    #[cfg(unix)]
    let uid = unsafe { libc::getuid() };
    #[cfg(not(unix))]
    let uid = "unknown";

    PathBuf::from(format!("/tmp/mcp-cli-{}", uid))
}

pub fn get_socket_path(server_name: &str) -> PathBuf {
    get_socket_dir().join(format!("{}.sock", server_name))
}

pub fn get_pid_path(server_name: &str) -> PathBuf {
    get_socket_dir().join(format!("{}.pid", server_name))
}

// ============================================================================
// Hashing Config
// ============================================================================

pub fn get_config_hash(config: &ServerConfig) -> String {
    // Top-level key sorting for config serialization
    // This replicates TS's JSON.stringify(config, Object.keys(config).sort())
    let value = serde_json::to_value(config).unwrap_or(serde_json::Value::Null);

    let mut map = BTreeMap::new();
    if let serde_json::Value::Object(m) = value {
        for (k, v) in m {
            map.insert(k, v);
        }
    }

    let serialized = serde_json::to_string(&map).unwrap_or_default();

    let mut hasher = Sha256::new();
    hasher.update(serialized.as_bytes());
    let hash = hasher.finalize();
    hex::encode(hash)[..16].to_string()
}

// ============================================================================
// Env substitution
// ============================================================================

fn is_strict_env_mode() -> bool {
    let value = env::var("MCP_STRICT_ENV")
        .unwrap_or_default()
        .to_lowercase();
    value != "false" && value != "0"
}

fn substitute_env_vars(value: &str) -> Result<String, crate::errors::CliError> {
    let re = regex::Regex::new(r"\$\{([^}]+)\}").unwrap();
    let mut missing_vars = Vec::new();

    let result = re.replace_all(value, |caps: &regex::Captures| {
        let var_name = &caps[1];
        match env::var(var_name) {
            Ok(v) => v,
            Err(_) => {
                missing_vars.push(var_name.to_string());
                String::new()
            }
        }
    });

    if !missing_vars.is_empty() {
        let var_list = missing_vars
            .iter()
            .map(|v| format!("${{{}}}", v))
            .collect::<Vec<_>>()
            .join(", ");
        let message = format!(
            "Missing environment variable{}: {}",
            if missing_vars.len() > 1 { "s" } else { "" },
            var_list
        );

        if is_strict_env_mode() {
            return Err(crate::errors::CliError {
                code: ErrorCode::ClientError,
                error_type: "MISSING_ENV_VAR".to_string(),
                message,
                details: Some("Referenced in config but not set in environment".to_string()),
                suggestion: Some(format!(
                    "Set the variable(s) before running: export {}=\"value\" or set MCP_STRICT_ENV=false to use empty values",
                    missing_vars[0]
                )),
            });
        } else {
            eprintln!("[mcp-cli] Warning: {}", message);
        }
    }

    Ok(result.into_owned())
}

fn substitute_value(val: serde_json::Value) -> Result<serde_json::Value, crate::errors::CliError> {
    match val {
        serde_json::Value::String(s) => {
            let replaced = substitute_env_vars(&s)?;
            Ok(serde_json::Value::String(replaced))
        }
        serde_json::Value::Array(arr) => {
            let mut new_arr = Vec::new();
            for item in arr {
                new_arr.push(substitute_value(item)?);
            }
            Ok(serde_json::Value::Array(new_arr))
        }
        serde_json::Value::Object(obj) => {
            let mut new_obj = serde_json::Map::new();
            for (k, v) in obj {
                new_obj.insert(k, substitute_value(v)?);
            }
            Ok(serde_json::Value::Object(new_obj))
        }
        _ => Ok(val),
    }
}

// ============================================================================
// Config Loader
// ============================================================================

/// 業務動機：支援跨平台家目錄解析。
/// Unix-like 環境（Linux, macOS, WSL）使用 HOME 環境變數；
/// 原生 Windows 環境（PowerShell / CMD）預設沒有 HOME，而是使用 USERPROFILE。
pub fn get_home_dir() -> Option<PathBuf> {
    env::var("HOME")
        .ok()
        .or_else(|| env::var("USERPROFILE").ok())
        .map(PathBuf::from)
}

/// 業務動機：定義預設設定檔的搜尋候選路徑。
/// 依據優先層級（專案層級優先於全域層級），由高到低排列：
/// 1. 當前執行目錄下的 ./mcp_servers.json
/// 2. 使用者家目錄下的 ~/.mcp_servers.json
/// 3. 使用者配置目錄下的 ~/.config/mcp/mcp_servers.json
fn get_default_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // 1. Current directory (專案層級優先)
    paths.push(PathBuf::from("./mcp_servers.json"));

    // 2. Home directory variants (全域層級，支援 Unix 與 Windows)
    if let Some(home_path) = get_home_dir() {
        paths.push(home_path.join(".mcp_servers.json"));
        paths.push(home_path.join(".config/mcp/mcp_servers.json"));
    }

    paths
}

/// 業務動機：解析單一設定檔檔案內容。
/// 包含：讀檔、JSON 反序列化、檢查 mcpServers 頂層物件欄位、遞迴展開環境變數 (${VAR})。
/// 採 Fail-Fast 原則：若檔案格式損毀或缺少必要欄位，立即回傳錯誤終止，不靜默吞掉錯誤。
fn parse_single_config(path: &Path) -> Result<McpServersConfig, crate::errors::CliError> {
    let content = fs::read_to_string(path)
        .map_err(|e| config_invalid_json_error(&path.to_string_lossy(), &e.to_string()))?;

    // 解析為 JSON Value 以便執行 schema 驗證與環境變數替換
    let mut json_val: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| config_invalid_json_error(&path.to_string_lossy(), &e.to_string()))?;

    // 驗證頂層是否包含必要的 mcpServers 物件
    let mcp_servers_field = json_val.get("mcpServers");
    if mcp_servers_field.is_none() || !mcp_servers_field.unwrap().is_object() {
        return Err(config_missing_field_error(&path.to_string_lossy()));
    }

    // 遞迴替換設定值中的環境變數 (${VAR_NAME})
    json_val = substitute_value(json_val)?;

    // 反序列化為 McpServersConfig 結構
    let config: McpServersConfig = serde_json::from_value(json_val)
        .map_err(|e| config_invalid_json_error(&path.to_string_lossy(), &e.to_string()))?;

    Ok(config)
}

/// 業務動機：驗證設定檔中各 Server 配置的必填欄位是否合法。
/// 避免在後續連線階段才因 command 或 url 為空而拋出未定義的系統錯誤。
fn validate_servers(config: &McpServersConfig) -> Result<(), crate::errors::CliError> {
    for (name, server_config) in &config.mcp_servers {
        match server_config {
            ServerConfig::Stdio(sc) => {
                if sc.command.is_empty() {
                    return Err(crate::errors::CliError {
                        code: ErrorCode::ClientError,
                        error_type: "CONFIG_INVALID_SERVER".to_string(),
                        message: format!("Server \"{}\" has empty command", name),
                        details: Some("command must be a non-empty string".to_string()),
                        suggestion: Some("Provide a valid command, e.g. \"npx\"".to_string()),
                    });
                }
            }
            ServerConfig::Http(hc) => {
                if hc.url.is_empty() {
                    return Err(crate::errors::CliError {
                        code: ErrorCode::ClientError,
                        error_type: "CONFIG_INVALID_SERVER".to_string(),
                        message: format!("Server \"{}\" has empty URL", name),
                        details: Some("url must be a non-empty string".to_string()),
                        suggestion: Some(
                            "Provide a valid URL, e.g. \"https://example.com/mcp\"".to_string(),
                        ),
                    });
                }
            }
        }
    }
    Ok(())
}

/// 業務動機：依路徑清單合併設定檔的核心實作。
/// 支援由高到低優先順序走訪多個路徑：
/// - 若檔案不存在則略過。
/// - 若檔案存在但損毀則拋出錯誤（Fail-Fast）。
/// - 成功解析時，以 Server 名稱為 key 進行淺層合併（先讀取到的為主）。
pub fn load_config_from_paths(
    candidate_paths: &[PathBuf],
) -> Result<McpServersConfig, crate::errors::CliError> {
    let mut merged_servers = HashMap::new();
    let mut found_any_config = false;

    for path in candidate_paths {
        // 檔案不存在則順暢跳過，繼續尋找下一個候選路徑
        if !path.exists() {
            continue;
        }

        // 業務動機：採 Fail-Fast 原則，檔案若存在但格式損毀，必須立即報錯提醒使用者修復
        let config = parse_single_config(path)?;
        found_any_config = true;

        // 業務動機：以 Server 名稱為單位進行合併。
        // 「先讀到的為主」：已存在的 key 不會被後面的設定檔覆寫。
        for (name, server_config) in config.mcp_servers {
            merged_servers.entry(name).or_insert(server_config);
        }
    }

    // 若掃描完所有候選路徑皆未發現任何存在的設定檔，拋出未找到設定檔錯誤
    if !found_any_config {
        return Err(config_search_error());
    }

    let final_config = McpServersConfig {
        mcp_servers: merged_servers,
    };

    if final_config.mcp_servers.is_empty() {
        eprintln!("[mcp-cli] Warning: No servers configured in mcpServers. Add server configurations to use MCP tools.");
    }

    // 驗證合併後的各 Server 設定有效性
    validate_servers(&final_config)?;

    Ok(final_config)
}

/// 業務動機：載入 MCP 設定。
/// 支援兩種模式：
/// 1. Override 模式：若呼叫端傳入 explicit_path（如 CLI 的 -c/--config）或設定了 MCP_CONFIG_PATH，
///    則僅讀取該特定檔案做為唯一來源，不與任何其他預設設定檔合併（若檔案不存在則立即報錯）。
/// 2. Merge 模式：若無指定特定路徑，依序走訪所有預設候選路徑（專案目錄 -> 家目錄），
///    檔案不存在時直接略過；檔案存在時解析內容並以 Server 名稱為 key 進行淺層合併（先讀取到的為主）。
pub fn load_config(
    explicit_path: Option<&str>,
) -> Result<McpServersConfig, crate::errors::CliError> {
    let mut override_path: Option<PathBuf> = None;

    // 業務動機：判斷是否由使用者明確指定了單一設定檔來源
    if let Some(path) = explicit_path {
        override_path = Some(PathBuf::from(path));
    } else if let Ok(env_path) = env::var("MCP_CONFIG_PATH") {
        override_path = Some(PathBuf::from(env_path));
    }

    if let Some(path) = override_path {
        // [模式 1：Override 模式] 明確指定設定檔時，完全覆寫，不與其他檔案合併
        if !path.exists() {
            return Err(config_not_found_error(&path.to_string_lossy()));
        }

        let config = parse_single_config(&path)?;

        if config.mcp_servers.is_empty() {
            eprintln!("[mcp-cli] Warning: No servers configured in mcpServers. Add server configurations to use MCP tools.");
        }

        validate_servers(&config)?;
        return Ok(config);
    }

    // [模式 2：Merge 模式] 預設依優先度掃描所有候選路徑並進行合併
    load_config_from_paths(&get_default_config_paths())
}

pub fn get_server_config(
    config: &McpServersConfig,
    server_name: &str,
) -> Result<ServerConfig, crate::errors::CliError> {
    match config.mcp_servers.get(server_name) {
        Some(server) => Ok(server.clone()),
        None => {
            let available: Vec<String> = config.mcp_servers.keys().cloned().collect();
            Err(crate::errors::server_not_found_error(
                server_name,
                &available,
            ))
        }
    }
}

pub fn list_server_names(config: &McpServersConfig) -> Vec<String> {
    config.mcp_servers.keys().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_matches_pattern() {
        assert!(matches_pattern("read_file", "read_file"));
        assert!(matches_pattern("read_file", "read_*"));
        assert!(matches_pattern("read_file", "*_file"));
        assert!(matches_pattern("read_file", "*"));
        assert!(matches_pattern("read_file", "r?ad_file"));
        assert!(!matches_pattern("read_file", "write_*"));
        // Case insensitivity
        assert!(matches_pattern("Read_File", "read_*"));
    }

    #[test]
    fn test_is_tool_allowed() {
        // 1. No allowed or disabled specified -> allowed
        let config_default = ServerConfig::Stdio(StdioServerConfig {
            command: "node".to_string(),
            args: None,
            env: None,
            cwd: None,
            allowed_tools: None,
            disabled_tools: None,
        });
        assert!(is_tool_allowed("read_file", &config_default));

        // 2. Only allowed_tools specified
        let config_allowed = ServerConfig::Stdio(StdioServerConfig {
            command: "node".to_string(),
            args: None,
            env: None,
            cwd: None,
            allowed_tools: Some(vec!["read_*".to_string(), "list_*".to_string()]),
            disabled_tools: None,
        });
        assert!(is_tool_allowed("read_file", &config_allowed));
        assert!(is_tool_allowed("list_directory", &config_allowed));
        assert!(!is_tool_allowed("write_file", &config_allowed));

        // 3. Only disabled_tools specified
        let config_disabled = ServerConfig::Stdio(StdioServerConfig {
            command: "node".to_string(),
            args: None,
            env: None,
            cwd: None,
            allowed_tools: None,
            disabled_tools: Some(vec!["write_*".to_string()]),
        });
        assert!(is_tool_allowed("read_file", &config_disabled));
        assert!(!is_tool_allowed("write_file", &config_disabled));

        // 4. Both specified (disabled has higher priority)
        let config_both = ServerConfig::Stdio(StdioServerConfig {
            command: "node".to_string(),
            args: None,
            env: None,
            cwd: None,
            allowed_tools: Some(vec!["read_*".to_string(), "write_*".to_string()]),
            disabled_tools: Some(vec!["write_secure".to_string()]),
        });
        assert!(is_tool_allowed("read_file", &config_both));
        assert!(is_tool_allowed("write_file", &config_both));
        assert!(!is_tool_allowed("write_secure", &config_both));
    }

    #[test]
    fn test_substitute_env_vars() {
        env::set_var("TEST_VAR_XYZ", "hello_world");
        let input = "path/${TEST_VAR_XYZ}/file.txt";
        let output = substitute_env_vars(input).unwrap();
        assert_eq!(output, "path/hello_world/file.txt");

        // Non-strict missing env var returns empty string without error
        env::set_var("MCP_STRICT_ENV", "false");
        let missing = "path/${NONEXISTENT_VAR_ABC}/file.txt";
        let output_missing = substitute_env_vars(missing).unwrap();
        assert_eq!(output_missing, "path//file.txt");

        // Strict missing env var returns error
        env::set_var("MCP_STRICT_ENV", "true");
        let result_err = substitute_env_vars(missing);
        assert!(result_err.is_err());
    }

    #[test]
    fn test_get_config_hash() {
        let config_1 = ServerConfig::Stdio(StdioServerConfig {
            command: "node".to_string(),
            args: Some(vec!["arg1".to_string(), "arg2".to_string()]),
            env: None,
            cwd: None,
            allowed_tools: None,
            disabled_tools: None,
        });

        let config_2 = ServerConfig::Stdio(StdioServerConfig {
            command: "node".to_string(),
            args: Some(vec!["arg1".to_string(), "arg2".to_string()]),
            env: None,
            cwd: None,
            allowed_tools: None,
            disabled_tools: None,
        });

        // Config hash should be deterministic and identical for same content
        let hash_1 = get_config_hash(&config_1);
        let hash_2 = get_config_hash(&config_2);
        assert_eq!(hash_1, hash_2);
        assert_eq!(hash_1.len(), 16);
    }

    #[test]
    fn test_load_config_valid() {
        use std::fs::File;
        use std::io::Write;

        let mut rng = rand::thread_rng();
        let rand_val: u32 = rand::Rng::gen(&mut rng);
        let file_path = std::env::temp_dir().join(format!("mcp_servers_test_{}.json", rand_val));
        let mut file = File::create(&file_path).unwrap();

        let content = r#"{
            "mcpServers": {
                "test-server": {
                    "command": "echo",
                    "args": ["hello"]
                }
            }
        }"#;
        file.write_all(content.as_bytes()).unwrap();

        let config = load_config(Some(file_path.to_str().unwrap())).unwrap();
        assert_eq!(config.mcp_servers.len(), 1);
        let srv = config.mcp_servers.get("test-server").unwrap();
        match srv {
            ServerConfig::Stdio(sc) => {
                assert_eq!(sc.command, "echo");
                assert_eq!(sc.args.as_ref().unwrap()[0], "hello");
            }
            _ => panic!("Expected Stdio configuration"),
        }

        let _ = std::fs::remove_file(file_path);
    }

    #[test]
    fn test_load_config_invalid() {
        use std::fs::File;
        use std::io::Write;

        let mut rng = rand::thread_rng();
        let rand_val: u32 = rand::Rng::gen(&mut rng);
        let file_path = std::env::temp_dir().join(format!("mcp_servers_invalid_{}.json", rand_val));
        let mut file = File::create(&file_path).unwrap();

        let content = r#"{
            "mcpServers": {
                "test-server": {
                    "command": ""
                }
            }
        }"#;
        file.write_all(content.as_bytes()).unwrap();

        let result = load_config(Some(file_path.to_str().unwrap()));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().error_type, "CONFIG_INVALID_SERVER");

        let _ = std::fs::remove_file(file_path);
    }

    #[test]
    fn test_get_home_dir() {
        // 業務動機：驗證跨平台家目錄讀取邏輯。
        // 當 HOME 存在時優先使用 HOME；當 HOME 不存在但 USERPROFILE 存在時應正確回退到 USERPROFILE。
        let original_home = env::var("HOME").ok();
        let original_profile = env::var("USERPROFILE").ok();

        // 1. 測試 HOME 優先
        env::set_var("HOME", "/custom/home/path");
        env::set_var("USERPROFILE", "C:\\Users\\custom");
        assert_eq!(get_home_dir(), Some(PathBuf::from("/custom/home/path")));

        // 2. 測試 Windows USERPROFILE 回退機制
        env::remove_var("HOME");
        assert_eq!(get_home_dir(), Some(PathBuf::from("C:\\Users\\custom")));

        // 恢復環境變數避免干擾其他測試
        match original_home {
            Some(v) => env::set_var("HOME", v),
            None => env::remove_var("HOME"),
        }
        match original_profile {
            Some(v) => env::set_var("USERPROFILE", v),
            None => env::remove_var("USERPROFILE"),
        }
    }

    #[test]
    fn test_load_config_from_paths_merge_priority() {
        // 業務動機：驗證多設定檔合併時的覆蓋優先級。
        // 1. 同名 Server 採「先讀取到的為主」（檔案 1 覆蓋檔案 2）。
        // 2. 異名 Server 進行聯集合併。
        // 3. 不存在的候選路徑應直接被略過，不影響其他檔案載入。
        use std::fs::File;
        use std::io::Write;

        let rand_val: u32 = rand::random();
        let file_path_1 = std::env::temp_dir().join(format!("mcp_merge_1_{}.json", rand_val));
        let file_path_2 = std::env::temp_dir().join(format!("mcp_merge_2_{}.json", rand_val));
        let file_path_nonexistent =
            std::env::temp_dir().join(format!("mcp_merge_nonexistent_{}.json", rand_val));

        // 檔案 1：高優先（例如專案設定）
        let content_1 = r#"{
            "mcpServers": {
                "shared": {
                    "command": "cmd_v1",
                    "args": ["arg_v1"]
                },
                "local_only": {
                    "command": "cmd_local"
                }
            }
        }"#;
        let mut f1 = File::create(&file_path_1).unwrap();
        f1.write_all(content_1.as_bytes()).unwrap();

        // 檔案 2：低優先（例如全域設定）
        let content_2 = r#"{
            "mcpServers": {
                "shared": {
                    "command": "cmd_v2_ignored",
                    "args": ["arg_v2"]
                },
                "global_only": {
                    "command": "cmd_global"
                }
            }
        }"#;
        let mut f2 = File::create(&file_path_2).unwrap();
        f2.write_all(content_2.as_bytes()).unwrap();

        let candidate_paths = vec![
            file_path_1.clone(),
            file_path_nonexistent,
            file_path_2.clone(),
        ];
        let merged = load_config_from_paths(&candidate_paths).unwrap();

        // 總共應有 3 個 server
        assert_eq!(merged.mcp_servers.len(), 3);

        // 驗證同名 server 保留先讀取到的版本 (cmd_v1)
        match merged.mcp_servers.get("shared").unwrap() {
            ServerConfig::Stdio(sc) => {
                assert_eq!(sc.command, "cmd_v1");
                assert_eq!(sc.args.as_ref().unwrap()[0], "arg_v1");
            }
            _ => panic!("Expected Stdio server"),
        }

        // 驗證非同名 server 皆被成功併入
        assert!(merged.mcp_servers.contains_key("local_only"));
        assert!(merged.mcp_servers.contains_key("global_only"));

        let _ = std::fs::remove_file(file_path_1);
        let _ = std::fs::remove_file(file_path_2);
    }

    #[test]
    fn test_load_config_override_does_not_merge() {
        // 業務動機：驗證 Override 模式語意。
        // 當明確指定 -c/explicit_path 時，僅能讀取該特定檔案，絕對不與其他候選路徑合併。
        use std::fs::File;
        use std::io::Write;

        let rand_val: u32 = rand::random();
        let file_path =
            std::env::temp_dir().join(format!("mcp_servers_override_{}.json", rand_val));
        let mut file = File::create(&file_path).unwrap();

        let content = r#"{
            "mcpServers": {
                "override-only": {
                    "command": "node",
                    "args": ["override.js"]
                }
            }
        }"#;
        file.write_all(content.as_bytes()).unwrap();

        let config = load_config(Some(file_path.to_str().unwrap())).unwrap();
        // 僅應包含 override 指定的 server
        assert_eq!(config.mcp_servers.len(), 1);
        assert!(config.mcp_servers.contains_key("override-only"));

        let _ = std::fs::remove_file(file_path);
    }

    #[test]
    fn test_parse_single_config_fail_fast() {
        // 業務動機：驗證 Fail-Fast 原則。
        // 若設定檔存在但語法損毀（例如缺少引號或逗號），必須立刻拋錯終止，不能吞掉錯誤。
        use std::fs::File;
        use std::io::Write;

        let rand_val: u32 = rand::random();
        let file_path = std::env::temp_dir().join(format!("mcp_servers_corrupt_{}.json", rand_val));
        let mut file = File::create(&file_path).unwrap();

        // 故意寫入格式錯誤的 JSON
        file.write_all(b"{ invalid json syntax ").unwrap();

        let result = parse_single_config(&file_path);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().error_type, "CONFIG_INVALID_JSON");

        let _ = std::fs::remove_file(file_path);
    }
}
