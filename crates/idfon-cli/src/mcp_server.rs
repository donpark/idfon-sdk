//! `idfon mcp server`: idfon as an MCP server over stdio.
//!
//! Every CLI command is a tool, generated from the clap definitions, so the
//! MCP surface never falls behind the CLI: `peer add` is the tool `peer_add`,
//! its positionals and flags are the tool's arguments. A call runs this same
//! binary with those arguments and returns what it printed (`json: true` adds
//! `--json`). A denied call comes back as a tool error with the daemon's
//! message. The daemon trusts every local caller, so the commands that
//! reconfigure it are not tools (see [`EXCLUDED`]).
//!
//! Speaks MCP `2026-07-28` (`server/discover`, `_meta` on every request) and
//! the older `initialize` handshake, for clients that still use it.

use std::{
    io::{self, BufRead, Write},
    process::{Command as StdCommand, Stdio},
    sync::{Arc, Mutex},
};

use clap::{ArgAction, CommandFactory};
use serde_json::{json, Map, Value};

const MCP_PROTOCOL: &str = "2026-07-28";
/// `initialize` versions answered as asked; anything else gets the first.
const LEGACY_PROTOCOLS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];
const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";
const ERR_INVALID_PARAMS: i64 = -32602;
const ERR_UNSUPPORTED_VERSION: i64 = -32022;

/// Commands that are not tools: this server and the relay (`mcp …`, which
/// block), `shutdown` (other clients share the daemon), `skill` (it is the
/// server's instructions already), and the commands that reconfigure the
/// daemon or go around the CLI. Peer text reaches the model through these
/// tools, so a tool call may be a prompt injection: `rpc mcp.configure` or
/// `config set mcp.command` plus an `mcp.transport` grant would hand a peer a
/// shell, `config set ask.peer` would redirect approvals, `config set
/// edge.url` would send the stored tokens elsewhere, and `provider start`
/// serves a local folder to peers. The user runs these in a terminal.
const EXCLUDED: &[&[&str]] = &[
    &["mcp"],
    &["shutdown"],
    &["skill"],
    &["rpc"],
    &["config", "set"],
    &["config", "unset"],
    &["provider"],
    &["gateway"],
];
/// Capabilities a tool call may not grant or put in a ticket: `mcp.transport`
/// splices the holder to the configured local MCP command.
const REFUSED_CAPABILITIES: &[&str] = &["mcp.transport"];
/// Flags that would block forever under a tool call.
const EXCLUDED_ARGS: &[&str] = &["follow"];

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Flag,
    Value,
    List,
}

