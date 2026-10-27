/*
 * Description: 下拉选择触发器 (含展开箭头)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import * as React from "react";
import * as SelectPrimitive from "@radix-ui/react-select";
import { cn } from "@/lib/utils";

/** 下拉选择触发器; 内部放 SelectValue。 */
function SelectTrigger({
  className,
  children,
  ...props
}: React.ComponentProps<typeof SelectPrimitive.Trigger>) {
  return (
    <SelectPrimitive.Trigger
      data-slot="select-trigger"
      className={cn(
        "flex h-8 w-full items-center justify-between gap-2 rounded-md px-2.5 py-1", // 布局尺寸
        "border border-input bg-card text-sm text-foreground", // 边框颜色
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring", // 焦点
        "disabled:cursor-not-allowed disabled:opacity-50", // 禁用
        className,
      )}
      {...props}
    >
      {children}
      <svg
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        className="size-3.5 shrink-0 opacity-60"
        aria-hidden="true"
      >
        <path d="m6 9 6 6 6-6" />
      </svg>
    </SelectPrimitive.Trigger>
  );
}

export { SelectTrigger };
