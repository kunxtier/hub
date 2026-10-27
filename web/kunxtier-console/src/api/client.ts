/*
 * Description: admin API 客户端 (会话令牌仅存模块内存, 401 统一广播登出)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

/** 401 统一事件名: App 层监听后跳转登录页。 */
export const UNAUTHORIZED_EVENT = "kunxtier:unauthorized";

/** 后端错误体投影 (kunxtier-relay server.rs ApiError / ApiErrorBody)。 */
interface ApiErrorShape {
  error: string;
  message: string;
}

/** 请求描述: json 存在即以 POST + application/json 发送。 */
interface ApiRequestInit {
  method?: string;
  json?: unknown;
}

/** 会话令牌 (内存态, FR-7 无本地存储: 刷新即重登)。 */
let sessionToken: string | null = null;

/** API 调用错误: HTTP 状态 + 后端错误码 + 可读信息。 */
export class ApiRequestError extends Error {
  /** HTTP 状态码; 401 表示会话失效。 */
  readonly status: number;
  /** 后端错误码 (如 unauthorized / invalid_request)。 */
  readonly code: string;

  constructor(status: number, code: string, message: string) {
    super(message);
    this.name = "ApiRequestError";
    this.status = status;
    this.code = code;
  }
}

/** 写入会话令牌 (登录成功 / 登出清空)。 */
export function setToken(token: string | null): void {
  sessionToken = token;
}

/** 从任意抛出错误提取用户可读文案 (mutation / query 错误展示统一入口)。 */
export function errorMessage(err: unknown): string {
  return err instanceof ApiRequestError ? err.message : "操作失败, 请重试";
}

/** 读取当前会话令牌。 */
export function getToken(): string | null {
  return sessionToken;
}

/**
 * 发起 admin API 请求; 401 时清除令牌并广播 UNAUTHORIZED_EVENT。
 * @throws ApiRequestError 非 2xx 应答
 */
export async function api<T>(path: string, init: ApiRequestInit = {}): Promise<T> {
  const headers: Record<string, string> = {};
  if (init.json !== undefined) {
    headers["Content-Type"] = "application/json";
  }
  if (sessionToken) {
    headers["Authorization"] = `Bearer ${sessionToken}`;
  }
  const resp = await fetch(path, {
    method: init.method ?? (init.json !== undefined ? "POST" : "GET"),
    headers,
    body: init.json !== undefined ? JSON.stringify(init.json) : undefined,
  });
  if (resp.status === 401) {
    setToken(null);
    window.dispatchEvent(new Event(UNAUTHORIZED_EVENT));
    throw new ApiRequestError(401, "unauthorized", "会话已失效, 请重新登录");
  }
  if (!resp.ok) {
    const body = (await resp.json().catch(() => null)) as ApiErrorShape | null;
    throw new ApiRequestError(
      resp.status,
      body?.error ?? "error",
      body?.message ?? `请求失败 (${resp.status})`,
    );
  }
  return (await resp.json()) as T;
}
