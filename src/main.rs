use mcp_cli::commands::{
    call_command, grep_command, info_command, list_command, CallOptions, GrepOptions, InfoOptions,
    ListOptions,
};

struct ParsedArgs {
    command: String, // "list", "info", "grep", "call", "help", "version"
    server: Option<String>,
    tool: Option<String>,
    pattern: Option<String>,
    args: Option<String>,
    with_descriptions: bool,
    config_path: Option<String>,
}

fn is_possible_subcommand(arg: &str) -> bool {
    let aliases = [
        "run", "execute", "exec", "invoke", "list", "ls", "get", "show", "describe", "search",
        "find", "query",
    ];
    aliases.contains(&arg.to_lowercase().as_str())
}

fn parse_server_tool(args: &[String]) -> (String, Option<String>) {
    if args.is_empty() {
        return (String::new(), None);
    }

    let first = &args[0];

    if first.contains('/') {
        let slash_index = first.find('/').unwrap();
        let server = first[..slash_index].to_string();
        let tool = if first.len() > slash_index + 1 {
            Some(first[slash_index + 1..].to_string())
        } else {
            None
        };
        (server, tool)
    } else {
        let server = first.clone();
        let tool = if args.len() > 1 {
            Some(args[1].clone())
        } else {
            None
        };
        (server, tool)
    }
}

