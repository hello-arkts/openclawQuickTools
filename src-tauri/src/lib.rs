use serde::{Deserialize, Serialize};
use std::process::Stdio;
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

#[tauri::command]
fn check_env() -> EnvStatus {
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
async fn install_node(app: tauri::AppHandle) -> Result<(), String> {
    emit_log(&app, "开始安装 Node.js...", "info");

    let mut command = if cfg!(windows) && run_cmd("winget", &["--version"]).0 {
        let mut command = tokio::process::Command::new("winget");
        command.args([
            "install",
            "OpenJS.NodeJS.LTS",
            "--silent",
            "--accept-package-agreements",
            "--accept-source-agreements",
        ]);
        command
    } else if cfg!(target_os = "macos") && run_cmd("brew", &["--version"]).0 {
        let mut command = tokio::process::Command::new("brew");
        command.args(["install", "node"]);
        command
    } else if cfg!(windows) {
        let mut command = tokio::process::Command::new("powershell");
        command.args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            r#"$url='https://nodejs.org/dist/v22.21.1/node-v22.21.1-x64.msi'; $tmp="$env:TEMP\node-installer.msi"; Invoke-WebRequest -Uri $url -OutFile $tmp -UseBasicParsing; Start-Process msiexec.exe -ArgumentList "/i `"$tmp`" /quiet /norestart" -Wait -WindowStyle Hidden; Remove-Item $tmp -Force"#,
        ]);
        command
    } else {
        let mut command = tokio::process::Command::new("bash");
        command.args(["-c", "curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash && export NVM_DIR=\"$HOME/.nvm\" && [ -s \"$NVM_DIR/nvm.sh\" ] && . \"$NVM_DIR/nvm.sh\" && nvm install 22 && nvm use 22"]);
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

#[tauri::command]
fn run_openclaw_command(args: Vec<String>) -> Result<CommandResult, String> {
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
fn open_dashboard() -> Result<(), String> {
    open_control_ui()
}

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
            install_node,
            install_openclaw,
            launch_openclaw,
            open_dashboard,
            run_openclaw_command,
            uninstall_openclaw,
        ])
        .run(tauri::generate_context!())
        .expect("error running tauri app");
}
