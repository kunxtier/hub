/*
 * Description: 网络组页 (列出与创建网络组, FR-8)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import { useState, type FormEvent } from "react";
import { errorMessage } from "@/api/client";
import { useCreateNetworkMutation, useNetworksQuery } from "@/api/hooks";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { cn } from "@/lib/utils";
import { formatTime } from "@/lib/summary";
import { useApp } from "@/state/AppContext";

/** 网络组页: 各网络数据完全独立呈现, 创建后经顶部切换器切换。 */
export default function NetworksPage() {
  const { network, setNetwork } = useApp();
  const networksQuery = useNetworksQuery();
  const createMutation = useCreateNetworkMutation();
  const [name, setName] = useState("");
  const [cidr, setCidr] = useState("");

  const networks = networksQuery.data ?? [];

  return (
    <section className="flex flex-col gap-5">
      <div>
        <h1 className="text-lg font-semibold">网络组</h1>
        <p className="mt-0.5 text-sm text-muted-foreground">
          网络组间完全隔离, 互不可见; 创建后经顶部切换器切换
        </p>
      </div>

      {createMutation.error !== null && (
        <p className="text-sm text-destructive">{errorMessage(createMutation.error)}</p>
      )}

      <form
        className={cn(
          "flex flex-wrap items-end gap-3 rounded-lg p-4", // 布局
          "border border-border", // 分层
        )}
        onSubmit={(event) => submitCreate(event)}
      >
        <div className="flex w-44 flex-col gap-1">
          <Label htmlFor="network-name">名称</Label>
          <Input
            id="network-name"
            placeholder="如 family"
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </div>
        <div className="flex w-52 flex-col gap-1">
          <Label htmlFor="network-cidr">地址段 (可选)</Label>
          <Input
            id="network-cidr"
            placeholder="缺省自动分配下一 /16"
            value={cidr}
            onChange={(event) => setCidr(event.target.value)}
          />
        </div>
        <Button type="submit" variant="primary" disabled={createMutation.isPending || !name.trim()}>
          创建
        </Button>
      </form>

      {networksQuery.isLoading ? (
        <p className="text-sm text-muted-foreground">网络组加载中</p>
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
                <th className="px-3 py-2 font-medium">名称</th>
                <th className="px-3 py-2 font-medium">地址段</th>
                <th className="px-3 py-2 font-medium">创建时间</th>
                <th className="px-3 py-2 font-medium">当前</th>
              </tr>
            </thead>
            <tbody>
              {networks.map((item) => (
                <tr key={item.name} className="border-b border-border last:border-b-0">
                  <td className="px-3 py-2 font-medium">{item.name}</td>
                  <td className="px-3 py-2 font-mono text-xs">{item.cidr}</td>
                  <td className="px-3 py-2 text-muted-foreground">{formatTime(item.created_at)}</td>
                  <td className="px-3 py-2">
                    {item.name === network ? (
                      <Badge>当前网络</Badge>
                    ) : (
                      <Button size="sm" variant="ghost" onClick={() => setNetwork(item.name)}>
                        切换到此网络
                      </Button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );

  /** 提交创建网络组; 成功后直接切换到新网络。 */
  function submitCreate(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) {
      return;
    }
    createMutation.mutate(
      { name: trimmed, cidr: cidr.trim() || undefined },
      { onSuccess: (created) => setNetwork(created.name) },
    );
  }
}
