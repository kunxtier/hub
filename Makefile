# hub monorepo 根编排: git hooks 装载 + 各语言域代理。
# 多语言域按 "<domain>-<target>" 命名在此扩展 (当前 rust, 后续 web)。

.DEFAULT_GOAL := help

.PHONY: help hooks hooks-uninstall rust-lint rust-test

help: ## 显示可用目标
	@awk -F':.*## ' '/^[a-zA-Z_-]+:.*## /{printf "  \033[36m%-16s\033[0m %s\n",$$1,$$2}' $(MAKEFILE_LIST)

hooks: ## 装载 git hooks (core.hooksPath 指向 .githooks, 随仓库版本化)
	git config core.hooksPath .githooks
	@echo "hooks 已装载: pre-commit / commit-msg (卸载: make hooks-uninstall)"

hooks-uninstall: ## 卸载 git hooks
	git config --unset core.hooksPath || true
	@echo "hooks 已卸载"

rust-lint: ## rust 域质量检查 (格式 + clippy)
	cd rust/kunxtier && cargo make lint

rust-test: ## rust 域全量测试
	cd rust/kunxtier && cargo make test
