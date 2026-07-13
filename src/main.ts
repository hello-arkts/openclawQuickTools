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

interface ProviderModel {
  id: string;
  name: string;
}

interface ProviderConfig {
  id: string;
  base_url: string;
  api_key: string;
  api: string;
  models: ProviderModel[];
  active: boolean;
  use_full_path: boolean;
}

interface OpenClawUpdateStatus {
  availability?: {
    available?: boolean;
    latestVersion?: string | null;
  };
  update?: {
    registry?: {
      latestVersion?: string | null;
    };
  };
}

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const logArea = $('log-area');
const progressFill = $('progress-fill');

let envReady = false;
let statusRefreshInFlight = false;
let progressTimer: number | null = null;
let progressValue = 0;
const busyButtons = new Set<string>();
let currentProviders: ProviderConfig[] = [];
let upstreamModels: ProviderModel[] = [];
let upstreamModelsProviderId = '';

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

function setProgress(value: number) {
  progressValue = Math.max(0, Math.min(100, value));
  progressFill.style.width = `${progressValue}%`;
  if (value === 100) {
    window.setTimeout(() => setProgress(0), 700);
  }
}

function startProgress() {
  stopProgress(false);
  setProgress(Math.max(progressValue, 12));
  progressTimer = window.setInterval(() => {
    const next = progressValue < 45 ? progressValue + 7 : progressValue < 75 ? progressValue + 4 : progressValue + 1;
    setProgress(Math.min(next, 92));
  }, 450);
}

function stopProgress(complete: boolean) {
  if (progressTimer !== null) {
    window.clearInterval(progressTimer);
    progressTimer = null;
  }
  if (complete) setProgress(100);
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

async function checkEnv(force = false) {
  if (busyButtons.has('btn-refresh-status') && !force) return;
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

async function withBusy<T>(
  buttonId: string,
  label: string,
  task: () => Promise<T>,
  options: { refreshEnv?: boolean; progress?: boolean } = {},
) {
  if (busyButtons.has(buttonId)) {
    log('该操作正在执行，请稍候', 'warn');
    return undefined as T;
  }
  const button = $(buttonId) as HTMLButtonElement;
  const originalText = button.textContent || '';
  busyButtons.add(buttonId);
  button.classList.add('busy');
  button.textContent = label;
  if (options.progress) startProgress();

  try {
    const result = await task();
    if (options.progress && progressValue > 0) {
      stopProgress(true);
    }
    return result;
  } finally {
    if (options.progress) {
      stopProgress(false);
    }
    button.classList.remove('busy');
    button.textContent = originalText;
    busyButtons.delete(buttonId);
    if (options.refreshEnv) await checkEnv(true);
  }
}

async function openclaw(args: string[]) {
  const result = await invoke<CommandResult>('run_openclaw_command', { args });
  return {
    ...result,
    output: cleanOpenClawOutput(result.output),
  };
}

function cleanOpenClawOutput(output: string) {
  const lines = output.split(/\r?\n/);
  const cleaned: string[] = [];
  let skippingDoctorBox = false;

  for (const line of lines) {
    if (line.includes('Doctor warnings')) {
      skippingDoctorBox = true;
      continue;
    }
    if (skippingDoctorBox) {
      if (line.trim().startsWith('+') || line.trim() === '|') {
        continue;
      }
      if (line.includes('[state-migrations]') || line.includes('Legacy state migration warnings')) {
        continue;
      }
      if (line.trim() === '') {
        skippingDoctorBox = false;
        continue;
      }
      if (line.includes('Left legacy config health state in place')) continue;
      if (line.includes('config-health.json')) continue;
      if (/^\|\s*$/.test(line) || /^\|/.test(line)) continue;
      skippingDoctorBox = false;
    }
    if (line.includes('[state-migrations]')) continue;
    if (line.includes('Legacy state migration warnings')) continue;
    if (line.includes('Left legacy config health state in place')) continue;
    if (line.includes('config-health.json')) continue;
    cleaned.push(line);
  }

  return cleaned.join('\n').replace(/\n{3,}/g, '\n\n').trim();
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
  }, { progress: true });
}

function inputValue(id: string) {
  return ($(id) as HTMLInputElement).value.trim();
}

function textValue(id: string) {
  return ($(id) as HTMLTextAreaElement).value.trim();
}

