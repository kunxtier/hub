/*
 * Description: 令牌签发表单 (node / relay 两类; 明文仅展示一次, 复制结果显式反馈)
 *
 * Author: qinzhenya
 * Created: 2026-10-31
 */

import { useState, type FormEvent } from "react";
import { errorMessage } from "@/api/client";
import { useIssueTokenMutation } from "@/api/hooks";
import type { IssueTokenResponse } from "@/api/types";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select } from "@/components/ui/select";
import { SelectContent } from "@/components/ui/select-content";
import { SelectItem } from "@/components/ui/select-item";
import { SelectTrigger } from "@/components/ui/select-trigger";
import { SelectValue } from "@/components/ui/select-value";
import { cn } from "@/lib/utils";

/** 令牌有效期选项。 */
const TTL_OPTIONS = [
  { value: "3600", label: "1 小时" },
  { value: "43200", label: "12 小时" },
  { value: "86400", label: "1 天" },
  { value: "604800", label: "7 天" },
] as const;

/** 复制反馈状态: idle 未复制 / done 已复制 / failed 复制失败。 */
type CopyState = "idle" | "done" | "failed";

interface TokenIssueFormProps {
  /** 当前网络组; null (未选择) 时禁用签发。 */
  network: string | null;
}

/** 令牌签发表单 (FR-2): 类型 / 属主 / 有效期, 成功后明文仅展示一次。 */
export default function TokenIssueForm({ network }: TokenIssueFormProps) {
  const issueMutation = useIssueTokenMutation();
  const [kind, setKind] = useState<"node" | "relay">("node");
  const [user, setUser] = useState("");
  const [ttlSecs, setTtlSecs] = useState("86400");
  const [result, setResult] = useState<IssueTokenResponse | null>(null);
  const [copyState, setCopyState] = useState<CopyState>("idle");

  return (
    <form
      className={cn(
        "flex flex-col gap-3 rounded-lg p-4", // 布局
        "border border-border", // 分层
      )}
      onSubmit={(event) => submitIssue(event)}
    >
      <h2 className="font-medium">签发令牌</h2>
      {issueMutation.error !== null && (
        <p className="text-sm text-destructive">{errorMessage(issueMutation.error)}</p>
      )}
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
            <Button size="sm" onClick={() => void copyToken()}>
              {copyState === "done" ? "已复制" : copyState === "failed" ? "复制失败" : "复制"}
            </Button>
          </div>
          <p className="break-all rounded-sm bg-muted px-2 py-1.5 font-mono text-xs">
            {result.token}
          </p>
        </div>
      )}
    </form>
  );

  /** 提交签发; 成功后展示明文并复位复制反馈。 */
  function submitIssue(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (network === null) {
      return;
    }
    issueMutation.mutate(
      {
        network,
        kind,
        user: kind === "node" ? user.trim() : undefined,
        ttlSecs: Number(ttlSecs),
      },
      {
        onSuccess: (issued) => {
          setResult(issued);
          setCopyState("idle");
        },
      },
    );
  }

  /** 复制明文令牌到剪贴板; 失败时按钮文案如实反馈, 不谎报已复制。 */
  async function copyToken() {
    if (result === null) {
      return;
    }
    try {
      await navigator.clipboard.writeText(result.token);
      setCopyState("done");
    } catch {
      setCopyState("failed");
    }
  }
}
