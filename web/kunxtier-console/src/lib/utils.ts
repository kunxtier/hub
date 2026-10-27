/*
 * Description: 样式类名合并工具 (shadcn 生态约定入口, components.json 别名指向此处)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

/** 合并条件类名并消解 Tailwind 冲突。 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
