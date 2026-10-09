import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type HookStatus =
  | { kind: "starting" | "needs_permission" | "running" }
  | { kind: "unsupported" | "failed"; detail: string };

interface State {
  version: string;
  mac: boolean;
  hotkey: { label: string; usage: string; is_default: boolean };
  mode: "toggle" | "push_to_talk";
  recording_hotkey: boolean;
  hook: HookStatus;
  restore_clipboard: boolean;
  organize: boolean;
  has_qwen_key: boolean;
  has_deepseek_key: boolean;
  deepseek_prompt: string;
  default_prompt: string;
  autostart: boolean;
  accessibility: boolean;
  microphone: "granted" | "denied" | "not_determined" | "unknown";
  config_path: string;
  update: { kind: "idle" | "checking" } | { kind: "found" | "downloading"; version: string };
}

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;

let qwenDirty = false;
let deepseekDirty = false;

async function refresh() {
  let s: State;
  try {
    s = await invoke<State>("get_state");
  } catch (e) {
    console.error(e);
    return;
  }

  if (!qwenDirty) {
    $<HTMLInputElement>("qwen-key").placeholder = s.has_qwen_key
      ? "已保存（留空保持不变）"
      : "sk-…";
  }
  $<HTMLInputElement>("organize").checked = s.organize;
  $("deepseek").hidden = !s.organize;
  if (!deepseekDirty) {
    $<HTMLInputElement>("deepseek-key").placeholder = s.has_deepseek_key
      ? "已保存（留空保持不变）"
      : "sk-…";
    const prompt = $<HTMLTextAreaElement>("deepseek-prompt");
    prompt.value = s.deepseek_prompt;
    prompt.placeholder = s.default_prompt;
  }

  $("hotkey-label").textContent = s.hotkey.label;
  $("hotkey-usage").textContent = s.hotkey.usage;
  $("recording").hidden = !s.recording_hotkey;
  $<HTMLButtonElement>("record").disabled = s.recording_hotkey || s.hook.kind !== "running";
  $<HTMLButtonElement>("reset-hotkey").hidden = s.hotkey.is_default;
  for (const r of document.querySelectorAll<HTMLInputElement>('input[name="mode"]')) {
    r.checked = r.value === s.mode;
  }
  const warning = hookWarning(s.hook);
  $("hook-warning").hidden = !warning;
  $("hook-warning").textContent = warning;

  status($("ax-status"), s.accessibility, s.accessibility ? "已授权" : "未授权");
  $("ax-open").hidden = s.accessibility;
  const mic = {
    granted: [true, "已授权"],
    denied: [false, "已拒绝"],
    not_determined: [null, "首次录音时会询问"],
    unknown: [null, "—"],
  } as const;
  const [micOk, micText] = mic[s.microphone];
  status($("mic-status"), micOk, micText);
  $("mic-open").hidden = s.microphone === "granted";
  if (!s.mac) {
    for (const id of ["ax-open", "mic-open"]) $(id).hidden = true;
  }

  $<HTMLInputElement>("autostart").checked = s.autostart;
  $<HTMLInputElement>("restore").checked = s.restore_clipboard;
  $("config-path").textContent = s.config_path;
  const checkUpdate = $<HTMLButtonElement>("check-update");
  checkUpdate.disabled = s.update.kind !== "idle";
  checkUpdate.textContent = updateLabel(s.update);
  $("version").textContent = `Moli Whisper ${s.version}`;
}

function updateLabel(u: State["update"]): string {
  switch (u.kind) {
    case "checking":
      return "正在检查…";
    case "found":
      return `发现 ${u.version}`;
    case "downloading":
      return `正在下载 ${u.version}…`;
    default:
      return "检查更新…";
  }
}

function hookWarning(h: HookStatus): string {
  switch (h.kind) {
    case "needs_permission":
      return "热键需要辅助功能权限。请在系统设置 → 隐私与安全性 → 辅助功能中打开 Moli Whisper，授权后立即生效。";
    case "unsupported":
    case "failed":
      return h.detail;
    default:
      return "";
  }
}

function status(el: HTMLElement, ok: boolean | null, text: string) {
  el.textContent = text;
  el.classList.toggle("ok", ok === true);
  el.classList.toggle("bad", ok === false);
}

async function call(cmd: string, args?: Record<string, unknown>) {
  try {
    await invoke(cmd, args);
    showError("");
  } catch (e) {
    showError(String(e));
  }
  await refresh();
}

function showError(text: string) {
  $("error").hidden = !text;
  $("error").textContent = text;
}

$<HTMLInputElement>("organize").onchange = (e) =>
  call("set_organize", { enabled: (e.target as HTMLInputElement).checked });

function message(id: string, text: string, ok: boolean | null) {
  const el = $(id);
  el.textContent = text;
  el.classList.toggle("ok", ok === true);
  el.classList.toggle("bad", ok === false);
}

// The key is sent only when something was typed; otherwise the saved one stays.
function secret(id: string): string | null {
  const v = $<HTMLInputElement>(id).value;
  return v === "" ? null : v;
}

for (const id of ["qwen-key"]) {
  $(id).oninput = () => {
    qwenDirty = true;
    message("qwen-msg", "未保存", null);
  };
}
$("qwen-test").onclick = async () => {
  message("qwen-msg", "正在连接…", null);
  try {
    await invoke("set_qwen", { apiKey: secret("qwen-key") });
    qwenDirty = false;
    $<HTMLInputElement>("qwen-key").value = "";
    message("qwen-msg", await invoke<string>("test_qwen"), true);
  } catch (e) {
    message("qwen-msg", String(e), false);
  }
  await refresh();
};

for (const id of ["deepseek-key", "deepseek-prompt"]) {
  $(id).oninput = () => {
    deepseekDirty = true;
    message("deepseek-msg", "未保存", null);
  };
}
$("deepseek-test").onclick = async () => {
  message("deepseek-msg", "正在测试…", null);
  try {
    await invoke("set_deepseek", {
      apiKey: secret("deepseek-key"),
      prompt: $<HTMLTextAreaElement>("deepseek-prompt").value,
    });
    deepseekDirty = false;
    $<HTMLInputElement>("deepseek-key").value = "";
    message("deepseek-msg", await invoke<string>("test_deepseek"), true);
  } catch (e) {
    message("deepseek-msg", String(e), false);
  }
  await refresh();
};
$("record").onclick = () => call("record_hotkey");
$("cancel-record").onclick = () => call("cancel_record_hotkey");
$("reset-hotkey").onclick = () => call("reset_hotkey");
for (const r of document.querySelectorAll<HTMLInputElement>('input[name="mode"]')) {
  r.onchange = () => r.checked && call("set_mode", { mode: r.value });
}
$("ax-open").onclick = () => call("open_accessibility_settings");
$("mic-open").onclick = () => call("open_microphone_settings");
$<HTMLInputElement>("autostart").onchange = (e) =>
  call("set_autostart", { enabled: (e.target as HTMLInputElement).checked });
$("check-update").onclick = () => call("check_for_updates");
$<HTMLInputElement>("restore").onchange = (e) =>
  call("set_restore_clipboard", { enabled: (e.target as HTMLInputElement).checked });

listen("settings-changed", refresh);
listen("hotkey-recorded", refresh);
// Permissions change in System Settings; look again when the user comes back.
window.addEventListener("focus", refresh);
refresh();
