/*
 * Description: 控制台数据查询与变更 hooks (列表页 3s 轮询, 页签隐藏自动暂停)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "./client";
import type { IssueTokenResponse, NetworkInfo, NodeInfo, PathPolicy, RelayInfo } from "./types";

/** 列表页轮询间隔 (FR-1 秒级可见; 后台轮询缺省关闭即页签隐藏时暂停)。 */
const POLL_INTERVAL_MS = 3000;

/** 令牌签发请求体 (POST /api/admin/tokens)。 */
interface IssueTokenInput {
  network: string;
  kind: "node" | "relay";
  /** node 令牌必填且用户须存在; relay 令牌不绑定用户。 */
  user?: string;
  ttlSecs: number;
}

/** 节点策略设置请求体 (POST /api/admin/nodes/policy)。 */
interface SetPolicyInput {
  network: string;
  name: string;
  policy: PathPolicy;
}

/** 要求网络组已选择 (供 queryFn 收口非空)。 */
function requireNetwork(network: string | null): string {
  if (network === null) {
    throw new Error("未选择网络组");
  }
  return network;
}

/** 网络组列表 (登录后加载, 不轮询)。 */
export function useNetworksQuery() {
  return useQuery({
    queryKey: ["networks"],
    queryFn: () => api<NetworkInfo[]>("/api/admin/networks"),
  });
}

/** 当前网络节点列表; owner 非空时按属主过滤 (FR-3)。 */
export function useNodesQuery(network: string | null, owner: string) {
  return useQuery({
    queryKey: ["nodes", network, owner],
    queryFn: () => {
      const net = requireNetwork(network);
      const ownerQuery = owner ? `&owner=${encodeURIComponent(owner)}` : "";
      return api<NodeInfo[]>(`/api/admin/nodes?network=${encodeURIComponent(net)}${ownerQuery}`);
    },
    enabled: network !== null,
    refetchInterval: POLL_INTERVAL_MS,
  });
}

/** 中继汇总 (在线卫星 + 内置条目)。 */
export function useRelaysQuery() {
  return useQuery({
    queryKey: ["relays"],
    queryFn: () => api<RelayInfo[]>("/api/admin/relays"),
    refetchInterval: POLL_INTERVAL_MS,
  });
}

/** 创建网络组; 成功后失效网络组列表。 */
export function useCreateNetworkMutation() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: { name: string; cidr?: string }) =>
      api<NetworkInfo>("/api/admin/networks", {
        json: { name: input.name, cidr: input.cidr || undefined },
      }),
    onSuccess: () => client.invalidateQueries({ queryKey: ["networks"] }),
  });
}

/** 设置节点路径策略 (FR-4 / FR-10); 成功后失效节点列表。 */
export function useSetNodePolicyMutation() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: SetPolicyInput) =>
      api<{ ok: boolean }>("/api/admin/nodes/policy", { json: input }),
    onSuccess: () => client.invalidateQueries({ queryKey: ["nodes"] }),
  });
}

/** 移除节点 (凭证即吊销); 成功后失效节点列表。 */
export function useRemoveNodeMutation() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: { network: string; name: string }) =>
      api<{ ok: boolean }>("/api/admin/nodes/remove", { json: input }),
    onSuccess: () => client.invalidateQueries({ queryKey: ["nodes"] }),
  });
}

/** 签发令牌 (一次性 node / 可复用 relay); 明文仅结果展示一次。 */
export function useIssueTokenMutation() {
  return useMutation({
    mutationFn: (input: IssueTokenInput) =>
      api<IssueTokenResponse>("/api/admin/tokens", {
        json: {
          network: input.network,
          kind: input.kind,
          user: input.user || undefined,
          ttl_secs: input.ttlSecs,
        },
      }),
  });
}

/** 撤销令牌 (按签发时返回的 token_id; relay 令牌撤销即卫星踢线)。 */
export function useRevokeTokenMutation() {
  return useMutation({
    mutationFn: (tokenId: number) =>
      api<{ ok: boolean }>(`/api/admin/tokens/${tokenId}/revoke`, {
        method: "POST",
      }),
  });
}

/** 创建用户 (属主标注载体, 令牌签发的前置)。 */
export function useCreateUserMutation() {
  return useMutation({
    mutationFn: (input: { network: string; name: string }) =>
      api<{ id: number }>("/api/admin/users", { json: input }),
  });
}

/** 禁用用户 (名下节点凭证即吊销、令牌失效)。 */
export function useDisableUserMutation() {
  return useMutation({
    mutationFn: (input: { network: string; name: string }) =>
      api<{ ok: boolean }>("/api/admin/users/disable", { json: input }),
  });
}