#[derive(Debug, Clone)]
struct Param {
    name: String,
    long: Option<String>,
    index: Option<usize>,
    kind: Kind,
    help: String,
    required: bool,
    choices: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Tool {
    pub name: String,
    path: Vec<String>,
    about: String,
    params: Vec<Param>,
}

/// One tool per leaf command of the CLI, minus [`EXCLUDED`].
pub fn tools() -> Vec<Tool> {
    // Built, so positionals have their indices (globals are propagated too,
    // and skipped below: the server passes its own).
    let mut command = crate::Cli::command();
    command.build();
    let mut out = Vec::new();
    collect(&command, &mut Vec::new(), &mut out);
    out
}

fn collect(command: &clap::Command, path: &mut Vec<String>, out: &mut Vec<Tool>) {
    for sub in command.get_subcommands() {
        if sub.is_hide_set() || sub.get_name() == "help" {
            continue;
        }
        path.push(sub.get_name().to_owned());
        let excluded = EXCLUDED.iter().any(|prefix| path.starts_with(&prefix.iter().map(|s| s.to_string()).collect::<Vec<_>>()));
        if !excluded {
            if sub.has_subcommands() {
                collect(sub, path, out);
            } else {
                out.push(tool(sub, path));
            }
        }
        path.pop();
    }
}

fn tool(command: &clap::Command, path: &[String]) -> Tool {
    let mut params: Vec<Param> = Vec::new();
    for arg in command.get_arguments() {
        let id = arg.get_id().as_str();
        if arg.is_hide_set() || arg.is_global_set() || id == "help" || id == "version" || EXCLUDED_ARGS.contains(&id) {
            continue;
        }
        let kind = match arg.get_action() {
            ArgAction::SetTrue | ArgAction::SetFalse => Kind::Flag,
            ArgAction::Append => Kind::List,
            _ => Kind::Value,
        };
        let long = arg.get_long().map(str::to_owned);
        let mut name = if arg.is_positional() {
            arg.get_value_names()
                .and_then(|names| names.first())
                .map(|name| name.as_str().to_ascii_lowercase())
                .unwrap_or_else(|| id.to_owned())
        } else {
            long.clone().unwrap_or_else(|| id.to_owned())
        }
        .replace('-', "_");
        if params.iter().any(|param| param.name == name) || name == "json" {
            name = id.to_owned();
        }
        params.push(Param {
            name,
            long,
            index: arg.get_index(),
            kind,
            help: arg.get_help().map(|help| help.to_string()).unwrap_or_default(),
            required: arg.is_required_set(),
            choices: arg
                .get_possible_values()
                .iter()
                .filter(|value| !value.is_hide_set())
                .map(|value| value.get_name().to_owned())
                .collect(),
        });
    }
    Tool {
        name: path.join("_").replace('-', "_"),
        path: path.to_vec(),
        about: command.get_about().map(|about| about.to_string()).unwrap_or_default(),
        params,
    }
}

impl Tool {
    fn schema(&self) -> Value {
        let mut properties = Map::new();
        let mut required = Vec::new();
        for param in &self.params {
            let mut property = match param.kind {
                Kind::Flag => json!({"type": "boolean"}),
                Kind::Value => json!({"type": "string"}),
                Kind::List => json!({"type": "array", "items": {"type": "string"}}),
            };
            if !param.help.is_empty() {
                property["description"] = json!(param.help);
            }
            if !param.choices.is_empty() {
                let target = if param.kind == Kind::List { &mut property["items"] } else { &mut property };
                target["enum"] = json!(param.choices);
            }
            properties.insert(param.name.clone(), property);
            if param.required {
                required.push(param.name.clone());
            }
        }
        properties.insert(
            "json".into(),
            json!({"type": "boolean", "description": "Machine output (the full response envelope) instead of short text"}),
        );
        json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
    }

    fn describe(&self) -> Value {
        json!({
            "name": self.name,
            "description": format!("`idfon {}`: {}", self.path.join(" "), self.about),
            "inputSchema": self.schema(),
        })
    }

