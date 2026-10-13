# One command per app. Run from the repo root.
.PHONY: api web test migrate check

api:        ## API + worker in one process (reads .env)
	cargo run --manifest-path apps/api/Cargo.toml -- serve

web:        ## React dev server on :5173
	cd apps/web && pnpm dev

migrate:
	cargo run --manifest-path apps/api/Cargo.toml -- migrate

test:
	cargo test --manifest-path apps/api/Cargo.toml
	cd apps/web && pnpm test --run

check:
	cargo clippy --manifest-path apps/api/Cargo.toml --all-targets -- -D warnings
	cd apps/web && pnpm typecheck
