# hub monorepo 根编排: git hooks 装载 + 跨项目聚合入口。
# 目标保持项目无关命名; 各项目自持任务入口 (rust 走 cargo make),
# 新项目落地时在对应目标内追加调用行。

.DEFAULT_GOAL := help

.PHONY: help verify hooks hooks-uninstall

help: ## 显示可用目标
	@awk -F':.*## ' '/^[a-zA-Z_-]+:.*## /{printf "  \033[36m%-16s\033[0m %s\n",$$1,$$2}' $(MAKEFILE_LIST)

hooks: ## 装载 git hooks (core.hooksPath 指向 .githooks, 随仓库版本化)
	git config core.hooksPath .githooks
	@echo "hooks 已装载: pre-commit / commit-msg (卸载: make hooks-uninstall)"

hooks-uninstall: ## 卸载 git hooks
	git config --unset core.hooksPath || true
	@echo "hooks 已卸载"

fmt: ## rust 域格式化
	cd rust/kunxtier && cargo make fmt

lint: ## rust 域质量检查 (格式 + clippy)
	cd rust/kunxtier && cargo make lint

test: ## rust 域全量测试
	cd rust/kunxtier && cargo make test

verify: ## 一键检查全部项目 (格式 + 静态检查 + 编译 + 测试; 当前 rust, 新项目追加调用行)
	cd rust/kunxtier && cargo make verify