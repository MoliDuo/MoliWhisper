import { listen } from "@tauri-apps/api/event";

type Msg =
  | { kind: "reset" }
  | { kind: "hide" }
  | { kind: "state"; state: "connecting" | "recording" | "finalizing" | "delivering" }
  | { kind: "level"; value: number }
  | { kind: "text"; text: string }
  | { kind: "message"; text: string; error: boolean };

/** Only say "connecting" when it takes long enough to notice. */
const CONNECTING_LABEL_AFTER_MS = 400;

const pill = document.getElementById("pill")!;
const halo = document.getElementById("halo")!;
const box = document.getElementById("box")!;
const text = document.getElementById("text")!;

let state = "idle";
let transcript = "";
let connectingTimer: number | undefined;
let level = 0;

function render() {
  pill.dataset.state = state;
  let shown = transcript;
  let hint = false;
  if (!shown) {
    hint = true;
    if (state === "recording") shown = "正在听…";
    else if (state === "finalizing" || state === "delivering") shown = "识别中…";
  }
  if (state === "message") hint = false;
  text.textContent = shown;
  text.classList.toggle("hint", hint);
  requestAnimationFrame(() => {
    box.classList.toggle("clipped", text.offsetWidth > box.clientWidth);
  });
}

function setLevel(value: number) {
  // Fast attack, slow release, so the halo reads as a pulse and not flicker.
  level = value > level ? value : level * 0.7 + value * 0.3;
  const on = state === "recording" ? level : 0;
  halo.style.opacity = String(Math.min(0.45, on * 0.6));
  halo.style.transform = `scale(${0.6 + on * 0.9})`;
}

function onMessage(msg: Msg) {
  switch (msg.kind) {
    case "reset":
      transcript = "";
      level = 0;
      pill.classList.remove("error");
      pill.classList.add("shown");
      break;
    case "hide":
      clearTimeout(connectingTimer);
      state = "idle";
      transcript = "";
      pill.classList.remove("shown", "error");
      setLevel(0);
      break;
    case "state":
      clearTimeout(connectingTimer);
      state = msg.state;
      if (state === "connecting") {
        connectingTimer = window.setTimeout(() => {
          if (state === "connecting" && !transcript) {
            text.textContent = "连接中…";
            text.classList.add("hint");
          }
        }, CONNECTING_LABEL_AFTER_MS);
      }
      if (state !== "recording") setLevel(0);
      break;
    case "level":
      setLevel(msg.value);
      return;
    case "text":
      transcript = msg.text;
      break;
    case "message":
      clearTimeout(connectingTimer);
      state = "message";
      transcript = msg.text;
      pill.classList.toggle("error", msg.error);
      pill.classList.add("shown");
      setLevel(0);
      break;
  }
  render();
}

listen<Msg>("overlay", (e) => onMessage(e.payload));
render();
