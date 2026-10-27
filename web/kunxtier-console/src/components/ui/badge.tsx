/*
 * Description: 状态徽标基件 (边框 + 语义文字色表达状态, 不做填充装饰)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

/** 徽标变体: neutral 中性 / success 在线与直连 / danger 危险与吊销。 */
const badgeVariants = cva(
  cn(
    "inline-flex items-center gap-1 rounded-sm px-1.5 py-0.5 text-xs font-medium", // 布局字重
    "border", // 边框
  ),
  {
    variants: {
      variant: {
        neutral: "border-border text-muted-foreground",
        success: "border-success/50 text-success",
        danger: "border-destructive/50 text-destructive",
      },
    },
    defaultVariants: {
      variant: "neutral",
    },
  },
);

/** 徽标属性。 */
type BadgeProps = React.ComponentProps<"span"> & VariantProps<typeof badgeVariants>;

/** 状态徽标。 */
function Badge({ className, variant, ...props }: BadgeProps) {
  return (
    <span data-slot="badge" className={cn(badgeVariants({ variant }), className)} {...props} />
  );
}

export { Badge, badgeVariants };
