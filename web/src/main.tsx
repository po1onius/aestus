import React, { lazy, Suspense } from "react";
import { createRoot } from "react-dom/client";
import { domAnimation, LazyMotion } from "motion/react";
import { HomePage } from "./pages/HomePage";
import { themeStorageKey } from "./config";
import "./styles.css";

// 公开首页独立于控制台，避免访问首页时加载管理功能或发起鉴权请求。
const DashboardApp = lazy(() =>
  import("./App").then((module) => ({ default: module.App })),
);
const isHomePage = window.location.pathname === "/";

// 在 React 首次渲染前恢复主题，避免刷新页面时先闪过亮色界面。
const initialTheme =
  !isHomePage && localStorage.getItem(themeStorageKey) === "dark"
    ? "dark"
    : "light";
document.documentElement.classList.toggle("dark", initialTheme === "dark");
document.documentElement.style.colorScheme = initialTheme;

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    {/* 动画能力放在应用根部，供页面内下拉框与全局弹窗共同复用同一份轻量特性集。 */}
    <LazyMotion features={domAnimation} strict>
      {isHomePage ? (
        <HomePage />
      ) : (
        <Suspense
          fallback={
            <div
              className="grid min-h-dvh place-items-center bg-slate-50 text-sm text-slate-500 dark:bg-slate-950"
              role="status"
            >
              正在加载控制台…
            </div>
          }
        >
          <DashboardApp />
        </Suspense>
      )}
    </LazyMotion>
  </React.StrictMode>,
);
