/*
 * Description: 用户列表与生命周期操作 (启用 / 禁用 / 改名 / 删除, 均带确认)
 *
 * Author: qinzhenya
 * Created: 2026-10-31
 */

import { errorMessage } from "@/api/client";
import {
  useDeleteUserMutation,
  useDisableUserMutation,
  useEnableUserMutation,
  useRenameUserMutation,
  useUsersQuery,
} from "@/api/hooks";
import ErrorHint from "@/components/ErrorHint";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { formatTime } from "@/lib/summary";
import { cn } from "@/lib/utils";

interface UserTableProps {
  /** 当前网络组; null (未选择) 时不发起查询。 */
  network: string | null;
}

/** 用户列表: 属主标注与令牌归属, 禁用即吊销名下节点凭证与令牌。 */
export default function UserTable({ network }: UserTableProps) {
  const usersQuery = useUsersQuery(network);
  const users = usersQuery.data ?? [];
  const disableUserMutation = useDisableUserMutation();
  const enableUserMutation = useEnableUserMutation();
  const renameUserMutation = useRenameUserMutation();
  const deleteUserMutation = useDeleteUserMutation();
  const mutationError =
    disableUserMutation.error ??
    enableUserMutation.error ??
    renameUserMutation.error ??
    deleteUserMutation.error;

  return (
    <div
      className={cn(
        "flex flex-col gap-3 rounded-lg p-4", // 布局
        "border border-border", // 分层
      )}
    >
      <h2 className="font-medium">用户列表</h2>
      <p className="text-xs text-muted-foreground">
        属主标注与令牌归属; 禁用即吊销名下节点凭证与令牌
      </p>
      {mutationError !== null && (
        <p className="text-sm text-destructive">{errorMessage(mutationError)}</p>
      )}
      {network === null ? (
        <p className="text-sm text-muted-foreground">未选择网络组</p>
      ) : usersQuery.isLoading ? (
        <p className="text-sm text-muted-foreground">用户加载中</p>
      ) : usersQuery.isError ? (
        <ErrorHint error={usersQuery.error} onRetry={() => void usersQuery.refetch()} />
      ) : users.length === 0 ? (
        <p className="text-sm text-muted-foreground">网络组内暂无用户</p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-border text-left text-xs text-muted-foreground">
                <th className="py-1.5 pr-3 font-medium">用户</th>
                <th className="py-1.5 pr-3 font-medium">状态</th>
                <th className="py-1.5 pr-3 font-medium">创建</th>
                <th className="py-1.5 font-medium">操作</th>
              </tr>
            </thead>
            <tbody>
              {users.map((entry) => (
                <tr key={entry.name} className="border-b border-border last:border-b-0">
                  <td className="py-1.5 pr-3 font-medium">{entry.name}</td>
                  <td className="py-1.5 pr-3">
                    <Badge variant={entry.disabled ? "danger" : "neutral"}>
                      {entry.disabled ? "已禁用" : "正常"}
                    </Badge>
                  </td>
                  <td className="py-1.5 pr-3 text-xs text-muted-foreground">
                    {formatTime(entry.created_at)}
                  </td>
                  <td className="py-1.5">
                    <div className="flex gap-1.5">
                      {entry.disabled ? (
                        <Button
                          size="sm"
                          disabled={enableUserMutation.isPending}
                          onClick={() => enableUserByName(entry.name)}
                        >
                          启用
                        </Button>
                      ) : (
                        <Button
                          size="sm"
                          variant="destructive"
                          disabled={disableUserMutation.isPending}
                          onClick={() => disableUserByName(entry.name)}
                        >
                          禁用
                        </Button>
                      )}
                      <Button
                        size="sm"
                        disabled={renameUserMutation.isPending}
                        onClick={() => renameUserByName(entry.name)}
                      >
                        改名
                      </Button>
                      <Button
                        size="sm"
                        variant="destructive"
                        disabled={deleteUserMutation.isPending}
                        onClick={() => deleteUserByName(entry.name)}
                      >
                        删除
                      </Button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );

  /** 禁用用户 (名下节点即断线, 可经启用恢复)。 */
  function disableUserByName(name: string) {
    if (network !== null && window.confirm(`禁用用户 ${name}? 其名下节点将全部断线`)) {
      disableUserMutation.mutate({ network, name });
    }
  }

  /** 启用用户 (禁用的逆操作)。 */
  function enableUserByName(name: string) {
    if (network !== null) {
      enableUserMutation.mutate({ network, name });
    }
  }

  /** 改名 (属主标注随之传播)。 */
  function renameUserByName(name: string) {
    if (network === null) {
      return;
    }
    const newName = window.prompt(`将用户 ${name} 改名为:`, name)?.trim();
    if (newName && newName !== name) {
      renameUserMutation.mutate({ network, name, newName });
    }
  }

  /** 删除用户 (名下有未移除节点时服务端拒绝)。 */
  function deleteUserByName(name: string) {
    if (network !== null && window.confirm(`删除用户 ${name}? 其历史令牌一并清理`)) {
      deleteUserMutation.mutate({ network, name });
    }
  }
}
