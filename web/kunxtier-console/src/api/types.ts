/*
 * Description: admin API 契约类型投影 (权威定义与快照对照见 kunxtier-core src/protocol.rs 的 rest_contract_snapshots 测试)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

/** 列表响应信封 (page 式分页; 权威定义见 kunxtier-core protocol.rs Page)。 */
export interface Page<T> {
  /** 当前页条目。 */
  items: T[];
  /** 页码, 从 1 起。 */
  page: number;
  /** 页大小 (契约上限 200)。 */
  size: number;
  /** 总条目数 (个人规模小表精确计数)。 */
  total: number;
}

/** 网络组条目 (GET /api/v1/admin/networks)。 */
export interface NetworkInfo {
  /** 网络组名 (切换器与各页面的网络维度键)。 */
  name: string;
  /** 地址段, 形如 "100.66.0.0/16"。 */
  cidr: string;
  /** 创建时间 (unix 秒)。 */
  created_at: number;
}

/**
 * 逐对端路径状态 (封闭词汇, 契约投影)。
 * 后端 protocol.rs 透传节点上报字符串, 不在服务端校验; 若未来节点上报新词汇,
 * 前端按原样降级展示 (标签回退原词, 计数不计入), 以本投影过期为信号对齐契约。
 * - idle: 空闲 (无活跃路径)
 * - punching: 打洞进行中
 * - direct: 直连已建立
 * - relayed: 经中继转发
 */
export type PathStatus = "idle" | "punching" | "direct" | "relayed";

/** 逐对端路径快照 (节点路径上报的聚合条目)。 */
export interface PeerPathReport {
  /** 对端节点名。 */
  peer: string;
  /** 路径状态 (与 status 词汇一致)。 */
  path: PathStatus;
  /** 走中继时的中继地址 (与中继列表条目 addr 一致); 其余路径缺省。 */
  relay?: string;
}

/** 路径策略: auto 自动打洞回落, force_relay 强制中继。 */
export type PathPolicy = "auto" | "force_relay";

/** 节点条目 (GET /api/v1/admin/nodes; 在线态/候选地址/版本/路径由信令注册表合并)。 */
export interface NodeInfo {
  network: string;
  name: string;
  ip: string;
  /** 属主标注 (仅标注语义, 无属主登录)。 */
  owner: string | null;
  created_at: number;
  online: boolean;
  policy: PathPolicy;
  /** 最近上报的候选地址 (仅在线节点有意义)。 */
  candidates: string[];
  /** 节点二进制版本 (仅在线节点有意义)。 */
  version: string | null;
  /** 逐对端路径快照 (仅在线节点有意义)。 */
  paths: PeerPathReport[];
}

/** 中继汇总条目 (GET /api/v1/admin/relays)。 */
export interface RelayInfo {
  /** 对外地址 host:port。 */
  addr: string;
  /** "core" (核心内置转发) 或 "satellite" (卫星)。 */
  kind: string;
  /** 端点 TLS 证书指纹 (sha256 hex)。 */
  fingerprint: string;
  /** 当前经该中继转发的节点数。 */
  nodes: number;
}

/** 登录应答 (POST /api/v1/admin/login)。 */
export interface LoginResponse {
  /** 会话令牌 (Bearer, 服务端内存态, relay 重启即失效)。 */
  token: string;
  /** 过期时间 (unix 秒)。 */
  expires_at: number;
}

/** 用户条目 (GET /api/v1/admin/users; 属主标注载体, 无属主登录)。 */
export interface UserInfo {
  network: string;
  name: string;
  disabled: boolean;
  /** 创建时间 (unix 秒)。 */
  created_at: number;
}

/** 令牌签发应答 (POST /api/v1/admin/tokens); 明文仅此一次展示。 */
export interface IssueTokenResponse {
  token_id: number;
  token: string;
}
