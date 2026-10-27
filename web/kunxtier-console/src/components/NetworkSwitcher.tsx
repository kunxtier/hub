/*
 * Description: 网络组切换器 (列表加载后默认选中第一个, 切换即全网页面跟随)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import { useEffect } from "react";
import { useNetworksQuery } from "@/api/hooks";
import { Select, SelectValue } from "@/components/ui/select";
import { SelectContent } from "@/components/ui/select-content";
import { SelectItem } from "@/components/ui/select-item";
import { SelectTrigger } from "@/components/ui/select-trigger";
import { useApp } from "@/state/AppContext";

/** 网络组切换器; 加载中与空列表时禁用并提示。 */
export default function NetworkSwitcher() {
  const { network, setNetwork } = useApp();
  const query = useNetworksQuery();
  const networks = query.data ?? [];

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
      <Select value={network ?? undefined} onValueChange={setNetwork}>
        <SelectTrigger className="w-44">
          <SelectValue placeholder={query.isLoading ? "加载中" : "无网络组"} />
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