fn parse_args() -> ParsedArgs {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut with_descriptions = false;
    let mut config_path = None;
    let mut positional = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];

        match arg.as_str() {
            "-h" | "--help" => {
                return ParsedArgs {
                    command: "help".to_string(),
                    server: None,
                    tool: None,
                    pattern: None,
                    args: None,
                    with_descriptions: false,
                    config_path: None,
                };
            }
            "-v" | "--version" => {
                return ParsedArgs {
                    command: "version".to_string(),
                    server: None,
                    tool: None,
                    pattern: None,
                    args: None,
                    with_descriptions: false,
                    config_path: None,
                };
            }
            "-d" | "--with-descriptions" => {
                with_descriptions = true;
            }
            "-c" | "--config" => {
                i += 1;
                if i >= args.len() {
                    let err = mcp_cli::errors::missing_argument_error("-c/--config", "path");
                    eprintln!("{}", err);
                    std::process::exit(mcp_cli::errors::ErrorCode::ClientError.exit_code());
                }
                config_path = Some(args[i].clone());
            }
            _ => {
                if arg.starts_with('-') && arg != "-" {
                    let err = mcp_cli::errors::unknown_option_error(arg);
                    eprintln!("{}", err);
                    std::process::exit(mcp_cli::errors::ErrorCode::ClientError.exit_code());
                }
                positional.push(arg.clone());
            }
        }
        i += 1;
    }

    if positional.is_empty() {
        return ParsedArgs {
            command: "list".to_string(),
            server: None,
            tool: None,
            pattern: None,
            args: None,
            with_descriptions,
            config_path,
        };
    }

    let first_arg = &positional[0];

    if first_arg == "info" {
        let remaining = &positional[1..];
        let (server, tool) = parse_server_tool(remaining);

        if server.is_empty() {
            let mut available_servers = Vec::new();
            if let Ok(config) = mcp_cli::config::load_config(config_path.as_deref()) {
                available_servers = mcp_cli::config::list_server_names(&config);
            }

            let server_list = if !available_servers.is_empty() {
                available_servers.join(", ")
            } else {
                "(none found)".to_string()
            };

            eprintln!("Error [MISSING_ARGUMENT]: Missing required argument for info: server");
            eprintln!("  Available servers: {}", server_list);
            eprintln!("  Suggestion: Use 'mcp-cli info <server>' to see server details, or just 'mcp-cli' to list all");
            std::process::exit(mcp_cli::errors::ErrorCode::ClientError.exit_code());
        }

        return ParsedArgs {
            command: "info".to_string(),
            server: Some(server),
            tool,
            pattern: None,
            args: None,
            with_descriptions,
            config_path,
        };
    }

    if first_arg == "grep" {
        if positional.len() < 2 {
            let err = mcp_cli::errors::missing_argument_error("grep", "pattern");
            eprintln!("{}", err);
            std::process::exit(mcp_cli::errors::ErrorCode::ClientError.exit_code());
        }
        let pattern = positional[1].clone();
        if positional.len() > 2 {
            let err = mcp_cli::errors::too_many_arguments_error("grep", positional.len() - 1, 1);
            eprintln!("{}", err);
            std::process::exit(mcp_cli::errors::ErrorCode::ClientError.exit_code());
        }
        return ParsedArgs {
            command: "grep".to_string(),
            server: None,
            tool: None,
            pattern: Some(pattern),
            args: None,
            with_descriptions,
            config_path,
        };
    }

    if first_arg == "call" {
        let remaining = &positional[1..];

        if remaining.is_empty() {
            let err = mcp_cli::errors::missing_argument_error("call", "server and tool");
            eprintln!("{}", err);
            std::process::exit(mcp_cli::errors::ErrorCode::ClientError.exit_code());
        }

        let (server, tool) = parse_server_tool(remaining);

        if tool.is_none() {
            if remaining[0].contains('/') && remaining[0].split('/').nth(1).unwrap_or("").is_empty()
            {
                let err = mcp_cli::errors::missing_argument_error("call", "tool");
                eprintln!("{}", err);
                std::process::exit(mcp_cli::errors::ErrorCode::ClientError.exit_code());
            }
            if remaining.len() < 2 {
                let err = mcp_cli::errors::missing_argument_error("call", "tool");
                eprintln!("{}", err);
                std::process::exit(mcp_cli::errors::ErrorCode::ClientError.exit_code());
            }
        }

        let args_start_index = if remaining[0].contains('/') { 1 } else { 2 };

        let mut collected_args = None;
        let json_args = &remaining[args_start_index..];
        if !json_args.is_empty() {
            let args_value = json_args.join(" ");
            if args_value != "-" {
                collected_args = Some(args_value);
            }
        }

        return ParsedArgs {
            command: "call".to_string(),
            server: Some(server),
            tool,
            pattern: None,
            args: collected_args,
            with_descriptions,
            config_path,
        };
    }

    if is_possible_subcommand(first_arg) {
        let err = mcp_cli::errors::unknown_subcommand_error(first_arg);
        eprintln!("{}", err);
        std::process::exit(mcp_cli::errors::ErrorCode::ClientError.exit_code());
    }

    if first_arg.contains('/') {
        let parts: Vec<&str> = first_arg.split('/').collect();
        let server_name = parts[0];
        let tool_name = parts.get(1).unwrap_or(&"");
        let has_args = positional.len() > 1;
        let err = mcp_cli::errors::ambiguous_command_error(server_name, tool_name, has_args);
        eprintln!("{}", err);
        std::process::exit(mcp_cli::errors::ErrorCode::ClientError.exit_code());
    }

    if positional.len() >= 2 {
        let server_name = &positional[0];
        let possible_tool = &positional[1];

        let looks_like_json = possible_tool.starts_with('{') || possible_tool.starts_with('[');

        let re = regex::Regex::new(r"^[a-zA-Z_][a-zA-Z0-9_-]*$").unwrap();
        let looks_like_tool_name = re.is_match(possible_tool);

        if !looks_like_json && looks_like_tool_name {
            let has_args = positional.len() > 2;
            let err =
                mcp_cli::errors::ambiguous_command_error(server_name, possible_tool, has_args);
            eprintln!("{}", err);
            std::process::exit(mcp_cli::errors::ErrorCode::ClientError.exit_code());
        }
    }

    ParsedArgs {
        command: "info".to_string(),
        server: Some(first_arg.clone()),
        tool: None,
        pattern: None,
        args: None,
        with_descriptions,
        config_path,
    }
}

