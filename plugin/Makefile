CARGO ?= cargo

REQUEST_CORE := target/wasm32-unknown-unknown/release/gpt_codex_request_plugin.wasm
BUFFERED_CORE := target/wasm32-unknown-unknown/release/gpt_codex_buffered_response_plugin.wasm
STREAM_CORE := target/wasm32-unknown-unknown/release/gpt_codex_stream_response_plugin.wasm

REQUEST_COMPONENT := target/gpt-codex-request.component.wasm
BUFFERED_COMPONENT := target/gpt-codex-buffered-response.component.wasm
STREAM_COMPONENT := target/gpt-codex-stream-response.component.wasm

.PHONY: build check clean

build:
	$(CARGO) build --release --target wasm32-unknown-unknown \
		-p gpt-codex-request-plugin \
		-p gpt-codex-buffered-response-plugin \
		-p gpt-codex-stream-response-plugin
	$(CARGO) run --release -p gpt-plugin-componentize -- $(REQUEST_CORE) $(REQUEST_COMPONENT)
	$(CARGO) run --release -p gpt-plugin-componentize -- $(BUFFERED_CORE) $(BUFFERED_COMPONENT)
	$(CARGO) run --release -p gpt-plugin-componentize -- $(STREAM_CORE) $(STREAM_COMPONENT)

check:
	$(CARGO) fmt --all -- --check
	$(CARGO) test -p gpt-codex-plugin-common
	$(CARGO) check --target wasm32-unknown-unknown \
		-p gpt-codex-request-plugin \
		-p gpt-codex-buffered-response-plugin \
		-p gpt-codex-stream-response-plugin
	$(CARGO) check -p gpt-plugin-componentize
	$(CARGO) clippy -p gpt-codex-plugin-common --all-targets -- -D warnings
	$(CARGO) clippy --target wasm32-unknown-unknown \
		-p gpt-codex-request-plugin \
		-p gpt-codex-buffered-response-plugin \
		-p gpt-codex-stream-response-plugin -- -D warnings
	$(CARGO) clippy -p gpt-plugin-componentize -- -D warnings

clean:
	$(CARGO) clean
