use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{Emitter, Manager};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EnvStatus {
    pub node_installed: bool,
    pub node_version: String,
    pub npm_installed: bool,
    pub npm_version: String,
    pub openclaw_installed: bool,
    pub openclaw_version: String,
    pub gateway_running: bool,
    pub platform: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LogLine {
    pub text: String,
    pub level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CommandResult {
    pub success: bool,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProviderModel {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProviderConfig {
    pub id: String,
    pub base_url: String,
    pub api_key: String,
    pub api: String,
    pub models: Vec<ProviderModel>,
    pub active: bool,
    #[serde(default)]
    pub use_full_path: bool,
}

fn hide_window(command: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

fn hide_tokio_window(command: &mut tokio::process::Command) -> &mut tokio::process::Command {
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

fn system_path() -> String {
    let current = std::env::var("PATH").unwrap_or_default();

    if !cfg!(windows) {
        return current;
    }

    let mut command = std::process::Command::new("cmd");
    command
        .args([
            "/C",
            r#"reg query "HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment" /v PATH 2>nul"#,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    if let Ok(output) = hide_window(&mut command).output() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if line.contains("PATH") {
                let parts: Vec<&str> = line.splitn(4, ' ').collect();
                if parts.len() >= 4 {
                    return format!("{};{}", current, parts[3].trim());
                }
            }
        }
    }

    let appdata_npm = std::env::var("APPDATA")
        .map(|appdata| format!("{}\\npm", appdata))
        .unwrap_or_default();
    format!("{current};C:\\Program Files\\nodejs;{appdata_npm}")
}

fn resolve_cmd(base: &str) -> String {
    if cfg!(windows) {
        match base {
            "npm" => "npm.cmd".to_string(),
            "openclaw" => "openclaw.cmd".to_string(),
            _ => base.to_string(),
        }
    } else {
        base.to_string()
    }
}

fn run_cmd(cmd: &str, args: &[&str]) -> (bool, String) {
    let mut command = std::process::Command::new(resolve_cmd(cmd));
    command
        .args(args)
        .env("PATH", system_path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    match hide_window(&mut command).output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if output.status.success() {
                (true, stdout)
            } else if !stdout.is_empty() {
                (true, stdout)
            } else if !stderr.is_empty() && stderr.contains('.') {
                (true, stderr)
            } else {
                (false, String::new())
            }
        }
        Err(_) => (false, String::new()),
    }
}

fn check_gateway() -> bool {
    let mut command = std::process::Command::new(resolve_cmd("openclaw"));
    command
        .args(["gateway", "status"])
        .env("PATH", system_path())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    match hide_window(&mut command).output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout.contains("Running: running")
                || stdout.contains("running (pid")
                || stdout.contains("Service: running")
        }
        Err(_) => false,
    }
}

fn check_env_blocking() -> EnvStatus {
    let (node_installed, node_version) = run_cmd("node", &["--version"]);
    let (npm_installed, npm_version) = run_cmd("npm", &["--version"]);
    let (openclaw_installed, openclaw_version) = run_cmd("openclaw", &["--version"]);

    EnvStatus {
        node_installed,
        node_version,
        npm_installed,
        npm_version,
        openclaw_installed,
        openclaw_version,
        gateway_running: check_gateway(),
        platform: std::env::consts::OS.to_string(),
    }
}

#[tauri::command]
async fn check_env() -> EnvStatus {
    tokio::task::spawn_blocking(check_env_blocking)
        .await
        .unwrap_or_else(|_| EnvStatus {
            node_installed: false,
            node_version: String::new(),
            npm_installed: false,
            npm_version: String::new(),
            openclaw_installed: false,
            openclaw_version: String::new(),
            gateway_running: false,
            platform: std::env::consts::OS.to_string(),
        })
}

#[tauri::command]
async fn install_node(app: tauri::AppHandle) -> Result<(), String> {
    emit_log(&app, "开始安装 Node.js（使用淘宝镜像）...", "info");

    let mut command = if cfg!(windows) {
        let mut command = tokio::process::Command::new("powershell");
        command.args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            r#"$url='https://npmmirror.com/mirrors/node/v22.21.1/node-v22.21.1-x64.msi'; $tmp="$env:TEMP\node-installer.msi"; Invoke-WebRequest -Uri $url -OutFile $tmp -UseBasicParsing; Start-Process msiexec.exe -ArgumentList "/i `"$tmp`" /quiet /norestart" -Wait -WindowStyle Hidden; Remove-Item $tmp -Force"#,
        ]);
        command
    } else if cfg!(target_os = "macos") && run_cmd("brew", &["--version"]).0 {
        let mut command = tokio::process::Command::new("brew");
        command.args(["install", "node"]);
        command
    } else {
        let mut command = tokio::process::Command::new("bash");
        command.args(["-c", "curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash && export NVM_DIR=\"$HOME/.nvm\" && export NVM_NODEJS_ORG_MIRROR=\"https://npmmirror.com/mirrors/node\" && [ -s \"$NVM_DIR/nvm.sh\" ] && . \"$NVM_DIR/nvm.sh\" && nvm install 22 && nvm use 22"]);
        command
    };

    command
        .env("PATH", system_path())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let status = hide_tokio_window(&mut command)
        .status()
        .await
        .map_err(|e| format!("Node.js 安装命令执行失败: {e}"))?;

    if status.success() {
        emit_log(&app, "Node.js 安装完成", "success");
        Ok(())
    } else {
        emit_log(&app, "Node.js 安装失败，请手动安装后重试", "error");
        Err("Node.js 安装失败".to_string())
    }
}