#[allow(clippy::print_literal)]
fn print_help() {
    let help_text = r#"mcp-cli vVERSION - A lightweight CLI for MCP servers

Usage:
  mcp-cli [options]                              List all servers and tools
  mcp-cli [options] info <server>                Show server details
  mcp-cli [options] info <server> <tool>         Show tool schema
  mcp-cli [options] grep <pattern>               Search tools by glob pattern
  mcp-cli [options] call <server> <tool>         Call tool (reads JSON from stdin if no args)
  mcp-cli [options] call <server> <tool> <json>  Call tool with JSON arguments

Formats (both work):
  mcp-cli info server tool                       Space-separated
  mcp-cli info server/tool                       Slash-separated
  mcp-cli call server tool '{}'                  Space-separated
  mcp-cli call server/tool '{}'                  Slash-separated

Options:
  -h, --help               Show this help message
  -v, --version            Show version number
  -d, --with-descriptions  Include tool descriptions
  -c, --config <path>      Path to config file (overrides all default configs, no merge)

Output:
  mcp-cli/info/grep        Human-readable text to stdout
  call                     Raw JSON to stdout (for piping)
  Errors                   Always to stderr

Examples:
  mcp-cli                                        # List all servers
  mcp-cli -d                                     # List with descriptions
  mcp-cli grep "*file*"                          # Search for file tools
  mcp-cli info filesystem                        # Show server tools
  mcp-cli info filesystem read_file              # Show tool schema
  mcp-cli call filesystem read_file '{}'         # Call tool
  cat input.json | mcp-cli call server tool      # Read from stdin (no '-' needed)

Environment Variables:
  MCP_NO_DAEMON=1        Disable connection caching (force fresh connections)
  MCP_DAEMON_TIMEOUT=N   Set daemon idle timeout in seconds (default: 60)

Config Files:
  By default, candidate configs are merged in order (first-match server wins):
    1. ./mcp_servers.json (current directory)
    2. ~/.mcp_servers.json ($HOME or %USERPROFILE%)
    3. ~/.config/mcp/mcp_servers.json
  Specifying -c/--config or MCP_CONFIG_PATH overrides all defaults (no merging)."#;
    println!(
        "{}",
        help_text.replace("VERSION", env!("CARGO_PKG_VERSION"))
    );
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 4 && args[1] == "--daemon" {
        let server_name = &args[2];
        let config_json = &args[3];
        let config: mcp_cli::config::ServerConfig = match serde_json::from_str(config_json) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to parse server config for daemon: {}", e);
                std::process::exit(mcp_cli::errors::ErrorCode::ClientError.exit_code());
            }
        };
        if let Err(e) = mcp_cli::daemon::run_daemon(server_name, config).await {
            eprintln!("{}", e);
            std::process::exit(e.code.exit_code());
        }
        std::process::exit(0);
    }

    let parsed = parse_args();

    let result = match parsed.command.as_str() {
        "help" => {
            print_help();
            Ok(())
        }
        "version" => {
            println!("mcp-cli v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "list" => {
            list_command(ListOptions {
                with_descriptions: parsed.with_descriptions,
                config_path: parsed.config_path,
            })
            .await
        }
        "info" => {
            let target = if let Some(ref server) = parsed.server {
                if let Some(ref tool) = parsed.tool {
                    format!("{}/{}", server, tool)
                } else {
                    server.clone()
                }
            } else {
                String::new()
            };

            info_command(InfoOptions {
                target,
                with_descriptions: parsed.with_descriptions,
                config_path: parsed.config_path,
            })
            .await
        }
        "grep" => {
            grep_command(GrepOptions {
                pattern: parsed.pattern.unwrap_or_default(),
                with_descriptions: parsed.with_descriptions,
                config_path: parsed.config_path,
            })
            .await
        }
        "call" => {
            let target = if let Some(ref server) = parsed.server {
                if let Some(ref tool) = parsed.tool {
                    format!("{}/{}", server, tool)
                } else {
                    server.clone()
                }
            } else {
                String::new()
            };

            call_command(CallOptions {
                target,
                args: parsed.args,
                config_path: parsed.config_path,
            })
            .await
        }
        _ => {
            eprintln!("Unknown command: {}", parsed.command);
            std::process::exit(mcp_cli::errors::ErrorCode::ClientError.exit_code());
        }
    };

    match result {
        Ok(_) => {
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(e.code.exit_code());
        }
    }
}
