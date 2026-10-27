/*
 * Description: 节点与路径的展示层派生计算 (时间格式化与路径聚合)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import type { NodeInfo } from "@/api/types";

/** 路径状态中文标签 (词汇与 PeerPathReport.path / status 一致)。 */
export const PATH_LABELS: Record<string, string> = {
  idle: "空闲",
  punching: "打洞中",
  direct: "直连",
  relayed: "中继",
};

/** 路径聚合计数形状 (键与 PATH_LABELS 词汇一致)。 */
export interface PathSummary {
  direct: number;
  relayed: number;
  punching: number;
  idle: number;
}

/** unix 秒转本地时间串 (表格展示用)。 */
export function formatTime(unixSec: number): string {
  return new Date(unixSec * 1000).toLocaleString();
}

/** 汇总全网逐对端路径计数 (总览统计用)。 */
export function summarizePaths(nodes: NodeInfo[]): PathSummary {
  const summary: PathSummary = { direct: 0, relayed: 0, punching: 0, idle: 0 };
  for (const node of nodes) {
    for (const report of node.paths) {
      if (report.path in summary) {
        summary[report.path as keyof PathSummary] += 1;
      }
    }
  }
  return summary;
}