/* Removed chat and flow implementation.
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

function builtInSlashCommands(): SlashCommand[] {
  return [
    { name: 'help', value: '/help', description: '查看所有内置命令和自定义流程' },
    { name: 'status', value: '/status', description: '查看 OpenClaw 状态' },
    { name: 'agents', value: '/agents', description: '查看 Agents 列表' },
    { name: 'skills', value: '/skills', description: '查看 Skills 列表' },
    { name: 'tasks', value: '/tasks', description: '查看定时任务' },
    { name: 'dashboard', value: '/dashboard', description: '打开 OpenClaw 控制台' },
    { name: 'restart', value: '/restart', description: '重启 Gateway' },
  ];
}

function slashCommands(): SlashCommand[] {
  const flows = loadFlows().map((flow) => ({
    name: flow.name,
    value: `/${flow.name}`,
    description: `自定义流程：${flow.prompt.slice(0, 64)}${flow.prompt.length > 64 ? '...' : ''}`,
  }));
  return [...builtInSlashCommands(), ...flows];
}

function slashQuery(value: string) {
  if (!value.startsWith('/')) return null;
  return value.slice(1).split(/\s+/)[0].toLowerCase();
}

function hideSlashMenu() {
  slashMenu.classList.remove('open');
  slashMenu.innerHTML = '';
  activeSlashIndex = 0;
}

function fillSlashCommand(command: SlashCommand) {
  const input = $('chat-input') as HTMLInputElement;
  input.value = `${command.value} `;
  input.focus();
  input.setSelectionRange(input.value.length, input.value.length);
  hideSlashMenu();
}

function renderSlashMenu() {
  const input = $('chat-input') as HTMLInputElement;
  const query = slashQuery(input.value);
  if (query === null || input.value.includes(' ')) {
    hideSlashMenu();
    return;
  }

  const commands = slashCommands().filter((command) => command.name.toLowerCase().includes(query));
  if (commands.length === 0) {
    hideSlashMenu();
    return;
  }

  activeSlashIndex = Math.min(activeSlashIndex, commands.length - 1);
  slashMenu.innerHTML = '';
  commands.forEach((command, index) => {
    const item = document.createElement('button');
    item.type = 'button';
    item.className = `slash-item${index === activeSlashIndex ? ' active' : ''}`;
    item.innerHTML = `<div class="slash-name">${escapeHtml(command.value)}</div><div class="slash-desc">${escapeHtml(command.description)}</div>`;
    item.onmousedown = (event) => {
      event.preventDefault();
      fillSlashCommand(command);
    };
    slashMenu.appendChild(item);
  });
  slashMenu.classList.add('open');
}

function moveSlashSelection(delta: number) {
  const count = slashMenu.querySelectorAll('.slash-item').length;
  if (count === 0) return;
  activeSlashIndex = (activeSlashIndex + delta + count) % count;
  renderSlashMenu();
}

function acceptSlashSelection() {
  const commands = slashCommands().filter((command) => {
    const query = slashQuery(($('chat-input') as HTMLInputElement).value) || '';
    return command.name.toLowerCase().includes(query);
  });
  const command = commands[activeSlashIndex];
  if (command) fillSlashCommand(command);
}

function shouldAcceptSlashWithEnter() {
  const input = ($('chat-input') as HTMLInputElement).value.trim();
  return input === '/' || !slashCommands().some((command) => input === command.value);
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
  const result = await invoke<GatewayAgentResult>('gateway_agent_message', { message });
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
  if (busyButtons.has('btn-chat-send')) {
    log('正在等待上一条对话响应，请稍候', 'warn');
    return;
  }
  const input = $('chat-input') as HTMLInputElement;
  const message = input.value.trim();
  if (!message) return;

  input.value = '';
  addChatMessage('user', message);
  const pending = addChatMessage('system', '正在通过 Gateway WebSocket 等待响应...');

  await withBusy('btn-chat-send', '发送中...', async () => {
    try {
      const reply = message.startsWith('/')
        ? await handleSlashCommand(message)
        : await runAgentMessage(message);
      pending.remove();
      addChatMessage('assistant', reply);
    } catch (error) {
      pending.remove();
      addChatMessage('system', `执行失败：${error}`);
      log(`对话执行失败: ${error}`, 'error');
    }
  }, { progress: true });
}

*/

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
    try {
      await invoke('launch_openclaw');
    } catch (error) {
      setProgress(0);
      log(`重启失败: ${error}`, 'error');
    }
  }, { refreshEnv: true, progress: true });

  $('btn-refresh-status').onclick = () => withBusy('btn-refresh-status', '刷新中...', async () => {
    await checkEnv();
    log('状态已刷新', 'success');
  }, { progress: true });

  $('btn-doctor-fix').onclick = () => withBusy('btn-doctor-fix', '修复中...', async () => {
    try {
      const result = await openclaw(['doctor', '--fix']);
      log(result.output || '修复完成', result.success ? 'success' : 'warn');
    } catch (error) {
      setProgress(0);
      log(`一键修复失败: ${error}`, 'error');
    }
  }, { refreshEnv: true, progress: true });

  $('btn-dashboard').onclick = () => withBusy('btn-dashboard', '打开中...', async () => {
    try {
      await invoke('open_dashboard');
    } catch (error) {
      log(`打开界面失败: ${error}`, 'error');
    }
  });

  $('btn-check-update').onclick = () => withBusy('btn-check-update', '检查中...', async () => {
    try {
      const result = await openclaw(['update', 'status', '--json']);
      if (!result.success) {
        log(result.output || '检查更新失败', 'warn');
        return;
      }

      const status = JSON.parse(result.output) as OpenClawUpdateStatus;
      const latestVersion = status.availability?.latestVersion || status.update?.registry?.latestVersion || '最新版本';
      if (!status.availability?.available) {
        log(`OpenClaw 已是最新版本${latestVersion ? `：${latestVersion}` : ''}`, 'success');
        return;
      }

      log(`检测到 OpenClaw 新版本：${latestVersion}`, 'warn');
      if (!confirm(`检测到 OpenClaw 新版本：${latestVersion}\n是否立即升级？`)) return;

      log('开始升级 OpenClaw...', 'info');
      const updateResult = await openclaw(['update', '--yes']);
      log(updateResult.output || 'OpenClaw 升级完成', updateResult.success ? 'success' : 'error');
    } catch (error) {
      log(`检查更新失败: ${error}`, 'error');
    }
  }, { refreshEnv: true, progress: true });

  $('btn-install-node').onclick = () => withBusy('btn-install-node', '安装中...', async () => {
    try {
      await invoke('install_node');
    } catch (error) {
      setProgress(0);
      log(`安装 Node.js 失败: ${error}`, 'error');
    }
  }, { refreshEnv: true, progress: true });

  $('btn-install-oc').onclick = () => withBusy('btn-install-oc', '安装中...', async () => {
    try {
      await invoke('install_openclaw');
    } catch (error) {
      setProgress(0);
      log(`安装 OpenClaw 失败: ${error}`, 'error');
    }
  }, { refreshEnv: true, progress: true });

  $('btn-uninstall').onclick = () => {
    if (!confirm('确定卸载 OpenClaw CLI？')) return;
    void withBusy('btn-uninstall', '卸载中...', async () => {
      try {
        await invoke('uninstall_openclaw');
      } catch (error) {
        setProgress(0);
        log(`卸载失败: ${error}`, 'error');
      }
    }, { refreshEnv: true, progress: true });
  };
}

