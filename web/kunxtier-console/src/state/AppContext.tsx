/*
 * Description: 会话与当前网络组状态 (会话令牌经 sessionStorage 标签页级续期, 管理数据不落本地)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";
import { api, getToken, setToken } from "@/api/client";
import type { LoginResponse } from "@/api/types";

/** 会话与网络组状态形状。 */
interface AppContextValue {
  /** 会话是否有效 (内存态令牌已写入)。 */
  authed: boolean;
  /** 以 admin 凭证登录, 成功后写入会话令牌。 */
  login: (credential: string) => Promise<void>;
  /** 登出: 尽力吊销服务端会话并清空本地态。 */
  logout: () => Promise<void>;
  /** 当前网络组; null 表示未选择 (列表加载后默认取第一个)。 */
  network: string | null;
  /** 切换当前网络组 (全部页面随之切换)。 */
  setNetwork: (name: string) => void;
}

const AppContext = createContext<AppContextValue | null>(null);

/**
 * AppContext 的具名 Provider 包裹 (规范: 禁止在组件 JSX 中匿名编写 XxxContext.Provider)。
 * 供 AppProvider 挂载会话与网络组状态。
 */
function AppContextProvider({ value, children }: { value: AppContextValue; children: ReactNode }) {
  return <AppContext.Provider value={value}>{children}</AppContext.Provider>;
}

/** 会话与网络组状态提供方 (刷新经 sessionStorage 恢复会话, 服务端过期由 401 兜底)。 */
export function AppProvider({ children }: { children: ReactNode }) {
  const [authed, setAuthed] = useState(() => getToken() !== null);
  const [network, setNetworkState] = useState<string | null>(null);

  const login = useCallback(async (credential: string) => {
    const resp = await api<LoginResponse>("/api/v1/admin/login", {
      json: { credential },
    });
    setToken(resp.token);
    setAuthed(true);
  }, []);

  const logout = useCallback(async () => {
    try {
      await api("/api/v1/admin/logout", { method: "POST" });
    } catch {
      // 会话已过期时服务端返回 401, 本地态照常清空, 不视为登出失败
    }
    setToken(null);
    setAuthed(false);
    setNetworkState(null);
  }, []);

  const value = useMemo<AppContextValue>(
    () => ({ authed, login, logout, network, setNetwork: setNetworkState }),
    [authed, login, logout, network],
  );

  return <AppContextProvider value={value}>{children}</AppContextProvider>;
}

/** 读取会话与网络组状态; 必须在 AppProvider 内使用。 */
export function useApp(): AppContextValue {
  const ctx = useContext(AppContext);
  if (ctx === null) {
    throw new Error("useApp 必须在 AppProvider 内使用");
  }
  return ctx;
}