#[tauri::command]
async fn install_openclaw(app: tauri::AppHandle) -> Result<(), String> {
    emit_log(&app, "正在安装 OpenClaw...", "info");

    let mut command = tokio::process::Command::new(resolve_cmd("npm"));
    command
        .args(["install", "-g", "openclaw"])
        .env("PATH", system_path())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let status = hide_tokio_window(&mut command)
        .status()
        .await
        .map_err(|e| format!("npm install 执行失败: {e}"))?;

    if status.success() {
        emit_log(&app, "OpenClaw 安装完成", "success");
        Ok(())
    } else {
        emit_log(&app, "安装失败，请手动执行: npm install -g openclaw", "error");
        Err("OpenClaw 安装失败".to_string())
    }
}

#[tauri::command]
async fn launch_openclaw(app: tauri::AppHandle) -> Result<(), String> {
    emit_log(&app, "正在重启 OpenClaw Gateway...", "info");

    let mut command = tokio::process::Command::new(resolve_cmd("openclaw"));
    command
        .args(["gateway", "restart", "--force"])
        .env("PATH", system_path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = hide_tokio_window(&mut command)
        .output()
        .await
        .map_err(|e| format!("Gateway 重启失败: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success()
        || stdout.contains("already running")
        || stderr.contains("already running")
    {
        emit_log(&app, "OpenClaw Gateway 已重启", "success");
        open_control_ui()?;
        Ok(())
    } else {
        let details = format!("{} {}", stdout.trim(), stderr.trim());
        emit_log(&app, &format!("Gateway 重启异常: {details}"), "error");
        Err(details)
    }
}

#[tauri::command]
async fn uninstall_openclaw(app: tauri::AppHandle) -> Result<(), String> {
    emit_log(&app, "正在卸载 OpenClaw...", "info");

    let mut command = tokio::process::Command::new(resolve_cmd("npm"));
    command
        .args(["uninstall", "-g", "openclaw"])
        .env("PATH", system_path())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let status = hide_tokio_window(&mut command)
        .status()
        .await
        .map_err(|e| format!("卸载命令执行失败: {e}"))?;

    if status.success() {
        emit_log(&app, "OpenClaw 已卸载", "success");
        Ok(())
    } else {
        emit_log(&app, "卸载失败，请手动执行: npm uninstall -g openclaw", "error");
        Err("OpenClaw 卸载失败".to_string())
    }
}

fn run_openclaw_command_blocking(args: Vec<String>) -> Result<CommandResult, String> {
    let mut command = std::process::Command::new(resolve_cmd("openclaw"));
    command
        .args(args)
        .env("PATH", system_path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = hide_window(&mut command)
        .output()
        .map_err(|e| format!("OpenClaw 执行失败: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    Ok(CommandResult {
        success: output.status.success(),
        output: clean_openclaw_output(&format!("{stdout}{stderr}")),
    })
}

#[tauri::command]
async fn run_openclaw_command(args: Vec<String>) -> Result<CommandResult, String> {
    tokio::task::spawn_blocking(move || run_openclaw_command_blocking(args))
        .await
        .map_err(|e| format!("OpenClaw 后台任务失败: {e}"))?
}

#[tauri::command]
fn list_providers() -> Result<Vec<ProviderConfig>, String> {
    let config = read_openclaw_config()?;
    Ok(read_providers_from_config(&config))
}

#[tauri::command]
async fn fetch_provider_models(provider: ProviderConfig) -> Result<Vec<ProviderModel>, String> {
    let base_url = normalize_external_base_url(&provider.base_url);
    if base_url.is_empty() {
        return Err("Base URL 不能为空".to_string());
    }
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("创建上游请求失败: {e}"))?;
    let mut request = client.get(&url);
    if !provider.api_key.trim().is_empty() {
        if provider.api.trim() == "anthropic-messages" {
            request = request
                .header("x-api-key", provider.api_key.trim())
                .header("anthropic-version", "2023-06-01");
        } else {
            request = request.bearer_auth(provider.api_key.trim());
        }
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("请求上游模型列表失败: {e}"))?;
    let status = response.status();
    let value = response
        .json::<Value>()
        .await
        .map_err(|e| format!("解析上游模型列表失败: {e}"))?;
    if !status.is_success() {
        let message = value
            .pointer("/error/message")
            .or_else(|| value.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("上游返回错误");
        return Err(format!("上游模型列表请求失败 ({status}): {message}"));
    }
    let models = value
        .get("data")
        .or_else(|| value.get("models"))
        .and_then(Value::as_array)
        .ok_or_else(|| "上游响应中未找到模型列表".to_string())?
        .iter()
        .filter_map(model_from_value)
        .collect::<Vec<ProviderModel>>();
    let mut unique = BTreeMap::new();
    for model in models {
        unique.insert(model.id.clone(), model);
    }
    if unique.is_empty() {
        return Err("上游未返回可用模型".to_string());
    }
    Ok(unique.into_values().collect())
}

#[tauri::command]
fn save_provider(provider: ProviderConfig) -> Result<Vec<ProviderConfig>, String> {
    let id = normalize_provider_id(&provider.id)?;
    if provider.base_url.trim().is_empty() {
        return Err("供应商 Base URL 不能为空".to_string());
    }
    if provider.models.is_empty() {
        return Err("至少需要配置一个模型".to_string());
    }

    let mut config = read_openclaw_config()?;
    ensure_object_path(&mut config, &["models", "providers"])?;
    let provider_value = json!({
        "baseUrl": if provider.use_full_path {
            provider.base_url.trim().trim_end_matches('/').to_string()
        } else {
            normalize_external_base_url(&provider.base_url)
        },
        "apiKey": provider.api_key.trim(),
        "api": if provider.api.trim().is_empty() { "openai-completions" } else { provider.api.trim() },
        "models": provider.models.iter().map(|model| json!({
            "id": model.id.trim(),
            "name": if model.name.trim().is_empty() { model.id.trim() } else { model.name.trim() }
        })).collect::<Vec<Value>>()
    });

    config["models"]["mode"] = Value::String("merge".to_string());
    config["models"]["providers"][&id] = provider_value;
    sync_agent_model_aliases(&mut config, &id, &provider.models)?;
    if provider.active {
        set_active_provider_model(&mut config, &id, &provider.models)?;
    }
    write_openclaw_config(&config)?;
    Ok(read_providers_from_config(&config))
}

#[tauri::command]
fn activate_provider(id: String) -> Result<Vec<ProviderConfig>, String> {
    let id = normalize_provider_id(&id)?;
    let mut config = read_openclaw_config()?;
    let models = config
        .pointer(&format!("/models/providers/{id}/models"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(model_from_value)
                .collect::<Vec<ProviderModel>>()
        })
        .unwrap_or_default();
    set_active_provider_model(&mut config, &id, &models)?;
    write_openclaw_config(&config)?;
    Ok(read_providers_from_config(&config))
}

#[tauri::command]
fn delete_provider(id: String) -> Result<Vec<ProviderConfig>, String> {
    let id = normalize_provider_id(&id)?;
    let mut config = read_openclaw_config()?;
    if let Some(providers) = config
        .pointer_mut("/models/providers")
        .and_then(Value::as_object_mut)
    {
        providers.remove(&id);
    }
    if let Some(models) = config
        .pointer_mut("/agents/defaults/models")
        .and_then(Value::as_object_mut)
    {
        models.retain(|model_id, _| !model_id.starts_with(&format!("{id}/")));
    }
    let primary = config
        .pointer("/agents/defaults/model/primary")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if primary.starts_with(&format!("{id}/")) {
        if let Some(defaults) = config
            .pointer_mut("/agents/defaults")
            .and_then(Value::as_object_mut)
        {
            defaults.remove("model");
        }
    }
    write_openclaw_config(&config)?;
    Ok(read_providers_from_config(&config))
}

#[tauri::command]
fn import_ccswitch_providers() -> Result<Vec<ProviderConfig>, String> {
    let mut config = read_openclaw_config()?;
    let imported = discover_external_providers();
    if imported.is_empty() {
        return Err("未发现 ccswitch/Claude 供应商配置，可手动添加或粘贴 JSON 后保存".to_string());
    }
    ensure_object_path(&mut config, &["models", "providers"])?;
    for provider in imported {
        let id = normalize_provider_id(&provider.id)?;
        config["models"]["providers"][&id] = json!({
            "baseUrl": provider.base_url,
            "apiKey": provider.api_key,
            "api": provider.api,
            "models": provider.models.iter().map(|model| json!({
                "id": model.id,
                "name": model.name
            })).collect::<Vec<Value>>()
        });
        sync_agent_model_aliases(&mut config, &id, &provider.models)?;
    }
    config["models"]["mode"] = Value::String("merge".to_string());
    write_openclaw_config(&config)?;
    Ok(read_providers_from_config(&config))
}

#[tauri::command]
fn open_dashboard() -> Result<(), String> {
    open_control_ui()
}

/* Removed chat WebSocket implementation.
#[tauri::command]
async fn gateway_agent_message(message: String) -> Result<GatewayAgentResult, String> {
    let trimmed = message.trim().to_string();
    if trimmed.is_empty() {
        return Err("消息不能为空".to_string());
    }

    tokio::task::spawn_blocking(move || gateway_agent_message_blocking(&trimmed))
        .await
        .map_err(|e| format!("Gateway WS 任务失败: {e}"))?
}
*/

#[tauri::command]
fn close_window(app: tauri::AppHandle, window: tauri::Window) {
    window.close().ok();
    app.exit(0);
}

fn open_control_ui() -> Result<(), String> {
    let mut command = std::process::Command::new(resolve_cmd("openclaw"));
    command
        .args(["dashboard", "--yes"])
        .env("PATH", system_path())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    hide_window(&mut command)
        .status()
        .map_err(|e| format!("打开 OpenClaw 界面失败: {e}"))?;
    Ok(())
}

/* Removed chat WebSocket implementation.
fn gateway_agent_message_blocking(message: &str) -> Result<GatewayAgentResult, String> {
    let gateway_token = read_openclaw_config()?
        .pointer("/gateway/auth/token")
        .and_then(Value::as_str)
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| "未配置 Gateway 访问令牌，请先运行 OpenClaw 初始化".to_string())?
        .to_string();
    let request = Request::builder()
        .uri("ws://127.0.0.1:18789")
        .body(())
        .map_err(|e| format!("Gateway WS 请求创建失败: {e}"))?;
    let (mut socket, _) = connect(request).map_err(|e| format!("Gateway WS 连接失败: {e}"))?;

    if let tungstenite::stream::MaybeTlsStream::Plain(stream) = socket.get_mut() {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(90)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    }

    loop {
        let raw = socket
            .read()
            .map_err(|e| format!("Gateway WS 读取失败: {e}"))?;
        let text = raw
            .to_text()
            .map_err(|e| format!("Gateway WS 非文本消息: {e}"))?;
        let frame: Value =
            serde_json::from_str(text).map_err(|e| format!("Gateway WS JSON 解析失败: {e}"))?;
        if frame.get("type").and_then(Value::as_str) == Some("event")
            && frame.get("event").and_then(Value::as_str) == Some("connect.challenge")
        {
            break;
        }
    }

    socket
        .send(Message::Text(
            json!({
                "type": "req",
                "id": "connect",
                "method": "connect",
                "params": {
                    "minProtocol": 4,
                    "maxProtocol": 4,
                    "client": {
                        "id": "gateway-client",
                        "displayName": "OpenClaw Launcher",
                        "version": env!("CARGO_PKG_VERSION"),
                        "platform": std::env::consts::OS,
                        "mode": "backend"
                    },
                    "caps": [],
                    "role": "operator",
                    "scopes": ["operator.admin", "operator.read", "operator.write"],
                    "auth": { "token": gateway_token }
                }
            })
            .to_string(),
        ))
        .map_err(|e| format!("Gateway WS 握手发送失败: {e}"))?;

    wait_gateway_response(&mut socket, "connect", false)?;

    let id = format!("agent-{}", now_millis());
    socket
        .send(Message::Text(
            json!({
                "type": "req",
                "id": id,
                "method": "agent",
                "params": {
                    "message": message,
                    "sessionKey": "agent:main:launcher",
                    "idempotencyKey": format!("launcher-{}", now_millis()),
                    "cleanupBundleMcpOnRunEnd": true,
                    "timeout": 600
                }
            })
            .to_string(),
        ))
        .map_err(|e| format!("Gateway WS 对话发送失败: {e}"))?;

    let payload = wait_gateway_response(&mut socket, &id, true)?;
    Ok(GatewayAgentResult {
        output: format_agent_payload(&payload),
    })
}

fn wait_gateway_response(
    socket: &mut tungstenite::WebSocket<
        tungstenite::stream::MaybeTlsStream<std::net::TcpStream>,
    >,
    id: &str,
    expect_final: bool,
) -> Result<Value, String> {
    loop {
        let raw = socket
            .read()
            .map_err(|e| format!("Gateway WS 读取失败: {e}"))?;
        if !raw.is_text() {
            continue;
        }
        let text = raw
            .to_text()
            .map_err(|e| format!("Gateway WS 非文本消息: {e}"))?;
        let frame: Value =
            serde_json::from_str(text).map_err(|e| format!("Gateway WS JSON 解析失败: {e}"))?;
        if frame.get("type").and_then(Value::as_str) != Some("res") {
            continue;
        }
        if frame.get("id").and_then(Value::as_str) != Some(id) {
            continue;
        }
        if frame.get("ok").and_then(Value::as_bool) != Some(true) {
            let message = frame
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("Gateway WS 请求失败");
            return Err(message.to_string());
        }
        let payload = frame.get("payload").cloned().unwrap_or(Value::Null);
        if expect_final && payload.get("status").and_then(Value::as_str) == Some("accepted") {
            continue;
        }
        return Ok(payload);
    }
}

fn format_agent_payload(payload: &Value) -> String {
    if let Some(summary) = payload.get("summary").and_then(Value::as_str) {
        if !summary.trim().is_empty() {
            return summary.trim().to_string();
        }
    }

    let mut lines = Vec::new();
    if let Some(payloads) = payload.pointer("/result/payloads").and_then(Value::as_array) {
        for item in payloads {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                if !text.trim().is_empty() {
                    lines.push(text.trim().to_string());
                }
            }
            if let Some(media_url) = item.get("mediaUrl").and_then(Value::as_str) {
                if !media_url.trim().is_empty() {
                    lines.push(format!("Attachment: {}", media_url.trim()));
                }
            }
            if let Some(media_urls) = item.get("mediaUrls").and_then(Value::as_array) {
                for url in media_urls {
                    if let Some(url) = url.as_str() {
                        if !url.trim().is_empty() {
                            lines.push(format!("Attachment: {}", url.trim()));
                        }
                    }
                }
            }
        }
    }

    if lines.is_empty() {
        "没有收到回复。".to_string()
    } else {
        lines.join("\n")
    }
}

*/

fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .ok_or_else(|| "无法定位用户目录".to_string())
}

fn openclaw_config_path() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".openclaw").join("openclaw.json"))
}

