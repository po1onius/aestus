import React from "react";
import { createRoot } from "react-dom/client";
import { domAnimation, LazyMotion } from "motion/react";
import "@fontsource-variable/ibm-plex-sans/wght.css";
import "@fontsource-variable/noto-sans-sc/wght.css";
import "@fontsource/ibm-plex-mono/latin-400.css";
import { App } from "./App";
import { themeStorageKey } from "./config";
import "./styles.css";

// 在 React 首次渲染前恢复主题，避免刷新页面时先闪过亮色界面。
const initialTheme = localStorage.getItem(themeStorageKey) === "dark" ? "dark" : "light";
document.documentElement.classList.toggle("dark", initialTheme === "dark");
document.documentElement.style.colorScheme = initialTheme;

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    {/* 动画能力放在应用根部，供页面内下拉框与全局弹窗共同复用同一份轻量特性集。 */}
    <LazyMotion features={domAnimation} strict>
      <App />
    </LazyMotion>
  </React.StrictMode>,
);
