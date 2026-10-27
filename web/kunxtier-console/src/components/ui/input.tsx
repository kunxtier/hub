/*
 * Description: 文本输入基件 (表单统一尺寸与焦点样式)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import * as React from "react";
import { cn } from "@/lib/utils";

/** 文本输入框。 */
function Input({ className, type = "text", ...props }: React.ComponentProps<"input">) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        "flex h-8 w-full rounded-md px-2.5 py-1 text-sm", // 布局尺寸
        "border border-input bg-card text-foreground", // 边框颜色
        "placeholder:text-muted-foreground", // 占位
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring", // 焦点
        "disabled:cursor-not-allowed disabled:opacity-50", // 禁用
        className,
      )}
      {...props}
    />
  );
}

export { Input };
