/*
 * Description: 令牌与用户页 (签发 / 撤销 / 用户创建与禁用; 明文仅展示一次)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import { useState, type FormEvent } from "react";
import { errorMessage } from "@/api/client";
import {
  useCreateUserMutation,
  useDisableUserMutation,
  useIssueTokenMutation,
  useRevokeTokenMutation,
} from "@/api/hooks";
import type { IssueTokenResponse } from "@/api/types";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectValue } from "@/components/ui/select";
import { SelectContent } from "@/components/ui/select-content";
import { SelectItem } from "@/components/ui/select-item";
import { SelectTrigger } from "@/components/ui/select-trigger";
import { cn } from "@/lib/utils";
import { useApp } from "@/state/AppContext";

/** 令牌有效期选项。 */
const TTL_OPTIONS = [
  { value: "3600", label: "1 小时" },
  { value: "43200", label: "12 小时" },
  { value: "86400", label: "1 天" },
  { value: "604800", label: "7 天" },
] as const;

/** 令牌与用户页: 管理链路操作集中地 (FR-2)。 */
export default function TokensPage() {
  const { network } = useApp();
  const issueMutation = useIssueTokenMutation();
  const revokeMutation = useRevokeTokenMutation();
  const createUserMutation = useCreateUserMutation();
  const disableUserMutation = useDisableUserMutation();

  const [kind, setKind] = useState<"node" | "relay">("node");
  const [user, setUser] = useState("");
  const [ttlSecs, setTtlSecs] = useState("86400");
  const [result, setResult] = useState<IssueTokenResponse | null>(null);
  const [copied, setCopied] = useState(false);
  const [revokeId, setRevokeId] = useState("");
  const [newUser, setNewUser] = useState("");
  const [disableUser, setDisableUser] = useState("");

  const anyError =
    issueMutation.error ??
    revokeMutation.error ??
    createUserMutation.error ??
    disableUserMutation.error;

  return (
    <section className="flex max-w-2xl flex-col gap-5">
      <div>
        <h1 className="text-lg font-semibold">令牌与用户</h1>
        <p className="mt-0.5 text-sm text-muted-foreground">
          网络组 {network ?? "未选择"}: 令牌即授权, 无审批流
        </p>
      </div>

      {anyError !== null && <p className="text-sm text-destructive">{errorMessage(anyError)}</p>}

      <form
        className={cn(
          "flex flex-col gap-3 rounded-lg p-4", // 布局
          "border border-border", // 分层
        )}
        onSubmit={(event) => {
          event.preventDefault();
          if (network !== null) {
            issueMutation.mutate(
              {
                network,
                kind,
                user: kind === "node" ? user.trim() : undefined,
                ttlSecs: Number(ttlSecs),
              },
              { onSuccess: setResult },
            );
            setCopied(false);
          }
        }}
      >
        <h2 className="font-medium">签发令牌</h2>
        <div className="flex flex-wrap items-end gap-3">
          <div className="flex w-36 flex-col gap-1">
            <Label>类型</Label>
            <Select
              value={kind}
              onValueChange={(value) => setKind(value === "relay" ? "relay" : "node")}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="node">节点一次性令牌</SelectItem>
                <SelectItem value="relay">卫星中继令牌</SelectItem>
              </SelectContent>
            </Select>
          </div>
          {kind === "node" && (
            <div className="flex w-40 flex-col gap-1">
              <Label htmlFor="token-user">属主用户</Label>
              <Input
                id="token-user"
                placeholder="须为已有用户"
                value={user}
                onChange={(event) => setUser(event.target.value)}
              />
            </div>
          )}
          <div className="flex w-32 flex-col gap-1">
            <Label>有效期</Label>
            <Select value={ttlSecs} onValueChange={setTtlSecs}>
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {TTL_OPTIONS.map((option) => (
                  <SelectItem key={option.value} value={option.value}>
                    {option.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <Button
            type="submit"
            variant="primary"
            disabled={
              issueMutation.isPending || network === null || (kind === "node" && !user.trim())
            }
          >
            签发
          </Button>
        </div>
        {result !== null && (
          <div
            className={cn(
              "flex flex-col gap-2 rounded-md p-3", // 布局
              "border border-border", // 分层
            )}
          >
            <div className="flex items-center justify-between gap-3">
              <p className="text-sm">
                令牌 #{result.token_id}
                <span className="ml-2 text-xs text-muted-foreground">明文仅展示这一次</span>
              </p>
              <Button
                size="sm"
                onClick={() => {
                  void navigator.clipboard.writeText(result.token);
                  setCopied(true);
                }}
              >
                {copied ? "已复制" : "复制"}
              </Button>
            </div>
            <p className="break-all rounded-sm bg-muted px-2 py-1.5 font-mono text-xs">
              {result.token}
            </p>
          </div>
        )}
      </form>

      <div className="grid gap-4 md:grid-cols-2">
        <form
          className={cn(
            "flex flex-col gap-3 rounded-lg p-4", // 布局
            "border border-border", // 分层
          )}
          onSubmit={(event) => submitCreateUser(event)}
        >
          <h2 className="font-medium">创建用户</h2>
          <p className="text-xs text-muted-foreground">
            用户是属主标注与令牌归属的载体, 节点令牌签发前须先创建
          </p>
          <div className="flex items-end gap-2">
            <div className="flex flex-1 flex-col gap-1">
              <Label htmlFor="new-user">用户名</Label>
              <Input
                id="new-user"
                placeholder="1-32 位字母数字与 - _"
                value={newUser}
                onChange={(event) => setNewUser(event.target.value)}
              />
            </div>
            <Button
              type="submit"
              disabled={createUserMutation.isPending || network === null || !newUser.trim()}
            >
              创建
            </Button>
          </div>
          {createUserMutation.isSuccess && newUser !== "" && (
            <p className="text-xs text-success">用户 {newUser} 已创建</p>
          )}
        </form>

        <form
          className={cn(
            "flex flex-col gap-3 rounded-lg p-4", // 布局
            "border border-border", // 分层
          )}
          onSubmit={(event) => submitDisableUser(event)}
        >
          <h2 className="font-medium">禁用用户</h2>
          <p className="text-xs text-muted-foreground">名下节点凭证即吊销断线, 令牌全部失效</p>
          <div className="flex items-end gap-2">
            <div className="flex flex-1 flex-col gap-1">
              <Label htmlFor="disable-user">用户名</Label>
              <Input
                id="disable-user"
                placeholder="要禁用的用户"
                value={disableUser}
                onChange={(event) => setDisableUser(event.target.value)}
              />
            </div>
            <Button
              type="submit"
              variant="destructive"
              disabled={disableUserMutation.isPending || network === null || !disableUser.trim()}
            >
              禁用
            </Button>
          </div>
        </form>
      </div>

      <form
        className={cn(
          "flex flex-wrap items-end gap-3 rounded-lg p-4", // 布局
          "border border-border", // 分层
        )}
        onSubmit={(event) => {
          event.preventDefault();
          const id = Number(revokeId.trim());
          if (Number.isInteger(id) && id > 0) {
            revokeMutation.mutate(id);
          }
        }}
      >
        <h2 className="w-full font-medium">撤销令牌</h2>
        <div className="flex w-48 flex-col gap-1">
          <Label htmlFor="revoke-id">令牌 ID</Label>
          <Input
            id="revoke-id"
            type="number"
            placeholder="签发结果中的 #ID"
            value={revokeId}
            onChange={(event) => setRevokeId(event.target.value)}
          />
        </div>
        <Button type="submit" disabled={revokeMutation.isPending}>
          撤销
        </Button>
        {revokeMutation.isSuccess && (
          <p className="text-xs text-success">已撤销 (relay 令牌撤销即卫星断线)</p>
        )}
        <p className="w-full text-xs text-muted-foreground">
          节点令牌撤销后注册被拒; 卫星注册令牌撤销即踢线
        </p>
      </form>
    </section>
  );

  /** 提交创建用户。 */
  function submitCreateUser(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (network !== null && newUser.trim()) {
      createUserMutation.mutate({ network, name: newUser.trim() });
    }
  }

  /** 提交禁用用户 (不可逆操作走二次确认)。 */
  function submitDisableUser(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (network !== null && disableUser.trim()) {
      const name = disableUser.trim();
      if (window.confirm(`禁用用户 ${name}? 其名下节点将全部断线`)) {
        disableUserMutation.mutate({ network, name });
      }
    }
  }
}
