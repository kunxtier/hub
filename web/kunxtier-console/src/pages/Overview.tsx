/*
 * Description: 总览页 (当前网络节点与路径概况, 3s 轮询)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import { useNetworksQuery, useNodesQuery, useRelaysQuery } from "@/api/hooks";
import type { PathStatus, PeerPathReport } from "@/api/types";
import ErrorHint from "@/components/ErrorHint";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";
import { PATH_LABELS, formatTime, summarizePaths } from "@/lib/summary";
import { useApp } from "@/state/AppContext";

/** 总览页: 状态卡片 + 节点简表, 与 CLI status 口径一致 (FR-1)。 */
export default function OverviewPage() {
  const { network } = useApp();
  const networksQuery = useNetworksQuery();
  const nodesQuery = useNodesQuery(network, "");
  const relaysQuery = useRelaysQuery();

  const nodes = nodesQuery.data ?? [];
  const onlineCount = nodes.filter((node) => node.online).length;
  const paths = summarizePaths(nodes);
  const relays = relaysQuery.data ?? [];
  const satelliteCount = relays.filter((relay) => relay.kind === "satellite").length;
  const cidr = networksQuery.data?.find((item) => item.name === network)?.cidr;

  return (
    <section className="flex flex-col gap-5">
      <div>
        <h1 className="text-lg font-semibold">总览</h1>
        <p className="mt-0.5 text-sm text-muted-foreground">
          {networksQuery.isError
            ? "网络组加载失败"
            : network !== null
              ? `网络组 ${network}${cidr ? ` (${cidr})` : ""}`
              : "加载网络组中"}
        </p>
      </div>

      <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
        <StatTile
          label="在线节点"
          value={nodesQuery.isError ? "加载失败" : `${onlineCount} / ${nodes.length}`}
        />
        <StatTile label="直连路径" value={nodesQuery.isError ? "加载失败" : String(paths.direct)} />
        <StatTile
          label="中继路径"
          value={nodesQuery.isError ? "加载失败" : String(paths.relayed)}
        />
        <StatTile
          label="在线卫星"
          value={relaysQuery.isError ? "加载失败" : `${satelliteCount} 个`}
        />
      </div>

      {nodesQuery.isLoading ? (
        <p className="text-sm text-muted-foreground">节点加载中</p>
      ) : nodesQuery.isError ? (
        <ErrorHint error={nodesQuery.error} onRetry={() => void nodesQuery.refetch()} />
      ) : nodes.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          当前网络暂无节点, 签发令牌后用 kunxtier up 接入
        </p>
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
                <th className="px-3 py-2 font-medium">节点</th>
                <th className="px-3 py-2 font-medium">虚拟 IP</th>
                <th className="px-3 py-2 font-medium">状态</th>
                <th className="px-3 py-2 font-medium">版本</th>
                <th className="px-3 py-2 font-medium">路径概况</th>
                <th className="px-3 py-2 font-medium">接入时间</th>
              </tr>
            </thead>
            <tbody>
              {nodes.map((node) => (
                <tr
                  key={`${node.network}/${node.name}`}
                  className="border-b border-border last:border-b-0"
                >
                  <td className="px-3 py-2 font-medium">{node.name}</td>
                  <td className="px-3 py-2 font-mono text-xs">{node.ip}</td>
                  <td className="px-3 py-2">
                    <Badge variant={node.online ? "success" : "neutral"}>
                      {node.online ? "在线" : "离线"}
                    </Badge>
                  </td>
                  <td className="px-3 py-2 text-muted-foreground">{node.version ?? "-"}</td>
                  <td className="px-3 py-2">
                    {node.paths.length === 0 ? (
                      <span className="text-muted-foreground">-</span>
                    ) : (
                      summarizeNodePaths(node.paths)
                    )}
                  </td>
                  <td className="px-3 py-2 text-muted-foreground">{formatTime(node.created_at)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

/** 单节点路径概况文本 (按词汇计数压缩展示)。 */
function summarizeNodePaths(paths: PeerPathReport[]): string {
  const counts = new Map<PathStatus, number>();
  for (const report of paths) {
    counts.set(report.path, (counts.get(report.path) ?? 0) + 1);
  }
  return [...counts.entries()]
    .map(([path, count]) => `${PATH_LABELS[path] ?? path} ${count}`)
    .join(" / ");
}

/** 状态统计瓦片 (数值 + 标签, 边框分层)。 */
function StatTile({ label, value }: { label: string; value: string }) {
  return (
    <div
      className={cn(
        "rounded-lg p-3", // 布局
        "border border-border", // 分层
      )}
    >
      <p className="text-xs text-muted-foreground">{label}</p>
      <p className="mt-1 text-xl font-semibold">{value}</p>
    </div>
  );
}
