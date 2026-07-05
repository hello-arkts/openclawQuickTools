import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';

interface EnvStatus {
  node_installed: boolean;
  node_version: string;
  npm_installed: boolean;
  npm_version: string;
  openclaw_installed: boolean;
  openclaw_version: string;
  gateway_running: boolean;
  platform: string;
}

interface CommandResult {
  success: boolean;
  output: string;
}

interface Flow {
  name: string;
  prompt: string;
}

const FLOW_STORAGE_KEY = 'openclaw-launcher.flows';
const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const logArea = $('log-area');
const progressFill = $('progress-fill');
const chatHistory = $('chat-history');

let envReady = false;
let statusRefreshInFlight = false;

function escapeHtml(value: string) {
  return value.replace(/[&<>"']/g, (char) => ({
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#039;',
  }[char]!));
}

function log(message: string, level = 'info') {
  if (logArea.textContent === '等待操作...') {
    logArea.textContent = '';
  }

  const time = new Date().toLocaleTimeString('zh-CN', { hour12: false });
  const line = document.createElement('div');
  line.className = `log-line ${level}`;
  line.innerHTML = `<span class="time">${time}</span>${escapeHtml(message)}`;
  logArea.appendChild(line);
  logArea.scrollTop = logArea.scrollHeight;
}

function addChatMessage(role: 'user' | 'assistant' | 'system', text: string) {
  const message = document.createElement('div');
  message.className = `message ${role}`;
  message.textContent = text;
  chatHistory.appendChild(message);
  chatHistory.scrollTop = chatHistory.scrollHeight;
}

function setProgress(value: number) {
  progressFill.style.width = `${value}%`;
  if (value === 100) {
    window.setTimeout(() => setProgress(0), 700);
  }
}

function setStatus(id: string, text: string, cls: string) {
  const element = $(id);
  element.textContent = text;
  element.className = `value ${cls}`;
}

function updateStatus(env: EnvStatus) {
  envReady = env.openclaw_installed;
  setStatus('st-node', env.node_installed ? env.node_version : '未安装', env.node_installed ? 'ok' : 'no');
  setStatus('st-npm', env.npm_installed ? env.npm_version : '未安装', env.npm_installed ? 'ok' : 'no');
  setStatus('st-oc', env.openclaw_installed ? env.openclaw_version : '未安装', env.openclaw_installed ? 'ok' : 'no');
  setStatus('st-gw', env.gateway_running ? '运行中' : '未运行', env.gateway_running ? 'ok' : 'warn');

  $('btn-launch').toggleAttribute('disabled', !env.openclaw_installed);
  $('btn-install-node').toggleAttribute('disabled', env.node_installed);
  $('btn-install-oc').toggleAttribute('disabled', env.openclaw_installed || !env.node_installed);
  $('btn-uninstall').toggleAttribute('disabled', !env.openclaw_installed);
}

async function checkEnv() {
  if (statusRefreshInFlight) return;
  statusRefreshInFlight = true;

  try {
    const env = await invoke<EnvStatus>('check_env');
    updateStatus(env);
  } catch (error) {
    log(`环境检测失败: ${error}`, 'error');
  } finally {
    statusRefreshInFlight = false;
  }
}

async function withBusy<T>(buttonId: string, label: string, task: () => Promise<T>) {
  const button = $(buttonId) as HTMLButtonElement;
  const originalText = button.textContent || '';
  button.disabled = true;
  button.classList.add('busy');
  button.textContent = label;

  try {
    return await task();
  } finally {
    button.classList.remove('busy');
    button.textContent = originalText;
    button.disabled = false;
    await checkEnv();
  }
}

async function openclaw(args: string[]) {
  if (!envReady) {
    throw new Error('OpenClaw 未安装，无法执行命令。');
  }
  return invoke<CommandResult>('run_openclaw_command', { args });
}

