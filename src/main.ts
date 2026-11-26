import { createApp } from "vue";
import App from "./App.vue";
import "./bullChip.css";

// 渲染期错误兜底：把异常画到页面上，避免出现无提示的黑屏。
function showFatalError(title: string, detail: string) {
  const existing = document.getElementById("fatal-error-overlay");
  if (existing) existing.remove();

  const overlay = document.createElement("div");
  overlay.id = "fatal-error-overlay";
  overlay.style.cssText = [
    "position:fixed",
    "inset:0",
    "z-index:99999",
    "overflow:auto",
    "background:#1b2024",
    "color:#ffd7d7",
    "font:13px/1.6 ui-monospace,Consolas,monospace",
    "padding:24px",
    "white-space:pre-wrap",
  ].join(";");

  const heading = document.createElement("strong");
  heading.textContent = title;
  heading.style.cssText = "display:block;color:#ff8f8f;font-size:16px;margin-bottom:12px";
  const body = document.createElement("div");
  body.textContent = detail;

  overlay.append(heading, body);
  document.body.append(overlay);
}

window.addEventListener("error", (event) => {
  showFatalError("运行时错误", `${event.message}\n\n${event.error?.stack ?? ""}`);
});

window.addEventListener("unhandledrejection", (event) => {
  const reason = event.reason;
  showFatalError(
    "未处理的 Promise 异常",
    reason instanceof Error ? `${reason.message}\n\n${reason.stack ?? ""}` : String(reason),
  );
});

// 键盘：放行 F12/开发者工具，方便排查黑屏。
window.addEventListener(
  "keydown",
  (event) => {
    if (event.key === "F12" || (event.ctrlKey && event.shiftKey && event.key.toLowerCase() === "i")) {
      event.stopImmediatePropagation();
    }
  },
  { capture: true },
);

const FUNCTION_KEY_PATTERN = /^F([1-9]|1[0-2])$/;

function shouldBlockKey(event: KeyboardEvent) {
  if (FUNCTION_KEY_PATTERN.test(event.key)) return true;
  if (event.key === "ContextMenu") return true;

  const key = event.key.toLowerCase();
  const withCtrlOrMeta = event.ctrlKey || event.metaKey;

  return (
    key === "browserback" ||
    key === "browserforward" ||
    (withCtrlOrMeta && key === "r") ||
    (withCtrlOrMeta && key === "u") ||
    (withCtrlOrMeta && event.shiftKey && ["c", "i", "j"].includes(key))
  );
}

window.addEventListener(
  "contextmenu",
  (event) => {
    event.preventDefault();
  },
  { capture: true },
);

window.addEventListener(
  "keydown",
  (event) => {
    if (!shouldBlockKey(event)) return;
    event.preventDefault();
    event.stopPropagation();
  },
  { capture: true },
);

createApp(App).mount("#app");
