/*
 * Description: 网络组切换器 (列表加载后默认选中第一个, 切换即全网页面跟随)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import { useEffect } from "react";
import { useNetworksQuery } from "@/api/hooks";
import { Select } from "@/components/ui/select";
import { SelectValue } from "@/components/ui/select-value";
import { SelectContent } from "@/components/ui/select-content";
import { SelectItem } from "@/components/ui/select-item";
import { SelectTrigger } from "@/components/ui/select-trigger";
import { useApp } from "@/state/AppContext";

/** 网络组切换器; 加载中 / 加载失败 / 空列表时禁用, placeholder 提示对应状态。 */
export default function NetworkSwitcher() {
  const { network, setNetwork } = useApp();
  const query = useNetworksQuery();
  const networks = query.data ?? [];
  /** 无可选项 (未就绪 / 失败 / 空) 时禁用, 避免点开空下拉。 */
  const disabled = query.isLoading || query.isError || networks.length === 0;

  useEffect(() => {
    // 无选择或所选网络已不存在时回落到第一个网络组
    if (!networks.some((item) => item.name === network)) {
      const first = networks.at(0)?.name;
      if (first !== undefined) {
        setNetwork(first);
      }
    }
  }, [network, networks, setNetwork]);

  return (
    <div className="flex items-center gap-2">
      <span className="text-sm text-muted-foreground">网络组</span>
      <Select value={network ?? undefined} onValueChange={setNetwork} disabled={disabled}>
        <SelectTrigger className="w-44">
          <SelectValue
            placeholder={query.isError ? "加载失败" : query.isLoading ? "加载中" : "无网络组"}
          />
        </SelectTrigger>
        <SelectContent>
          {networks.map((item) => (
            <SelectItem key={item.name} value={item.name}>
              {item.name}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}