async function runOpenClaw(args: string[], outputId: string, buttonId: string) {
  await withBusy(buttonId, '执行中...', async () => {
    const output = $(outputId);
    output.textContent = `执行中: openclaw ${args.join(' ')}`;

    try {
      const result = await openclaw(args);
      output.textContent = result.output || '(无输出)';
      if (!result.success) {
        log(`命令失败: openclaw ${args.join(' ')}`, 'error');
      }
    } catch (error) {
      output.textContent = String(error);
      log(String(error), 'error');
    }
  });
}

function inputValue(id: string) {
  return ($(id) as HTMLInputElement).value.trim();
}

function loadFlows(): Flow[] {
  try {
    const raw = localStorage.getItem(FLOW_STORAGE_KEY);
    return raw ? JSON.parse(raw) : [];
  } catch {
    return [];
  }
}

function saveFlows(flows: Flow[]) {
  localStorage.setItem(FLOW_STORAGE_KEY, JSON.stringify(flows));
  renderFlows();
}

function normalizeFlowName(name: string) {
  return name.trim().replace(/^\/+/, '').replace(/\s+/g, '-');
}

function renderFlows() {
  const flows = loadFlows();
  const custom = flows.length
    ? flows.map((flow) => `/${flow.name}\n  ${flow.prompt}`).join('\n\n')
    : '暂无自定义流程。';

  $('flows-output').textContent = [
    '内置 / 命令：',
    '/help - 查看命令',
    '/status - 查看 OpenClaw 状态',
    '/agents - 查看 Agents',
    '/skills - 查看 Skills',
    '/tasks - 查看定时任务',
    '/dashboard - 打开控制台',
    '/restart - 重启 Gateway',
    '',
    '自定义流程：',
    custom,
  ].join('\n');
}

function getSlashHelp() {
  const flows = loadFlows();
  const flowLines = flows.length
    ? flows.map((flow) => `/${flow.name} <输入> - 执行自定义流程`).join('\n')
    : '暂无自定义流程。';

  return [
    '可用 / 命令：',
    '/help',
    '/status',
    '/agents',
    '/skills',
    '/tasks',
    '/dashboard',
    '/restart',
    '',
    '自定义流程：',
    flowLines,
  ].join('\n');
}

async function runAgentMessage(message: string) {
  const result = await openclaw(['agent', '--message', message]);
  return result.output || '(无输出)';
}

async function handleSlashCommand(raw: string) {
  const [commandRaw, ...rest] = raw.slice(1).trim().split(/\s+/);
  const command = commandRaw.toLowerCase();
  const input = rest.join(' ');

  if (!command) return getSlashHelp();

  if (command === 'help') return getSlashHelp();
  if (command === 'status') return (await openclaw(['status'])).output || '(无输出)';
  if (command === 'agents') return (await openclaw(['agents', 'list'])).output || '(无输出)';
  if (command === 'skills') return (await openclaw(['skills', 'list'])).output || '(无输出)';
  if (command === 'tasks') return (await openclaw(['cron', 'list', '--all'])).output || '(无输出)';
  if (command === 'dashboard') {
    await invoke('open_dashboard');
    return '已打开 OpenClaw 控制台。';
  }
  if (command === 'restart') {
    await invoke('launch_openclaw');
    return 'Gateway 已请求重启。';
  }

  const flow = loadFlows().find((item) => item.name.toLowerCase() === command);
  if (!flow) {
    return `未知命令：/${command}\n\n${getSlashHelp()}`;
  }

  const prompt = flow.prompt.includes('{{input}}')
    ? flow.prompt.replaceAll('{{input}}', input)
    : [flow.prompt, input].filter(Boolean).join('\n\n输入：');
  return runAgentMessage(prompt);
}

async function sendChat() {
  const input = $('chat-input') as HTMLInputElement;
  const message = input.value.trim();
  if (!message) return;

  input.value = '';
  addChatMessage('user', message);

  await withBusy('btn-chat-send', '发送中...', async () => {
    try {
      const reply = message.startsWith('/')
        ? await handleSlashCommand(message)
        : await runAgentMessage(message);
      addChatMessage('assistant', reply);
    } catch (error) {
      addChatMessage('system', `执行失败：${error}`);
      log(`对话执行失败: ${error}`, 'error');
    }
  });
}