/* Removed chat and flow setup.
function setupChatActions() {
  $('btn-chat-send').onclick = () => void sendChat();
  $('chat-input').addEventListener('input', () => {
    activeSlashIndex = 0;
    renderSlashMenu();
  });
  $('chat-input').addEventListener('blur', () => {
    window.setTimeout(hideSlashMenu, 120);
  });
  $('chat-input').addEventListener('keydown', (event) => {
    const keyboardEvent = event as KeyboardEvent;
    if (slashMenu.classList.contains('open')) {
      if (keyboardEvent.key === 'ArrowDown') {
        keyboardEvent.preventDefault();
        moveSlashSelection(1);
        return;
      }
      if (keyboardEvent.key === 'ArrowUp') {
        keyboardEvent.preventDefault();
        moveSlashSelection(-1);
        return;
      }
      if (keyboardEvent.key === 'Tab') {
        keyboardEvent.preventDefault();
        acceptSlashSelection();
        return;
      }
      if (keyboardEvent.key === 'Escape') {
        keyboardEvent.preventDefault();
        hideSlashMenu();
        return;
      }
    }
    if (keyboardEvent.key === 'Enter') {
      keyboardEvent.preventDefault();
      if (slashMenu.classList.contains('open') && shouldAcceptSlashWithEnter()) {
        acceptSlashSelection();
        return;
      }
      hideSlashMenu();
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
    renderSlashMenu();
    addChatMessage('system', `流程 /${name} 已保存。`);
  };

  $('btn-flow-delete').onclick = () => {
    const name = normalizeFlowName(inputValue('flow-name'));
    if (!name) return log('请输入要删除的流程名', 'warn');
    saveFlows(loadFlows().filter((flow) => flow.name !== name));
    renderSlashMenu();
    addChatMessage('system', `流程 /${name} 已删除。`);
  };
}

*/

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

