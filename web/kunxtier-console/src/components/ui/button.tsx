/*
 * Description: 按钮基件 (CVA 变体; 语义色仅用于主操作与破坏性操作)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import * as React from "react";
import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

/** 按钮变体定义 (结构化风格: 默认 outline, 主操作 / 破坏性才上语义色)。 */
const buttonVariants = cva(
  cn(
    "inline-flex items-center justify-center gap-1.5 whitespace-nowrap rounded-md text-sm font-medium", // 布局
    "border border-transparent", // 边框
    "transition-colors", // 过渡
    "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background", // 焦点
    "disabled:pointer-events-none disabled:opacity-50", // 禁用
  ),
  {
    variants: {
      variant: {
        primary: "bg-primary text-primary-foreground hover:bg-primary/90",
        outline:
          "border-border bg-card text-foreground hover:bg-accent hover:text-accent-foreground",
        ghost: "text-foreground hover:bg-accent hover:text-accent-foreground",
        destructive:
          "border-destructive/40 bg-card text-destructive hover:bg-destructive hover:text-destructive-foreground",
      },
      size: {
        default: "h-8 px-3",
        sm: "h-7 px-2 text-xs",
        icon: "size-8 p-0",
      },
    },
    defaultVariants: {
      variant: "outline",
      size: "default",
    },
  },
);

/** 按钮属性 (标准 button 属性 + 变体)。 */
type ButtonProps = React.ComponentProps<"button"> &
  VariantProps<typeof buttonVariants> & {
    /** 以子元素承载按钮语义与样式 (NavLink 等场景)。 */
    asChild?: boolean;
  };

/** 通用按钮。 */
function Button({ className, variant, size, asChild = false, ...props }: ButtonProps) {
  const Comp = asChild ? Slot : "button";
  return (
    <Comp
      data-slot="button"
      className={cn(buttonVariants({ variant, size }), className)}
      {...props}
    />
  );
}

export { Button, buttonVariants };