function setupTabs() {
  document.querySelectorAll<HTMLButtonElement>('.tab').forEach((tab) => {
    tab.onclick = () => {
      document.querySelectorAll('.tab').forEach((item) => item.classList.remove('active'));
      document.querySelectorAll('.panel').forEach((item) => item.classList.remove('active'));
      tab.classList.add('active');
      $(`panel-${tab.dataset.tab}`).classList.add('active');
    };
  });
}

function setupLauncherActions() {
  $('btn-launch').onclick = () => withBusy('btn-launch', '重启中...', async () => {
    setProgress(40);
    try {
      await invoke('launch_openclaw');
      setProgress(100);
    } catch (error) {
      setProgress(0);
      log(`重启失败: ${error}`, 'error');
    }
  });

  $('btn-refresh-status').onclick = () => withBusy('btn-refresh-status', '刷新中...', async () => {
    setProgress(35);
    await checkEnv();
    setProgress(100);
    log('状态已刷新', 'success');
  });

  $('btn-dashboard').onclick = () => withBusy('btn-dashboard', '打开中...', async () => {
    try {
      await invoke('open_dashboard');
    } catch (error) {
      log(`打开界面失败: ${error}`, 'error');
    }
  });

  $('btn-install-node').onclick = () => withBusy('btn-install-node', '安装中...', async () => {
    setProgress(35);
    try {
      await invoke('install_node');
      setProgress(100);
    } catch (error) {
      setProgress(0);
      log(`安装 Node.js 失败: ${error}`, 'error');
    }
  });

  $('btn-install-oc').onclick = () => withBusy('btn-install-oc', '安装中...', async () => {
    setProgress(35);
    try {
      await invoke('install_openclaw');
      setProgress(100);
    } catch (error) {
      setProgress(0);
      log(`安装 OpenClaw 失败: ${error}`, 'error');
    }
  });

  $('btn-uninstall').onclick = () => {
    if (!confirm('确定卸载 OpenClaw CLI？')) return;
    void withBusy('btn-uninstall', '卸载中...', async () => {
      setProgress(35);
      try {
        await invoke('uninstall_openclaw');
        setProgress(100);
      } catch (error) {
        setProgress(0);
        log(`卸载失败: ${error}`, 'error');
      }
    });
  };
}

function setupChatActions() {
  $('btn-chat-send').onclick = () => void sendChat();
  $('chat-input').addEventListener('keydown', (event) => {
    const keyboardEvent = event as KeyboardEvent;
    if (keyboardEvent.key === 'Enter') {
      keyboardEvent.preventDefault();
      void sendChat();
    }
  });
  $('btn-chat-clear').onclick = () => {
    chatHistory.innerHTML = '<div class="message system">对话已清空。输入 /help 查看可用命令。</div>';
  };
}

function setupFlowActions() {
  $('btn-flow-save').onclick = () => {
    const name = normalizeFlowName(inputValue('flow-name'));
    const prompt = ($('flow-prompt') as HTMLTextAreaElement).value.trim();
    if (!name) return log('请输入流程名', 'warn');
    if (!/^[a-zA-Z0-9_-]+$/.test(name)) return log('流程名只能包含字母、数字、- 和 _', 'warn');
    if (!prompt) return log('请输入流程提示词', 'warn');

    const flows = loadFlows().filter((flow) => flow.name !== name);
    flows.push({ name, prompt });
    saveFlows(flows.sort((a, b) => a.name.localeCompare(b.name)));
    ($('flow-name') as HTMLInputElement).value = name;
    addChatMessage('system', `流程 /${name} 已保存。`);
  };

  $('btn-flow-delete').onclick = () => {
    const name = normalizeFlowName(inputValue('flow-name'));
    if (!name) return log('请输入要删除的流程名', 'warn');
    saveFlows(loadFlows().filter((flow) => flow.name !== name));
    addChatMessage('system', `流程 /${name} 已删除。`);
  };
}

