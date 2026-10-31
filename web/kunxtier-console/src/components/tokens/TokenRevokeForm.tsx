/*
 * Description: 令牌撤销表单 (按 token_id 撤销; relay 令牌撤销即卫星踢线)
 *
 * Author: qinzhenya
 * Created: 2026-10-31
 */

import { useState, type FormEvent } from "react";
import { errorMessage } from "@/api/client";
import { useRevokeTokenMutation } from "@/api/hooks";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { cn } from "@/lib/utils";

/** 令牌撤销表单: 按签发结果中的 #ID 撤销 (全局操作, 不绑定网络组)。 */
export default function TokenRevokeForm() {
  const revokeMutation = useRevokeTokenMutation();
  const [revokeId, setRevokeId] = useState("");
  const parsedId = Number(revokeId.trim());
  /** 提交可用: ID 为正整数 (空 / 零 / 非整数时按钮禁用, 不静默丢弃)。 */
  const idValid = Number.isInteger(parsedId) && parsedId > 0;

  return (
    <form
      className={cn(
        "flex flex-wrap items-end gap-3 rounded-lg p-4", // 布局
        "border border-border", // 分层
      )}
      onSubmit={(event) => submitRevoke(event)}
    >
      <h2 className="w-full font-medium">撤销令牌</h2>
      {revokeMutation.error !== null && (
        <p className="w-full text-sm text-destructive">{errorMessage(revokeMutation.error)}</p>
      )}
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
      <Button type="submit" disabled={revokeMutation.isPending || !idValid}>
        撤销
      </Button>
      {revokeMutation.isSuccess && (
        <p className="text-xs text-success">已撤销 (relay 令牌撤销即卫星断线)</p>
      )}
      <p className="w-full text-xs text-muted-foreground">
        节点令牌撤销后注册被拒; 卫星注册令牌撤销即踢线
      </p>
    </form>
  );

  /** 提交撤销 (按钮禁用兜底外再校验一次, 防御隐式提交路径)。 */
  function submitRevoke(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (idValid) {
      revokeMutation.mutate(parsedId);
    }
  }
}
