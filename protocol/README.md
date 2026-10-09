# Xcoc 产品协议来源

`protocol-contract.json` 是 Server 官方仓库版本 v0.4.0 / 完整 revision `42f38ae8dbe78d5fe0a4cafac81095f97f1c60b9` 中 `web/src/protocol-contract.json` 的受控逐字节副本，SHA256 `45948784e2670e9d3803bfcbd8d9c442a29bc38b3e61e50b103c6fa92f33fa42`。复制时使用该不可变 Git 对象并核 SHA，未将相邻工作树或未发布远端假作可重新获取的权威。

`build.rs` 校验严格 provenance、完整 commit 和副本 SHA，再派生唯一 edge protocol / API prefix 编译常量。产品的 wire schema 与能力/回执语义由 Server 定义，Client 仅消费当前协议；不能手改常量或回退旧 bool/outcome 字段。升级副本需从新的受控正式源对象重做复制、记录并运行协议一致性与故障回归。