    /// The CLI arguments for a call: path, flags, then `--` and positionals
    /// in order (so a value starting with `-` is never read as a flag).
    fn argv(&self, arguments: &Value) -> Result<Vec<String>, String> {
        let arguments = match arguments {
            Value::Null => Map::new(),
            Value::Object(map) => map.clone(),
            _ => return Err("arguments must be an object".into()),
        };
        for key in arguments.keys() {
            if key != "json" && !self.params.iter().any(|param| &param.name == key) {
                return Err(format!("unknown argument {key:?} for {}", self.name));
            }
        }
        if self.path.first().map(String::as_str) == Some("access") {
            let named = ["capabilities", "capability"]
                .iter()
                .filter_map(|key| arguments.get(*key))
                .flat_map(|value| match value {
                    Value::Array(items) => items.clone(),
                    other => vec![other.clone()],
                });
            for value in named {
                if let Some(capability) = value.as_str().filter(|text| REFUSED_CAPABILITIES.contains(&text.trim())) {
                    return Err(format!(
                        "{capability} cannot be granted from a tool call; the user grants it in a terminal \
                         (`idfon access grant <peer> {capability}`)"
                    ));
                }
            }
        }
        let mut argv = Vec::new();
        if arguments.get("json").and_then(Value::as_bool) == Some(true) {
            argv.push("--json".to_owned());
        }
        argv.extend(self.path.iter().cloned());
        let mut positionals: Vec<(usize, Vec<String>)> = Vec::new();
        for param in &self.params {
            let Some(value) = arguments.get(&param.name).filter(|value| !value.is_null()) else {
                if param.required {
                    return Err(format!("{} is required", param.name));
                }
                continue;
            };
            let values = match (&param.kind, value) {
                (Kind::Flag, Value::Bool(on)) => {
                    if *on {
                        argv.push(format!("--{}", param.long.as_deref().unwrap_or(&param.name)));
                    }
                    continue;
                }
                (Kind::Flag, _) => return Err(format!("{} must be true or false", param.name)),
                (Kind::List, Value::Array(items)) => items.iter().map(scalar).collect::<Result<Vec<_>, _>>()?,
                (_, value) => vec![scalar(value)?],
            };
            match (param.index, &param.long) {
                (Some(index), _) => positionals.push((index, values)),
                (None, Some(long)) => {
                    for value in values {
                        // An empty value is the bare flag (`--file` reads stdin).
                        argv.push(if value.is_empty() { format!("--{long}") } else { format!("--{long}={value}") });
                    }
                }
                (None, None) => return Err(format!("{} has no flag form", param.name)),
            }
        }
        positionals.sort_by_key(|(index, _)| *index);
        if !positionals.is_empty() {
            argv.push("--".into());
            argv.extend(positionals.into_iter().flat_map(|(_, values)| values));
        }
        Ok(argv)
    }
}

fn scalar(value: &Value) -> Result<String, String> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Number(number) => Ok(number.to_string()),
        Value::Bool(flag) => Ok(flag.to_string()),
        other => Err(format!("expected a string, got {other}")),
    }
}

/// The server's instructions: how tools map to commands, then the skill.
fn instructions() -> String {
    format!(
        "Each tool is an idfon CLI command: `idfon peer add carol.vcf` is the tool `peer_add` with \
         `{{\"input\": \"carol.vcf\"}}`; flags are arguments of the same name. The guide below uses \
         CLI syntax. `rpc`, `config set`/`unset`, `provider` and `gateway` are not tools, and no \
         tool grants `mcp.transport`: they reconfigure the daemon, so ask the user to run them in \
         a terminal.\n\n{}",
        crate::SKILL
    )
}

/// Runs the server until stdin closes. `globals` are the server's own
/// `--profile`/`--socket`/`--identity` flags, passed to every call.
pub fn serve(globals: Vec<String>) -> io::Result<()> {
    let tools = Arc::new(tools());
    let stdout = Arc::new(Mutex::new(io::stdout()));
    let globals = Arc::new(globals);
    let mut legacy = false;
    for line in io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => {
                write(&stdout, &json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": error.to_string()}}));
                continue;
            }
        };
        // Notifications (no id) get no response.
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let method = request.get("method").and_then(Value::as_str).unwrap_or_default();
        if method == "initialize" {
            legacy = true;
            let asked = request["params"]["protocolVersion"].as_str().unwrap_or_default();
            let version = LEGACY_PROTOCOLS.iter().find(|v| **v == asked).unwrap_or(&LEGACY_PROTOCOLS[0]);
            write(
                &stdout,
                &json!({"jsonrpc": "2.0", "id": id, "result": {
                    "protocolVersion": version,
                    "capabilities": {"tools": {}},
                    "serverInfo": server_info(),
                    "instructions": instructions(),
                }}),
            );
            continue;
        }
        if !legacy {
            if let Some(error) = validate_meta(&request, &id) {
                write(&stdout, &error);
                continue;
            }
        }
        match method {
            "server/discover" => write(
                &stdout,
                &json!({"jsonrpc": "2.0", "id": id, "result": {
                    "resultType": "complete",
                    "supportedVersions": [MCP_PROTOCOL],
                    "capabilities": {"tools": {}},
                    "instructions": instructions(),
                    "_meta": server_meta(),
                }}),
            ),
            "ping" => write(&stdout, &json!({"jsonrpc": "2.0", "id": id, "result": {}})),
            "tools/list" => write(
                &stdout,
                &json!({"jsonrpc": "2.0", "id": id, "result": {
                    "resultType": "complete",
                    "tools": tools.iter().map(Tool::describe).collect::<Vec<_>>(),
                    "_meta": server_meta(),
                }}),
            ),
            "tools/call" => {
                // Each call on its own thread: `card wait` may block for
                // minutes while other calls go on.
                let (tools, stdout, globals) = (Arc::clone(&tools), Arc::clone(&stdout), Arc::clone(&globals));
                std::thread::spawn(move || {
                    let response = call(&tools, &globals, &id, &request["params"]);
                    write(&stdout, &response);
                });
            }
            other => write(
                &stdout,
                &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("Method not found: {other}")}}),
            ),
        }
    }
    Ok(())
}

