Bench 直接读 ACL attributes 未兼容 camelCase，与 a3s-code-core loader 行为不一致

标签: bug

## 现象

用户使用 a3s-code 生成的 `.a3s/config.acl`（camelCase 风格）直接跑 `a3s bench run` 时，`resolve_model_route` 报错：

```
provider "openai" has no api_key
```

a3s-code 生成并正常使用的配置如下：

```
providers "openai" {
  apiKey = "test"
  baseUrl = "https://example.com/v1"
  models "fake" { name = "Fake" }
}
```

## 根因

a3s-code-core 的 config loader（`a3s-code-core/src/config/loader.rs:626`）在解析 provider attributes 时用 `match` 同时匹配两种写法：

```rust
"apiKey" | "api_key" => { provider.api_key = Some(api_key); }
"baseUrl" | "base_url" => { provider.base_url = Some(base_url); }
```

而 A3S-Bench 的 `resolve_model_route`（`src/config.rs`）绕过了 a3s-code-core 的 loader，直接用 `provider.attributes.get("api_key")` 读取 a3s-acl parser 输出的原始 HashMap。a3s-acl parser 原样存储 key 不做归一化，因此只能匹配 snake_case，camelCase 配置无法识别。

两端共享同一份 `config.acl`，但解析行为不一致，导致用户必须为 bench 和 a3s-code 分别维护不同字段名的配置。

## 环境

- a3s-bench v0.1.2
- 由 a3s-code 生成的 config.acl（camelCase 风格）