function maskKey(value: string) {
  if (!value) return '';
  if (value.length <= 10) return '*'.repeat(value.length);
  return `${value.slice(0, 4)}...${value.slice(-4)}`;
}

function parseProviderModels(): ProviderModel[] {
  return textValue('provider-models')
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const [id, name] = line.split('|').map((item) => item.trim());
      return { id, name: name || id };
    });
}

function renderUpstreamModels() {
  const picker = $('provider-model-picker');
  const providerId = inputValue('provider-id');
  picker.innerHTML = '';
  if (!providerId || upstreamModelsProviderId !== providerId) return;

  const configured = new Set(parseProviderModels().map((model) => model.id));
  for (const model of upstreamModels) {
    const chip = document.createElement('button');
    chip.type = 'button';
    chip.className = `model-chip${configured.has(model.id) ? ' selected' : ''}`;
    chip.title = model.name;
    chip.textContent = model.name === model.id ? model.id : `${model.name} (${model.id})`;
    chip.onclick = () => {
      const models = new Map(parseProviderModels().map((item) => [item.id, item]));
      if (models.has(model.id)) models.delete(model.id);
      else models.set(model.id, model);
      ($('provider-models') as HTMLTextAreaElement).value = [...models.values()]
        .map((item) => item.name === item.id ? item.id : `${item.id} | ${item.name}`)
        .join('\n');
      renderUpstreamModels();
    };
    picker.appendChild(chip);
  }
}

function fillProviderForm(provider: ProviderConfig) {
  ($('provider-id') as HTMLInputElement).value = provider.id;
  ($('provider-base-url') as HTMLInputElement).value = provider.base_url;
  ($('provider-api-key') as HTMLInputElement).value = provider.api_key;
  ($('provider-api') as HTMLInputElement).value = provider.api || 'openai-completions';
  ($('provider-use-full-path') as HTMLInputElement).checked = provider.use_full_path;
  ($('provider-models') as HTMLTextAreaElement).value = provider.models
    .map((model) => model.name && model.name !== model.id ? `${model.id} | ${model.name}` : model.id)
    .join('\n');
  renderUpstreamModels();
}

function renderProviders(providers: ProviderConfig[]) {
  currentProviders = providers;
  const output = $('providers-output');
  const list = $('provider-list');
  const selectedId = inputValue('provider-id');
  if (providers.length === 0) {
    list.innerHTML = '';
    output.textContent = '暂无供应商。可手动添加，或从 ccswitch/Claude 配置导入。';
    return;
  }
  output.textContent = providers.map((provider) => [
    `${provider.active ? '●' : '○'} ${provider.id}`,
    `  Base URL: ${provider.base_url || '(未配置)'}`,
    `  API: ${provider.api || '(默认)'}`,
    `  API Key: ${maskKey(provider.api_key) || '(未配置)'}`,
    `  Models: ${provider.models.map((model) => model.id).join(', ') || '(未配置)'}`,
  ].join('\n')).join('\n\n');

  const currentId = inputValue('provider-id');
  const selected = providers.find((provider) => provider.id === currentId)
    || providers.find((provider) => provider.active)
    || providers[0];
  if (selected) fillProviderForm(selected);
  list.innerHTML = '';
  for (const provider of providers) {
    const card = document.createElement('button');
    card.type = 'button';
    card.className = `provider-card${provider.active ? ' active' : ''}${provider.id === selected.id ? ' selected' : ''}`;
    const title = document.createElement('div');
    title.className = 'provider-card-title';
    title.textContent = `${provider.active ? '已启用  ' : ''}${provider.id}`;
    const details = document.createElement('div');
    details.className = 'provider-card-meta';
    details.textContent = `${provider.models.map((model) => model.id).join(', ') || '未配置模型'}\n${provider.base_url || '未配置 Base URL'}`;
    card.append(title, details);
    card.onclick = () => {
      fillProviderForm(provider);
      renderProviders(currentProviders);
    };
    list.appendChild(card);
  }
}

