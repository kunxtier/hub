/*
 * Description: Vite 构建配置 (React + Tailwind 插件, 产物直出 relay 嵌入目录)
 *
 * Author: qinzhenya
 * Created: 2026-10-27
 */

import { fileURLToPath, URL } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// 产物直接输出 relay 嵌入目录: 二进制重编译即换资产;
// assets/ 下哈希文件名仅服务浏览器长缓存 (relay 侧按前缀定 immutable)。
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  server: {
    // 本地开发: /api 代理核心中继; 自签证书校验在 dev 代理放行
    // (生产形态 TLS 固定在核心托管 / console 反代层, 不经过此配置)
    proxy: {
      "/api": {
        target: "https://127.0.0.1:9443",
        changeOrigin: true,
        secure: false,
      },
    },
  },
  build: {
    outDir: "../../rust/kunxtier/crates/kunxtier-relay/assets/console",
    // 清目录: outDir 在项目根之外, vite 缺省不清, 显式打开;
    // 目录内无随 git 的文件 (fresh clone 兜底页在 relay 代码内嵌)
    emptyOutDir: true,
  },
});
