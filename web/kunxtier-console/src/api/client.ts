/*
 * Description: admin API 客户端 (会话令牌 sessionStorage 标签页级续期, 401 统一广播登出)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

/** 401 统一事件名: App 层监听后跳转登录页。 */
export const UNAUTHORIZED_EVENT = "kunxtier:unauthorized";

/** 后端错误体投影 (kunxtier-core protocol.rs ApiErrorBody)。 */
interface ApiErrorShape {
  code: string;
  message: string;
}

/** 请求描述: json 存在即以 POST + application/json 发送。 */
interface ApiRequestInit {
  method?: string;
  json?: unknown;
}

/** 会话令牌持久化键 (sessionStorage: 标签页生命周期, 关标签即失效)。 */
const SESSION_STORAGE_KEY = "kunxtier-console-session";

/** 会话令牌 (内存态; 标签页内刷新经 sessionStorage 续期, FR-7 管理数据不落本地)。 */
let sessionToken: string | null = readStoredToken();

/** 读回标签页内持久化的会话令牌 (隐私模式等存取受限时静默降级为刷新重登)。 */
function readStoredToken(): string | null {
  try {
    return sessionStorage.getItem(SESSION_STORAGE_KEY);
  } catch {
    return null;
  }
}

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

/** 写入会话令牌 (登录成功 / 登出清空 / 401 失效), 同步标签页级持久化。 */
export function setToken(token: string | null): void {
  sessionToken = token;
  try {
    if (token === null) {
      sessionStorage.removeItem(SESSION_STORAGE_KEY);
    } else {
      sessionStorage.setItem(SESSION_STORAGE_KEY, token);
    }
  } catch {
    // 存取受限 (隐私模式) 时退化为刷新重登, 不影响当前会话
  }
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
      body?.code ?? "error",
      body?.message ?? `请求失败 (${resp.status})`,
    );
  }
  return (await resp.json()) as T;
}
