/*
 * Description: 表单标签基件 (与 Input 尺寸对齐)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import * as React from "react";
import { cn } from "@/lib/utils";

/** 表单标签。 */
function Label({ className, ...props }: React.ComponentProps<"label">) {
  return (
    <label
      data-slot="label"
      className={cn(
        "text-sm font-medium text-foreground", // 字重层级
        className,
      )}
      {...props}
    />
  );
}

export { Label };