fn call(tools: &[Tool], globals: &[String], id: &Value, params: &Value) -> Value {
    let name = params["name"].as_str().unwrap_or_default();
    let Some(tool) = tools.iter().find(|tool| tool.name == name) else {
        return json!({"jsonrpc": "2.0", "id": id, "error": {"code": ERR_INVALID_PARAMS, "message": format!("Unknown tool: {name}")}});
    };
    let (text, is_error) = match tool.argv(&params["arguments"]) {
        Err(message) => (message, true),
        Ok(argv) => run(globals, &argv),
    };
    json!({"jsonrpc": "2.0", "id": id, "result": {
        "resultType": "complete",
        "content": [{"type": "text", "text": text}],
        "isError": is_error,
        "_meta": server_meta(),
    }})
}

/// Runs `idfon GLOBALS ARGV` with no stdin; returns stdout plus stderr (hints
/// and errors go there) and whether it failed.
fn run(globals: &[String], argv: &[String]) -> (String, bool) {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => return (format!("cannot find the idfon binary: {error}"), true),
    };
    let output = match StdCommand::new(exe).args(globals).args(argv).stdin(Stdio::null()).output() {
        Ok(output) => output,
        Err(error) => return (format!("cannot run idfon: {error}"), true),
    };
    let mut text = match String::from_utf8(output.stdout) {
        Ok(text) => text,
        Err(error) => format!(
            "({} bytes of binary output; pass `out` to save it to a file)\n",
            error.as_bytes().len()
        ),
    };
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        text.push_str(&stderr);
    }
    let failed = !output.status.success();
    if text.trim().is_empty() {
        text = if failed { format!("failed ({})", output.status) } else { "done".into() };
    }
    (text.trim_end().to_owned(), failed)
}

fn write(stdout: &Mutex<io::Stdout>, value: &Value) {
    let mut stdout = stdout.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let _ = writeln!(stdout, "{value}");
    let _ = stdout.flush();
}

fn server_info() -> Value {
    json!({"name": "idfon", "version": env!("CARGO_PKG_VERSION")})
}

fn server_meta() -> Value {
    json!({META_SERVER_INFO: server_info()})
}

