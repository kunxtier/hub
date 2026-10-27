# hub monorepo 根编排: git hooks 装载 + 跨项目聚合入口。
# 目标保持项目无关命名; 各项目自持任务入口 (rust 走 cargo make),
# 新项目落地时在对应目标内追加调用行。

.DEFAULT_GOAL := help

.PHONY: help verify hooks hooks-uninstall dev relay-dev console-dev

help: ## 显示可用目标
	@awk -F':.*## ' '/^[a-zA-Z_-]+:.*## /{printf "  \033[36m%-16s\033[0m %s\n",$$1,$$2}' $(MAKEFILE_LIST)

hooks: ## 装载 git hooks (core.hooksPath 指向 .githooks, 随仓库版本化)
	git config core.hooksPath .githooks
	@echo "hooks 已装载: pre-commit / commit-msg (卸载: make hooks-uninstall)"

hooks-uninstall: ## 卸载 git hooks
	git config --unset core.hooksPath || true
	@echo "hooks 已卸载"

install: ## 安装依赖 (web)
	cd web/kunxtier-console && pnpm install

fmt: ## rust 域格式化
	cd rust/kunxtier && cargo make fmt

lint: ## rust 域质量检查 (格式 + clippy)
	cd rust/kunxtier && cargo make lint

test: ## rust 域全量测试
	cd rust/kunxtier && cargo make test

verify: ## 一键检查全部项目 (格式 + 静态检查 + 编译 + 测试; 当前 rust, 新项目追加调用行)
	cd rust/kunxtier && cargo make verify

# 开发热更: 前置 rust/kunxtier 已 relay-init 且装好 cargo-watch, web/kunxtier-console 已 pnpm install。
# 浏览器访问 http://127.0.0.1:5173, /api 由 Vite 代理到核心中继 9443。

dev-relay: ## 核心中继热编译重启 (委托 cargo make relay-watch)
	cd rust/kunxtier && cargo make relay-watch

dev-console: ## 前端 Vite 热更 (/api 代理本地核心中继)
	cd web/kunxtier-console && pnpm dev