# CI

Core workflow 在普通 PR/push 上执行 Linux、Windows、macOS 编译检查；手动触发或 release 分支执行全量跨平台构建，并打包 Core、CLI、中文 config.toml 模板和 Magisk 模块。

Test workflow 执行 CI 脚本测试、格式检查、clippy、WASI、依赖锁检查及网络测试。网络集成测试需要 Linux namespace、网桥及相应权限。

Nightly 比较 main HEAD 与 Core、Test 的最近成功手动构建，按需触发并等待同一提交构建完成；失败任务重试一次。成功后发布固定 nightly 预发布版本和 SHA256SUMS。版本号从 easytier/Cargo.toml 读取。

Release workflow 接收成功的 Core 全量构建 run ID，下载产物并创建草稿发布。GUI、移动 GUI 和 Web 构建任务已经移除。

OHOS、Go、JavaScript Hosts、Nix 独立使用各自路径过滤。Docker 使用指定 Core 构建产物。
