/*
 * Description: 中继页 (在线卫星与核心内置转发汇总, FR-5)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import { useRelaysQuery } from "@/api/hooks";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

/** 中继页: 注册长连接在线态 + 每中继连接节点数, 3s 轮询。 */
export default function RelaysPage() {
  const relaysQuery = useRelaysQuery();
  const relays = relaysQuery.data ?? [];

  return (
    <section className="flex flex-col gap-5">
      <div>
        <h1 className="text-lg font-semibold">中继</h1>
        <p className="mt-0.5 text-sm text-muted-foreground">
          卫星断连后列表及时摘除; 节点数来自节点路径上报聚合
        </p>
      </div>

      {relaysQuery.isLoading ? (
        <p className="text-sm text-muted-foreground">中继加载中</p>
      ) : relays.length === 0 ? (
        <p className="text-sm text-muted-foreground">暂无中继条目</p>
      ) : (
        <div
          className={cn(
            "overflow-x-auto rounded-lg", // 布局
            "border border-border", // 分层
          )}
        >
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-border text-left text-xs text-muted-foreground">
                <th className="px-3 py-2 font-medium">类型</th>
                <th className="px-3 py-2 font-medium">地址</th>
                <th className="px-3 py-2 font-medium">证书指纹</th>
                <th className="px-3 py-2 font-medium">连接节点数</th>
              </tr>
            </thead>
            <tbody>
              {relays.map((relay) => (
                <tr key={relay.addr} className="border-b border-border last:border-b-0">
                  <td className="px-3 py-2">
                    <Badge variant={relay.kind === "core" ? "neutral" : "success"}>
                      {relay.kind === "core" ? "核心内置" : "卫星中继"}
                    </Badge>
                  </td>
                  <td className="px-3 py-2 font-mono text-xs">{relay.addr}</td>
                  <td
                    className="px-3 py-2 font-mono text-xs text-muted-foreground"
                    title={relay.fingerprint}
                  >
                    {relay.fingerprint.slice(0, 16)}...
                  </td>
                  <td className="px-3 py-2">{relay.nodes}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
