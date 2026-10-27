/*
 * Description: 下拉选择根组件 (Radix Select 封装家族入口)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import * as React from "react";
import * as SelectPrimitive from "@radix-ui/react-select";

/**
 * 下拉选择根 (受控 value / onValueChange 语义与原生 select 一致)。
 * 家族成员: SelectTrigger / SelectContent / SelectItem (各自独立文件)。
 */
function Select(props: React.ComponentProps<typeof SelectPrimitive.Root>) {
  return <SelectPrimitive.Root data-slot="select" {...props} />;
}

/** 选中值展示 (放 Trigger 内)。 */
export const SelectValue = SelectPrimitive.Value;

export { Select };
