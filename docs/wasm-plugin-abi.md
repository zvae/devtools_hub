# WASM Plugin ABI

Phase 2 loads plugins from `plugins/<plugin-id>/plugin.toml` and executes only `runtime = "wasm"` plugins. The host accepts the manifest layout described in [技术架构.md](../技术架构.md).

WASM modules must export the following functions:

```text
memory                 exported linear memory
alloc(length: i32) -> i32
execute(ptr: i32, len: i32) -> i64
```

`execute` receives UTF-8 JSON and returns `(result_ptr << 32) | result_len`. The returned bytes must be UTF-8 JSON:

```json
{
  "type": "text",
  "title": "Formatted JSON",
  "content": "{\n  \"hello\": \"world\"\n}",
  "actions": [{ "id": "copy", "title": "Copy", "kind": "copy" }]
}
```

The request shape is:

```json
{
  "command": "json.format",
  "input": "{ \"hello\": \"world\" }",
  "context": { "theme": "dark", "platform": "windows", "locale": "zh-CN" }
}
```

Plugins receive no WASI, filesystem, network, shell, clipboard, or host imports. Requested manifest permissions are checked before execution and are denied until a future settings UI stores an explicit grant. The host limits input to 512 KiB, output to 2 MiB, execution to two seconds, and CPU work through Wasmtime fuel.
