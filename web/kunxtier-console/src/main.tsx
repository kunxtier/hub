/*
 * Description: 应用入口 (Provider 装配: QueryClient / Router / 会话状态)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router";
import App from "@/App";
import { ErrorBoundary } from "@/components/ErrorBoundary";
import { AppProvider } from "@/state/AppContext";
import "@/index.css";

/** 全局查询客户端: 轮询由各 hook 自带间隔驱动, 窗口聚焦不自动刷新。 */
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 1,
      refetchOnWindowFocus: false,
    },
  },
});

const rootElement = document.getElementById("root");
if (rootElement === null) {
  throw new Error("缺少 #root 挂载点");
}

createRoot(rootElement).render(
  <StrictMode>
    <ErrorBoundary>
      <QueryClientProvider client={queryClient}>
        <BrowserRouter>
          <AppProvider>
            <App />
          </AppProvider>
        </BrowserRouter>
      </QueryClientProvider>
    </ErrorBoundary>
  </StrictMode>,
);