function setupAgentActions() {
  $('btn-agents-refresh').onclick = () => runOpenClaw(['agents', 'list'], 'agents-output', 'btn-agents-refresh');
  $('btn-agent-add').onclick = () => {
    const name = inputValue('agent-name');
    if (!name) return log('请输入 Agent 名称', 'warn');
    void runOpenClaw(['agents', 'add', name], 'agents-output', 'btn-agent-add');
  };
  $('btn-agent-delete').onclick = () => {
    const name = inputValue('agent-name');
    if (!name) return log('请输入要删除的 Agent 名称', 'warn');
    if (confirm(`确定删除 Agent "${name}"？`)) {
      void runOpenClaw(['agents', 'delete', name], 'agents-output', 'btn-agent-delete');
    }
  };
}

function setupSkillActions() {
  $('btn-skills-refresh').onclick = () => runOpenClaw(['skills', 'list'], 'skills-output', 'btn-skills-refresh');
  $('btn-skill-info').onclick = () => {
    const query = inputValue('skill-query');
    if (!query) return log('请输入 Skill 名称', 'warn');
    void runOpenClaw(['skills', 'info', query], 'skills-output', 'btn-skill-info');
  };
  $('btn-skill-search').onclick = () => {
    const query = inputValue('skill-query');
    if (!query) return log('请输入搜索词', 'warn');
    void runOpenClaw(['skills', 'search', query], 'skills-output', 'btn-skill-search');
  };
  $('btn-skill-install').onclick = () => {
    const query = inputValue('skill-query');
    if (!query) return log('请输入要安装的 Skill 名称或来源', 'warn');
    void runOpenClaw(['skills', 'install', query], 'skills-output', 'btn-skill-install');
  };
}

function setupTaskActions() {
  $('btn-tasks-refresh').onclick = () => runOpenClaw(['cron', 'list', '--all'], 'tasks-output', 'btn-tasks-refresh');
  $('btn-tasks-status').onclick = () => runOpenClaw(['cron', 'status'], 'tasks-output', 'btn-tasks-status');
  $('btn-task-add').onclick = () => {
    const name = inputValue('task-name');
    const every = inputValue('task-every');
    const message = inputValue('task-message');
    if (!name) return log('请输入任务名称', 'warn');
    if (!every) return log('请输入任务间隔，例如 1h 或 30m', 'warn');
    if (!message) return log('请输入任务内容', 'warn');
    void runOpenClaw(['cron', 'add', '--name', name, '--every', every, '--message', message], 'tasks-output', 'btn-task-add');
  };
  $('btn-task-run').onclick = () => {
    const id = inputValue('task-id');
    if (!id) return log('请输入任务 ID', 'warn');
    void runOpenClaw(['cron', 'run', id], 'tasks-output', 'btn-task-run');
  };
  $('btn-task-delete').onclick = () => {
    const id = inputValue('task-id');
    if (!id) return log('请输入要删除的任务 ID', 'warn');
    if (confirm(`确定删除定时任务 "${id}"？`)) {
      void runOpenClaw(['cron', 'rm', id], 'tasks-output', 'btn-task-delete');
    }
  };
}

function setupWindowActions() {
  $('btn-clear-log').onclick = () => {
    logArea.textContent = '等待操作...';
  };
  $('btn-min').onclick = () => {
    void getCurrentWindow().minimize();
  };
  $('btn-close').onclick = async () => {
    await invoke('close_window');
  };
}

void listen<{ text: string; level: string }>('log', (event) => {
  log(event.payload.text, event.payload.level);
});

setupTabs();
setupLauncherActions();
setupChatActions();
setupFlowActions();
setupAgentActions();
setupSkillActions();
setupTaskActions();
setupWindowActions();
renderFlows();
void checkEnv();
window.setInterval(checkEnv, 30000);
