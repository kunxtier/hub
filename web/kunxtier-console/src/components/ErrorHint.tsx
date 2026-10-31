/*
 * Description: 查询失败提示 (查询页 error 态统一呈现, 与加载/空态文案可区分)
 *
 * Author: qinzhenya
 * Created: 2026-10-31
 */

import { ApiRequestError, errorMessage } from "@/api/client";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

interface ErrorHintProps {
  /** 查询抛出的错误 (ApiRequestError 带服务端文案, 其余按网络异常提示)。 */
  error: unknown;
  /** 重试回调 (通常传 query.refetch); 缺省不展示重试按钮。 */
  onRetry?: () => void;
}

/** 查询失败提示: destructive 文案 + 可选重试, 避免失败被空态掩盖。 */
export default function ErrorHint({ error, onRetry }: ErrorHintProps) {
  return (
    <div
      className={cn(
        "flex items-center justify-between gap-3 rounded-lg px-4 py-3", // 布局
        "border border-destructive/40", // 分层
      )}
    >
      <p className="text-sm text-destructive">
        {error instanceof ApiRequestError
          ? `加载失败: ${errorMessage(error)}`
          : "加载失败, 请检查网络后重试"}
      </p>
      {onRetry !== undefined && (
        <Button size="sm" variant="outline" onClick={onRetry}>
          重试
        </Button>
      )}
    </div>
  );
}
