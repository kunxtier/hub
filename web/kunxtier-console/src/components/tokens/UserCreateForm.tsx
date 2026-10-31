/*
 * Description: 用户创建表单 (属主标注载体, 节点令牌签发的前置)
 *
 * Author: qinzhenya
 * Created: 2026-10-31
 */

import { useState, type FormEvent } from "react";
import { errorMessage } from "@/api/client";
import { useCreateUserMutation } from "@/api/hooks";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { cn } from "@/lib/utils";

interface UserCreateFormProps {
  /** 当前网络组; null (未选择) 时禁用创建。 */
  network: string | null;
}

/** 创建用户表单: 用户名 1-32 位字母数字与 - _。 */
export default function UserCreateForm({ network }: UserCreateFormProps) {
  const createUserMutation = useCreateUserMutation();
  const [newUser, setNewUser] = useState("");
  /** 最近一次成功创建的用户名 (提示锚定提交时点, 不随输入框漂移)。 */
  const [createdName, setCreatedName] = useState<string | null>(null);

  return (
    <form
      className={cn(
        "flex flex-col gap-3 rounded-lg p-4", // 布局
        "border border-border", // 分层
      )}
      onSubmit={(event) => submitCreate(event)}
    >
      <h2 className="font-medium">创建用户</h2>
      <p className="text-xs text-muted-foreground">
        用户是属主标注与令牌归属的载体, 节点令牌签发前须先创建
      </p>
      {createUserMutation.error !== null && (
        <p className="text-sm text-destructive">{errorMessage(createUserMutation.error)}</p>
      )}
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
      {createUserMutation.isSuccess && createdName !== null && (
        <p className="text-xs text-success">用户 {createdName} 已创建</p>
      )}
    </form>
  );

  /** 提交创建用户; 成功后记录实际创建的用户名。 */
  function submitCreate(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const trimmed = newUser.trim();
    if (network !== null && trimmed) {
      createUserMutation.mutate(
        { network, name: trimmed },
        { onSuccess: () => setCreatedName(trimmed) },
      );
    }
  }
}
