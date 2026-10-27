/*
 * Description: 节点页 (属主过滤 / 路径明细 / 强制中继策略 / 移除)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import { useEffect, useState } from "react";
import { useNodesQuery, useRemoveNodeMutation, useSetNodePolicyMutation } from "@/api/hooks";
import { errorMessage } from "@/api/client";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { formatTime } from "@/lib/summary";
import { useApp } from "@/state/AppContext";

/** 属主过滤输入防抖间隔。 */
const OWNER_FILTER_DEBOUNCE_MS = 300;

/** 节点页: 当前网络全部节点与管理操作 (FR-3 / FR-4)。 */
export default function NodesPage() {
  const { network } = useApp();
  const [ownerInput, setOwnerInput] = useState("");
  const [owner, setOwner] = useState("");

  useEffect(() => {
    const timer = setTimeout(() => setOwner(ownerInput.trim()), OWNER_FILTER_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [ownerInput]);

  const nodesQuery = useNodesQuery(network, owner);
  const nodes = nodesQuery.data ?? [];
  const policyMutation = useSetNodePolicyMutation();
  const removeMutation = useRemoveNodeMutation();
  const mutationError = policyMutation.error ?? removeMutation.error;

  return (
    <section className="flex flex-col gap-4">
      <div className="flex items-end justify-between gap-4">
        <div>
          <h1 className="text-lg font-semibold">节点</h1>
          <p className="mt-0.5 text-sm text-muted-foreground">
            当前网络组 {network ?? "未选择"} 的全部节点
          </p>
        </div>
        <div className="flex items-center gap-2">
          <label htmlFor="owner-filter" className="text-sm text-muted-foreground">
            属主过滤
          </label>
          <Input
            id="owner-filter"
            className="w-40"
            placeholder="属主名"
            value={ownerInput}
            onChange={(event) => setOwnerInput(event.target.value)}
          />
        </div>
      </div>

      {mutationError !== null && (
        <p className="text-sm text-destructive">{errorMessage(mutationError)}</p>
      )}

      {nodesQuery.isLoading ? (
        <p className="text-sm text-muted-foreground">节点加载中</p>
      ) : nodes.length === 0 ? (
        <p className="text-sm text-muted-foreground">无匹配节点</p>
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
                <th className="px-3 py-2 font-medium">属主</th>
                <th className="px-3 py-2 font-medium">版本</th>
                <th className="px-3 py-2 font-medium">状态</th>
                <th className="px-3 py-2 font-medium">策略</th>
                <th className="px-3 py-2 font-medium">路径 / 候选地址</th>
                <th className="px-3 py-2 font-medium">操作</th>
              </tr>
            </thead>
            <tbody>
              {nodes.map((node) => (
                <tr
                  key={`${node.network}/${node.name}`}
                  className="border-b border-border last:border-b-0 align-top"
                >
                  <td className="px-3 py-2 font-medium">{node.name}</td>
                  <td className="px-3 py-2 font-mono text-xs">{node.ip}</td>
                  <td className="px-3 py-2 text-muted-foreground">{node.owner ?? "-"}</td>
                  <td className="px-3 py-2 text-muted-foreground">{node.version ?? "-"}</td>
                  <td className="px-3 py-2">
                    <Badge variant={node.online ? "success" : "neutral"}>
                      {node.online ? "在线" : "离线"}
                    </Badge>
                  </td>
                  <td className="px-3 py-2">
                    <Badge variant={node.policy === "force_relay" ? "danger" : "neutral"}>
                      {node.policy === "force_relay" ? "强制中继" : "自动"}
                    </Badge>
                  </td>
                  <td className="px-3 py-2 text-xs text-muted-foreground">
                    {pathTooltip(node)}
                    {node.candidates.length > 0 && (
                      <span
                        className="mt-0.5 block max-w-56 truncate"
                        title={node.candidates.join("\n")}
                      >
                        {node.candidates.join(", ")}
                      </span>
                    )}
                  </td>
                  <td className="px-3 py-2">
                    <div className="flex gap-1.5">
                      <Button
                        size="sm"
                        disabled={policyMutation.isPending}
                        onClick={() => togglePolicy(network, node.name, node.policy)}
                      >
                        {node.policy === "force_relay" ? "恢复自动" : "强制中继"}
                      </Button>
                      <Button
                        size="sm"
                        variant="destructive"
                        disabled={removeMutation.isPending}
                        onClick={() => removeNode(network, node.name)}
                      >
                        移除
                      </Button>
                    </div>
                    <p className="mt-1 text-xs text-muted-foreground">
                      {formatTime(node.created_at)} 接入
                    </p>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );

  /** 切换节点路径策略 (FR-10: auto / force_relay 互切)。 */
  function togglePolicy(currentNetwork: string | null, name: string, policy: string) {
    if (currentNetwork === null) {
      return;
    }
    policyMutation.mutate({
      network: currentNetwork,
      name,
      policy: policy === "auto" ? "force_relay" : "auto",
    });
  }

  /** 移除节点 (二次确认; 凭证即吊销并断线)。 */
  function removeNode(currentNetwork: string | null, name: string) {
    if (currentNetwork === null) {
      return;
    }
    if (window.confirm(`移除节点 ${name}? 其凭证立即吊销并断开连接`)) {
      removeMutation.mutate({ network: currentNetwork, name });
    }
  }
}

/** 节点逐对端路径概况 (title 提示承载明细)。 */
function pathTooltip(node: { paths: { peer: string; path: string; relay?: string }[] }) {
  if (node.paths.length === 0) {
    return <span className="block">无路径上报</span>;
  }
  const lines = node.paths.map(
    (report) => `${report.peer} -> ${report.path}${report.relay ? ` (${report.relay})` : ""}`,
  );
  return (
    <span className="block" title={lines.join("\n")}>
      {node.paths.length} 条路径 (悬停查看)
    </span>
  );
}
