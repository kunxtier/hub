/*
 * Description: 下拉选择浮层 (Portal 挂载, 边框卡片样式)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import * as React from "react";
import * as SelectPrimitive from "@radix-ui/react-select";
import { cn } from "@/lib/utils";

/** 下拉浮层; 内部放 SelectItem 列表。 */
function SelectContent({
  className,
  children,
  position = "popper",
  ...props
}: React.ComponentProps<typeof SelectPrimitive.Content>) {
  return (
    <SelectPrimitive.Portal>
      <SelectPrimitive.Content
        data-slot="select-content"
        position={position}
        className={cn(
          "z-50 min-w-(--radix-select-trigger-width) overflow-hidden rounded-md", // 布局
          "border border-border bg-card text-foreground shadow-md", // 边框颜色
          className,
        )}
        {...props}
      >
        <SelectPrimitive.Viewport className="p-1">{children}</SelectPrimitive.Viewport>
      </SelectPrimitive.Content>
    </SelectPrimitive.Portal>
  );
}

export { SelectContent };