async function refreshProviders() {
  const providers = await invoke<ProviderConfig[]>('list_providers');
  renderProviders(providers);
}

function providerFromForm(active = false): ProviderConfig {
  return {
    id: inputValue('provider-id'),
    base_url: inputValue('provider-base-url'),
    api_key: inputValue('provider-api-key'),
    api: inputValue('provider-api') || 'openai-completions',
    models: parseProviderModels(),
    active,
    use_full_path: ($('provider-use-full-path') as HTMLInputElement).checked,
  };
}

function setupProviderActions() {
  $('provider-models').addEventListener('input', renderUpstreamModels);
  $('btn-provider-new').onclick = () => {
    ($('provider-id') as HTMLInputElement).value = '';
    ($('provider-base-url') as HTMLInputElement).value = '';
    ($('provider-api-key') as HTMLInputElement).value = '';
    ($('provider-api') as HTMLInputElement).value = 'openai-completions';
    ($('provider-use-full-path') as HTMLInputElement).checked = false;
    ($('provider-models') as HTMLTextAreaElement).value = '';
    upstreamModels = [];
    upstreamModelsProviderId = '';
    renderUpstreamModels();
    ($('provider-id') as HTMLInputElement).focus();
  };
  $('btn-provider-fetch-models').onclick = () => withBusy('btn-provider-fetch-models', '获取中...', async () => {
    try {
      const provider = providerFromForm(false);
      if (!provider.base_url) {
        log('请先填写 Base URL，再获取模型', 'warn');
        return;
      }
      upstreamModels = await invoke<ProviderModel[]>('fetch_provider_models', { provider });
      upstreamModelsProviderId = provider.id;
      renderUpstreamModels();
      log(`已从上游获取 ${upstreamModels.length} 个模型，点击即可配置`, 'success');
    } catch (error) {
      log(`获取上游模型失败: ${error}`, 'error');
    }
  }, { progress: true });
  $('btn-providers-refresh').onclick = () => withBusy('btn-providers-refresh', '刷新中...', async () => {
    await refreshProviders();
  }, { progress: true });

  $('btn-providers-import').onclick = () => withBusy('btn-providers-import', '导入中...', async () => {
    try {
      const existing = new Set(currentProviders.map((provider) => provider.id));
      const providers = await invoke<ProviderConfig[]>('import_ccswitch_providers');
      renderProviders(providers);
      const added = providers.filter((provider) => !existing.has(provider.id)).length;
      log(`已从 ccswitch/Claude 配置导入，新增 ${added} 个供应商`, 'success');
    } catch (error) {
      log(`导入失败: ${error}`, 'error');
    }
  }, { progress: true });

  $('btn-provider-save').onclick = () => withBusy('btn-provider-save', '保存中...', async () => {
    try {
      const providers = await invoke<ProviderConfig[]>('save_provider', { provider: providerFromForm(false) });
      renderProviders(providers);
      log('供应商已保存并同步到 OpenClaw 配置', 'success');
    } catch (error) {
      log(`保存供应商失败: ${error}`, 'error');
    }
  }, { progress: true });

  $('btn-provider-activate').onclick = () => withBusy('btn-provider-activate', '启用中...', async () => {
    try {
      const providers = await invoke<ProviderConfig[]>('save_provider', { provider: providerFromForm(true) });
      renderProviders(providers);
      log('供应商已启用，建议重启 Gateway 生效', 'success');
    } catch (error) {
      log(`启用供应商失败: ${error}`, 'error');
    }
  }, { progress: true });

  $('btn-provider-delete').onclick = () => {
    const id = inputValue('provider-id');
    if (!id) return log('请输入要删除的供应商 ID', 'warn');
    if (!confirm(`确定删除供应商 "${id}"？会同步删除对应模型别名。`)) return;
    void withBusy('btn-provider-delete', '删除中...', async () => {
      try {
        const providers = await invoke<ProviderConfig[]>('delete_provider', { id });
        renderProviders(providers);
        log('供应商已删除', 'success');
      } catch (error) {
        log(`删除供应商失败: ${error}`, 'error');
      }
    }, { progress: true });
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
setupProviderActions();
setupAgentActions();
setupSkillActions();
setupTaskActions();
setupWindowActions();
void refreshProviders();
void checkEnv();
window.setInterval(checkEnv, 30000);
