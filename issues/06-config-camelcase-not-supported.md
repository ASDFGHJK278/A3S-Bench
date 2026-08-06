Provider 配置不支持 camelCase 字段名

标签: bug

## 现象

`resolve_model_route` 仅查找 `api_key` 和 `base_url`（snake_case）。部分 provider 配置使用 camelCase（`apiKey`、`baseUrl`），导致解析失败报错 "provider has no api_key"。

## 根因

`resolve_model_route` 对 `api_key`/`base_url` 的查找是硬编码的 snake_case，没有回退到 camelCase 变体。

## 环境

- a3s-bench v0.1.2
- 使用 camelCase 字段名的 provider 配置