fn read_openclaw_config() -> Result<Value, String> {
    let path = openclaw_config_path()?;
    let text = fs::read_to_string(&path).map_err(|e| format!("读取 OpenClaw 配置失败: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("OpenClaw 配置 JSON 解析失败: {e}"))
}

fn write_openclaw_config(config: &Value) -> Result<(), String> {
    let path = openclaw_config_path()?;
    if path.exists() {
        let backup = path.with_extension(format!("json.bak.launcher.{}", now_millis()));
        fs::copy(&path, backup).map_err(|e| format!("备份 OpenClaw 配置失败: {e}"))?;
    }
    let text = serde_json::to_string_pretty(config).map_err(|e| format!("序列化配置失败: {e}"))?;
    fs::write(&path, format!("{text}\n")).map_err(|e| format!("写入 OpenClaw 配置失败: {e}"))
}

fn ensure_object_path(config: &mut Value, path: &[&str]) -> Result<(), String> {
    let mut current = config;
    for key in path {
        if !current.is_object() {
            *current = json!({});
        }
        if current.get(*key).is_none() || !current[*key].is_object() {
            current[*key] = json!({});
        }
        current = current
            .get_mut(*key)
            .ok_or_else(|| format!("无法创建配置节点: {key}"))?;
    }
    Ok(())
}

fn normalize_provider_id(id: &str) -> Result<String, String> {
    let normalized = id
        .trim()
        .trim_matches('/')
        .replace(char::is_whitespace, "-")
        .to_lowercase();
    if normalized.is_empty() {
        return Err("供应商 ID 不能为空".to_string());
    }
    if !normalized
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err("供应商 ID 只能包含字母、数字、- 和 _".to_string());
    }
    Ok(normalized)
}

fn read_providers_from_config(config: &Value) -> Vec<ProviderConfig> {
    let primary = config
        .pointer("/agents/defaults/model/primary")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut providers = config
        .pointer("/models/providers")
        .and_then(Value::as_object)
        .map(|items| {
            items
                .iter()
                .map(|(id, value)| ProviderConfig {
                    id: id.to_string(),
                    base_url: string_field(value, &["baseUrl", "baseURL", "base_url", "url"]),
                    api_key: string_field(value, &["apiKey", "api_key", "key", "token"]),
                    api: string_field(value, &["api", "type"]),
                    models: value
                        .get("models")
                        .and_then(Value::as_array)
                        .map(|models| models.iter().filter_map(model_from_value).collect())
                        .unwrap_or_default(),
                    active: primary.starts_with(&format!("{id}/")),
                    use_full_path: uses_full_path(value),
                })
                .collect::<Vec<ProviderConfig>>()
        })
        .unwrap_or_default();
    providers.sort_by(|a, b| a.id.cmp(&b.id));
    providers
}

fn model_from_value(value: &Value) -> Option<ProviderModel> {
    if let Some(id) = value.as_str() {
        return Some(ProviderModel {
            id: id.to_string(),
            name: id.to_string(),
        });
    }
    let id = string_field(value, &["id", "model", "name"]);
    if id.is_empty() {
        return None;
    }
    let name = string_field(value, &["name", "label", "displayName"]);
    Some(ProviderModel {
        id: id.clone(),
        name: if name.is_empty() { id } else { name },
    })
}

fn string_field(value: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .unwrap_or_default()
        .to_string()
}

fn sync_agent_model_aliases(
    config: &mut Value,
    provider_id: &str,
    models: &[ProviderModel],
) -> Result<(), String> {
    ensure_object_path(config, &["agents", "defaults", "models"])?;
    for model in models {
        let full_id = format!("{provider_id}/{}", model.id.trim());
        config["agents"]["defaults"]["models"][full_id]["alias"] =
            Value::String(if model.name.trim().is_empty() {
                model.id.trim().to_string()
            } else {
                model.name.trim().to_string()
            });
    }
    Ok(())
}

fn set_active_provider_model(
    config: &mut Value,
    provider_id: &str,
    models: &[ProviderModel],
) -> Result<(), String> {
    let first = models
        .first()
        .ok_or_else(|| "启用供应商前至少需要一个模型".to_string())?;
    ensure_object_path(config, &["agents", "defaults", "model"])?;
    config["agents"]["defaults"]["model"]["primary"] =
        Value::String(format!("{provider_id}/{}", first.id.trim()));
    config["agents"]["defaults"]["model"]["fallbacks"] = Value::Array(
        models
            .iter()
            .skip(1)
            .map(|model| Value::String(format!("{provider_id}/{}", model.id.trim())))
            .collect(),
    );
    Ok(())
}

fn discover_external_providers() -> Vec<ProviderConfig> {
    let Ok(home) = home_dir() else {
        return vec![];
    };
    let candidates = [
        ".cc-switch/providers.json",
        ".cc-switch/settings.json",
        ".cc-switch/config.json",
        ".ccswitch/providers.json",
        ".ccswitch/settings.json",
        ".ccswitch/config.json",
        ".ccswich/providers.json",
        ".ccswich/settings.json",
        ".ccswich/config.json",
        ".claude/settings.json",
        ".claude.json",
    ];
    let mut paths = candidates
        .iter()
        .map(|relative| home.join(relative))
        .collect::<Vec<PathBuf>>();
    if let Some(app_data) = std::env::var_os("APPDATA") {
        let app_data = PathBuf::from(app_data);
        paths.push(app_data.join("cc-switch-desktop/config.json"));
        paths.push(app_data.join("com.ccswitch.desktop/config.json"));
    }

    let mut providers = BTreeMap::<String, ProviderConfig>::new();
    for path in paths {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        collect_provider_like_values(&value, "", &mut providers);
    }
    providers.into_values().collect()
}

fn collect_provider_like_values(
    value: &Value,
    key_hint: &str,
    providers: &mut BTreeMap<String, ProviderConfig>,
) {
    match value {
        Value::Object(map) => {
            let base_url = string_field(
                value,
                &["baseUrl", "baseURL", "base_url", "api_base_url", "apiBaseUrl", "url"],
            );
            let api_key = string_field(value, &["apiKey", "api_key", "authToken", "token", "key"]);
            if !base_url.is_empty() {
                let hinted_id = string_field(value, &["provider_type", "providerType", "provider", "name", "id"]);
                let id = normalize_provider_id(if hinted_id.is_empty() {
                    key_hint
                } else {
                    &hinted_id
                })
                .unwrap_or_else(|_| format!("provider-{}", providers.len() + 1));
                let models = value
                    .get("models")
                    .or_else(|| value.get("availableModels"))
                    .and_then(Value::as_array)
                    .map(|items| items.iter().filter_map(model_from_value).collect())
                    .unwrap_or_else(|| {
                        let model = string_field(value, &["model", "defaultModel"]);
                        if model.is_empty() {
                            vec![ProviderModel {
                                id: "default".to_string(),
                                name: "default".to_string(),
                            }]
                        } else {
                            vec![ProviderModel {
                                id: model.clone(),
                                name: model,
                            }]
                        }
                    });
                providers.entry(id.clone()).or_insert(ProviderConfig {
                    id,
                    base_url: normalize_external_base_url(&base_url),
                    api_key,
                    api: provider_api(value),
                    models,
                    active: false,
                    use_full_path: false,
                });
            }
            for (key, child) in map {
                collect_provider_like_values(child, key, providers);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_provider_like_values(item, key_hint, providers);
            }
        }
        _ => {}
    }
}

fn normalize_external_base_url(value: &str) -> String {
    let value = value.trim().trim_end_matches('/');
    value
        .strip_suffix("/chat/completions")
        .or_else(|| value.strip_suffix("/messages"))
        .unwrap_or(value)
        .to_string()
}

fn uses_full_path(value: &Value) -> bool {
    string_field(value, &["baseUrl", "baseURL", "base_url", "url"])
        .trim_end_matches('/')
        .ends_with("/chat/completions")
        || string_field(value, &["baseUrl", "baseURL", "base_url", "url"])
            .trim_end_matches('/')
            .ends_with("/messages")
}

fn provider_api(value: &Value) -> String {
    let api = string_field(value, &["api", "type"]);
    if !api.is_empty() {
        return api;
    }

    match string_field(value, &["provider_type", "providerType"]).to_lowercase().as_str() {
        "anthropic" | "claude" => "anthropic-messages".to_string(),
        _ => "openai-completions".to_string(),
    }
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn clean_openclaw_output(output: &str) -> String {
    let mut cleaned = Vec::new();
    let mut skipping_doctor_box = false;

    for line in output.lines() {
        if line.contains("Doctor warnings") {
            skipping_doctor_box = true;
            continue;
        }
        if skipping_doctor_box {
            let trimmed = line.trim();
            if trimmed == "|" || trimmed.starts_with('+') {
                continue;
            }
            if line.contains("[state-migrations]")
                || line.contains("Legacy state migration warnings")
                || line.contains("Left legacy config health state in place")
                || line.contains("config-health.json")
                || line.starts_with('|')
            {
                continue;
            }
            if trimmed.is_empty() {
                skipping_doctor_box = false;
                continue;
            }
            skipping_doctor_box = false;
        }
        if line.contains("[state-migrations]")
            || line.contains("Legacy state migration warnings")
            || line.contains("Left legacy config health state in place")
            || line.contains("config-health.json")
        {
            continue;
        }
        cleaned.push(line);
    }

    cleaned.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_ccswitch_desktop_provider_shape() {
        let value = json!({
            "providers": [{
                "id": "7f7ca4f3-6164-4ba1-bc69-b2f8a9900e17",
                "name": "DeepSeek",
                "provider_type": "deepseek",
                "api_key": "test-key",
                "base_url": "https://api.deepseek.com/v1/chat/completions",
                "model": "deepseek-chat"
            }]
        });
        let mut providers = BTreeMap::new();

        collect_provider_like_values(&value, "", &mut providers);

        let provider = providers.get("deepseek").expect("provider should be imported");
        assert_eq!(provider.api, "openai-completions");
        assert_eq!(provider.api_key, "test-key");
        assert_eq!(provider.base_url, "https://api.deepseek.com/v1");
        assert!(!provider.use_full_path);
        assert_eq!(provider.models[0].id, "deepseek-chat");
        assert!(uses_full_path(&json!({ "baseUrl": "https://example.com/v1/chat/completions" })));
    }
}

fn emit_log(app: &tauri::AppHandle, text: &str, level: &str) {
    let log = LogLine {
        text: text.to_string(),
        level: level.to_string(),
    };
    let _ = app.emit("log", &log);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                window.app_handle().exit(0);
            }
        })
        .invoke_handler(tauri::generate_handler![
            check_env,
            close_window,
            activate_provider,
            delete_provider,
            fetch_provider_models,
            import_ccswitch_providers,
            install_node,
            install_openclaw,
            launch_openclaw,
            list_providers,
            open_dashboard,
            run_openclaw_command,
            save_provider,
            uninstall_openclaw,
        ])
        .run(tauri::generate_context!())
        .expect("error running tauri app");
}
