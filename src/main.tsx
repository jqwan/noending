import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles/tokens.css";
import "./styles/global.css";
import "./styles/layout.css";
import "./styles/components.css";

// Appearance 设置持久化在 localStorage；启动时先恢复，避免闪烁。
const theme = localStorage.getItem("noending.theme");
if (theme === "light" || theme === "dark") {
  document.documentElement.dataset.theme = theme;
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
