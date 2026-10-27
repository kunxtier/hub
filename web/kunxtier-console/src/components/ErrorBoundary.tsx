/*
 * Description: 根级错误边界 (渲染异常兜底, 防白屏)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import { Component, type ErrorInfo, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/** 错误边界属性。 */
interface ErrorBoundaryProps {
  children: ReactNode;
}

/** 错误边界状态: 捕获到的渲染异常。 */
interface ErrorBoundaryState {
  error: Error | null;
}

/** 根级错误边界: 子树渲染异常时展示兜底界面而非白屏。 */
export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { error: null };

  /** 渲染异常时更新兜底状态 (React 约定入口)。 */
  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { error };
  }

  /** 异常留痕到控制台, 不吞错误 (便于排障)。 */
  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error("渲染异常", error, info.componentStack);
  }

  render(): ReactNode {
    if (this.state.error === null) {
      return this.props.children;
    }
    return (
      <div className="flex min-h-screen items-center justify-center p-4">
        <div
          className={cn(
            "flex w-full max-w-md flex-col gap-3 rounded-lg p-6", // 布局
            "border border-border bg-card", // 分层
          )}
        >
          <h1 className="text-lg font-semibold">页面渲染异常</h1>
          <p className="break-all text-sm text-muted-foreground">
            {this.state.error.message || "未知错误"}
          </p>
          <div>
            <Button onClick={() => window.location.reload()}>重新加载</Button>
          </div>
        </div>
      </div>
    );
  }
}
