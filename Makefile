.PHONY: dev check test website-check relay-worker-check tray-test apple-test mobile-projects

dev:
	bash scripts/dev.sh

check:
	bash scripts/check.sh
	cargo fmt --all --check
	bash -c 'source scripts/build/macos_env.sh; cargo clippy --locked --workspace --all-targets -- -D warnings'

test:
	bash -c 'source scripts/build/macos_env.sh; cargo test --locked --workspace -- --test-threads=1'

website-check:
	cd apps/website && pnpm check && pnpm check:docs && pnpm build && pnpm test

relay-worker-check:
	cd apps/relay-worker && npm run check && npm test

tray-test:
	bash apps/tray/Tests/run_integration_test.sh

apple-test:
	bash -c 'source scripts/build/macos_env.sh; xcrun swift test --package-path packages/apple-cloud-sync'

mobile-projects:
	xcodegen generate --spec apps/mobile/project.yml
	xcodegen generate --spec apps/cloud-sync-helper/project.yml
