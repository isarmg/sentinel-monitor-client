# xcoc 产品协议来源

`protocol-contract.json` 是服务端官方仓库版本 v1.0.0 / 完整提交 `d4da410120eb39af2ed2975c872ae3b5ca16f85b` 中 `web/src/protocol-contract.json` 的受控逐字节副本，SHA256 `3036eb547d0780dad40963b1b88d6fcac31bbdc689d3373e70a0a3430ec6573e`。复制时使用该不可变 Git 对象并核 SHA，未将相邻工作树或未发布远端假作可重新获取的权威。

`build.rs` 严格校验来源记录、完整提交和副本 SHA，再派生唯一的边缘协议及 API 路径前缀编译常量。产品的线上协议结构与能力、回执语义由服务端定义，客户端仅消费当前协议；不能手改常量或回退旧 bool/outcome 字段。升级副本需从新的受控正式源对象重新复制、记录，并运行协议一致性与故障回归。
