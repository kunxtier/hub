/*
 * Description: 路由装配 (登录守卫与 401 全局跳转)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import { useEffect, type ReactElement } from "react";
import { Navigate, Route, Routes, useNavigate } from "react-router";
import { UNAUTHORIZED_EVENT } from "@/api/client";
import AppLayout from "@/components/AppLayout";
import LoginPage from "@/pages/Login";
import NetworksPage from "@/pages/Networks";
import NodesPage from "@/pages/Nodes";
import OverviewPage from "@/pages/Overview";
import RelaysPage from "@/pages/Relays";
import TokensPage from "@/pages/Tokens";
import { useApp } from "@/state/AppContext";

/** 登录守卫: 未登录访问受保护路由时重定向登录页 (携带来源便于回跳)。 */
function RequireAuth({ children }: { children: ReactElement }) {
  const { authed } = useApp();
  if (!authed) {
    return <Navigate to="/login" replace />;
  }
  return children;
}

/** 路由根: /login 独立, 其余挂 AppLayout (守卫内)。 */
export default function App() {
  const { authed } = useApp();
  const navigate = useNavigate();

  useEffect(() => {
    // 会话过期 (任意请求 401) 统一回到登录页
    const onUnauthorized = () => navigate("/login", { replace: true });
    window.addEventListener(UNAUTHORIZED_EVENT, onUnauthorized);
    return () => window.removeEventListener(UNAUTHORIZED_EVENT, onUnauthorized);
  }, [navigate]);

  return (
    <Routes>
      <Route path="/login" element={authed ? <Navigate to="/" replace /> : <LoginPage />} />
      <Route
        element={
          <RequireAuth>
            <AppLayout />
          </RequireAuth>
        }
      >
        <Route index element={<OverviewPage />} />
        <Route path="nodes" element={<NodesPage />} />
        <Route path="tokens" element={<TokensPage />} />
        <Route path="networks" element={<NetworksPage />} />
        <Route path="relays" element={<RelaysPage />} />
      </Route>
    </Routes>
  );
}
