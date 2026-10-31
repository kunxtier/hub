/*
 * Description: 令牌与用户页 (签发 / 撤销 / 用户创建与管理; 明文仅展示一次)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import TokenIssueForm from "@/components/tokens/TokenIssueForm";
import TokenRevokeForm from "@/components/tokens/TokenRevokeForm";
import UserCreateForm from "@/components/tokens/UserCreateForm";
import UserTable from "@/components/tokens/UserTable";
import { useApp } from "@/state/AppContext";

/** 令牌与用户页: 管理链路操作集中地 (FR-2), 各表单域自持状态与错误反馈。 */
export default function TokensPage() {
  const { network } = useApp();

  return (
    <section className="flex flex-col gap-5">
      <div>
        <h1 className="text-lg font-semibold">令牌与用户</h1>
        <p className="mt-0.5 text-sm text-muted-foreground">
          网络组 {network ?? "未选择"}: 令牌即授权, 无审批流
        </p>
      </div>

      <TokenIssueForm network={network} />

      <div className="grid gap-4 md:grid-cols-2">
        <UserCreateForm network={network} />
        <UserTable network={network} />
      </div>

      <TokenRevokeForm />
    </section>
  );
}