/// MCP 2026-07-28: every request carries `_meta` with the protocol version
/// and client capabilities. Missing fields are `-32602`; another version is
/// `-32022` with the versions this server supports.
fn validate_meta(request: &Value, id: &Value) -> Option<Value> {
    let error = |code: i64, message: &str, data: Option<Value>| {
        let mut error = json!({"code": code, "message": message});
        if let Some(data) = data {
            error["data"] = data;
        }
        json!({"jsonrpc": "2.0", "id": id, "error": error})
    };
    let Some(meta) = request.get("params").and_then(|params| params.get("_meta")) else {
        return Some(error(ERR_INVALID_PARAMS, "missing required _meta", None));
    };
    if meta.get(META_VERSION).is_none() || meta.get(META_CLIENT_CAPABILITIES).is_none() {
        return Some(error(
            ERR_INVALID_PARAMS,
            "missing required _meta fields (protocolVersion, clientCapabilities)",
            None,
        ));
    }
    let requested = meta.get(META_VERSION).and_then(Value::as_str).unwrap_or_default();
    if requested != MCP_PROTOCOL {
        return Some(error(
            ERR_UNSUPPORTED_VERSION,
            "Unsupported protocol version",
            Some(json!({"supported": [MCP_PROTOCOL], "requested": requested})),
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(name: &str) -> Tool {
        tools().into_iter().find(|tool| tool.name == name).unwrap_or_else(|| panic!("no tool {name}"))
    }

    #[test]
    fn every_command_is_a_tool_except_the_excluded() {
        let names: Vec<String> = tools().into_iter().map(|tool| tool.name).collect();
        for name in ["send", "inbox", "peer_add", "card_send", "card_wait", "request_accept", "ask", "room_call_join", "access_grant", "config_show", "config_path"] {
            assert!(names.contains(&name.to_owned()), "{name} missing from {names:?}");
        }
        for name in [
            "mcp_server", "mcp_listen", "mcp_configure", "shutdown", "skill", "rpc", "config_set", "config_unset",
            "provider_start", "provider_stop", "gateway_start", "gateway_stop",
        ] {
            assert!(!names.contains(&name.to_owned()), "{name} should not be a tool");
        }
    }

    #[test]
    fn tools_cannot_grant_mcp_transport() {
        let grant = find("access_grant");
        assert!(grant.argv(&json!({"peer": "bob", "capabilities": ["message.send", "mcp.transport"]})).is_err());
        assert!(grant.argv(&json!({"peer": "bob", "capabilities": [" mcp.transport "]})).is_err());
        assert!(grant.argv(&json!({"peer": "bob", "capabilities": ["message.send"]})).is_ok());
        let ticket = find("access_ticket");
        assert!(ticket.argv(&json!({"peer": "bob", "capability": "mcp.transport"})).is_err());
        assert!(ticket.argv(&json!({"peer": "bob", "capability": "message.receive"})).is_ok());
    }

    #[test]
    fn schemas_follow_the_flags() {
        let inbox = find("inbox").schema();
        assert!(inbox["properties"].get("follow").is_none(), "follow blocks forever");
        assert_eq!(inbox["properties"]["new"]["type"], "boolean");
        let send = find("send").schema();
        assert_eq!(send["properties"]["peer"]["type"], "string");
        assert_eq!(send["properties"]["delivery"]["enum"], json!(["failover", "one", "all"]));
        assert!(send["properties"].get("stream").is_none(), "hidden flags stay hidden");
        let card = find("card_send").schema();
        assert_eq!(card["properties"]["action"]["type"], "array");
        assert_eq!(card["required"], json!(["peer", "title"]));
    }

    #[test]
    fn argv_puts_positionals_after_the_flags() {
        let argv = find("send").argv(&json!({"peer": "bob", "message": "-5 degrees", "wait": true, "timeout": "30s"})).unwrap();
        assert_eq!(argv, ["send", "--wait", "--timeout=30s", "--", "bob", "-5 degrees"]);
        let argv = find("card_send")
            .argv(&json!({"peer": "bob", "title": "Lunch?", "action": ["yes=Yes", "no=No"], "json": true}))
            .unwrap();
        assert_eq!(argv, ["--json", "card", "send", "--action=yes=Yes", "--action=no=No", "--", "bob", "Lunch?"]);
        let argv = find("access_grant").argv(&json!({"peer": "bob", "capabilities": ["message.send"]})).unwrap();
        assert_eq!(argv, ["access", "grant", "--", "bob", "message.send"]);
        assert!(find("send").argv(&json!({"peer": "bob", "bogus": 1})).is_err());
        assert!(find("card_send").argv(&json!({"peer": "bob"})).is_err(), "title is required");
    }
}
