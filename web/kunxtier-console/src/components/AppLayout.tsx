/*
 * Description: 受保护页面共用布局 (侧边导航 + 顶部网络切换器与登出)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import { NavLink, Outlet } from "react-router";
import { cn } from "@/lib/utils";
import NetworkSwitcher from "@/components/NetworkSwitcher";
import { Button } from "@/components/ui/button";
import { useApp } from "@/state/AppContext";

/** 侧边导航项 (页面结构与需求 feat-kunxtier-console 六块对齐)。 */
const NAV_ITEMS = [
  { to: "/", label: "总览" },
  { to: "/nodes", label: "节点" },
  { to: "/tokens", label: "令牌与用户" },
  { to: "/networks", label: "网络组" },
  { to: "/relays", label: "中继" },
] as const;

/** 受保护页面布局; 子路由经 Outlet 呈现。 */
export default function AppLayout() {
  const { logout, network } = useApp();

  return (
    <div className="flex min-h-screen">
      <aside
        className={cn(
          "flex w-44 shrink-0 flex-col gap-1 p-3", // 布局
          "border-r border-border", // 分层
        )}
      >
        <p className="mb-3 px-2 text-base font-semibold">kunxtier 控制台</p>
        <nav className="flex flex-col gap-0.5">
          {NAV_ITEMS.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              end={item.to === "/"}
              className={({ isActive }) =>
                cn(
                  "rounded-sm px-2 py-1.5 text-sm", // 布局
                  "border-l-2 border-transparent text-muted-foreground", // 默认态
                  "hover:text-foreground", // 悬停
                  isActive && "border-l-2 border-foreground font-medium text-foreground", // 激活: 边框 + 字重
                )
              }
            >
              {item.label}
            </NavLink>
          ))}
        </nav>
      </aside>
      <div className="flex min-w-0 flex-1 flex-col">
        <header
          className={cn(
            "flex h-12 items-center justify-between px-4", // 布局
            "border-b border-border", // 分层
          )}
        >
          <NetworkSwitcher />
          <div className="flex items-center gap-2">
            <span className="text-xs text-muted-foreground">
              {network ? `当前网络: ${network}` : "未选择网络组"}
            </span>
            <Button variant="ghost" size="sm" onClick={() => void logout()}>
              登出
            </Button>
          </div>
        </header>
        <main className="flex-1 p-5">
          <Outlet />
        </main>
      </div>
    </div>
  );
}
