import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./styles.css";
import App from "./App";
import { TrayMenu } from "./pages/TrayMenu";

/*
 * 同一个 index.html 服务两个窗口：
 *   主窗口        → App
 *   托盘菜单窗口  → TrayMenu（URL 带 #tray）
 *
 * 为什么不单独建 tray.html：多一个入口就要多配一份 vite 构建目标、
 * 多维护一份要同步的 HTML 壳，而这个菜单总共几十行。用 hash 区分
 * 是这里最省事也最稳的做法（hash 不会发到服务端，纯前端判断）。
 */
const isTray = typeof location !== "undefined" && location.hash === "#tray";

// 托盘窗口是 transparent 的，body 必须跟着透明，
// 否则窗口矩形会露出一整块白底，圆角外面全是白的（等于没透明）。
// 只给托盘模式加这个类，主窗口的配色一个像素都不动。
if (isTray) {
  document.body.classList.add("tray-mode");
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>{isTray ? <TrayMenu /> : <App />}</StrictMode>,
);
