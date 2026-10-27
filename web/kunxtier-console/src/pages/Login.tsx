/*
 * Description: 登录页 (admin 凭证换会话, 失败信息就地展示)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import { useState, type FormEvent } from "react";
import { ApiRequestError } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { cn } from "@/lib/utils";
import { useApp } from "@/state/AppContext";

/** 登录页; 提交期间禁用并提示状态。 */
export default function LoginPage() {
  const { login } = useApp();
  const [credential, setCredential] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!credential || submitting) {
      return;
    }
    setSubmitting(true);
    setError(null);
    try {
      await login(credential);
    } catch (err) {
      setError(err instanceof ApiRequestError ? err.message : "登录失败, 请重试");
      setSubmitting(false);
    }
  }

  return (
    <div className="flex min-h-screen items-center justify-center p-4">
      <form
        onSubmit={(event) => void onSubmit(event)}
        className={cn(
          "w-full max-w-sm rounded-lg p-6", // 布局尺寸
          "border border-border bg-card", // 分层
        )}
      >
        <h1 className="text-lg font-semibold">kunxtier 控制台</h1>
        <p className="mt-1 text-sm text-muted-foreground">使用核心中继的 admin 凭证登录</p>
        <div className="mt-5 flex flex-col gap-1.5">
          <Label htmlFor="credential">admin 凭证</Label>
          <Input
            id="credential"
            type="password"
            autoComplete="current-password"
            autoFocus
            value={credential}
            onChange={(event) => setCredential(event.target.value)}
            disabled={submitting}
          />
        </div>
        {error !== null && <p className="mt-3 text-sm text-destructive">{error}</p>}
        <Button
          type="submit"
          variant="primary"
          className="mt-5 w-full"
          disabled={submitting || !credential}
        >
          {submitting ? "登录中" : "登录"}
        </Button>
      </form>
    </div>
  );
}
