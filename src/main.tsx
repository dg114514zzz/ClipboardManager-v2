import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { attachConsole } from "@tauri-apps/plugin-log";

// 把 webview 的 console 转发到后端日志（logs\clipboard.log），便于排查前端错误（文档 4.3）
attachConsole().catch(() => {});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
// 触发整页刷新，使布局测量生效
