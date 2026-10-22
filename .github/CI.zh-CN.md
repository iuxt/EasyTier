# CI

向 `main` 推送代码时，Core workflow 自动执行全量跨平台构建。全部构建成功后，将同一次运行的产物打包为 ZIP，附带 `SHA256SUMS`，自动发布到当前仓库的 Releases，并标记为 Latest。

发布标签为 `v<包版本>-build.<Actions 运行编号>`，例如 `v2.7.0-build.42`。每次推送产生独立版本，无需手动创建 tag 或先发布历史版本。ZIP 中保留平台目录，包含 `easytier-core`、中文 `config.toml` 模板，Windows 包还包含 DLL 和驱动。上传全部成功前保持为草稿；失败后可在 Actions 中重跑失败任务。较旧提交的构建不会覆盖当前 main 的 Latest。

推送到 `main` 不进行 SignPath 签名，不需要配置额外 Secrets。发布任务使用工作流声明的 `GITHUB_TOKEN` 写权限；仓库需要启用 GitHub Actions，并允许工作流使用的 Actions。连续推送时会取消尚未完成的同分支旧构建。

Core workflow 在 PR 和其他开发分支 push 上执行 Linux、Windows、macOS 编译检查；手动触发或 `releases/**` 分支执行全量跨平台构建。手动触发默认仍可选 SignPath 签名，需要相应 Secret；不签名时将 `sign_windows` 设为 false。

Test workflow 执行 CI 脚本测试、格式检查、clippy、WASI、依赖锁检查及网络测试。网络集成测试需要 Linux namespace、网桥及相应权限。

Nightly 比较 main HEAD 与 Core、Test 的最近成功手动构建，按需触发并等待同一提交构建完成；失败任务重试一次。成功后发布固定 nightly 预发布版本和 SHA256SUMS。版本号从 easytier/Cargo.toml 读取。

Release workflow 接收成功的 Core 全量构建 run ID，下载产物并创建草稿发布。GUI、移动 GUI 和 Web 构建任务已经移除。

OHOS、Go、JavaScript Hosts、Nix 独立使用各自路径过滤。Docker 使用指定 Core 构建产物。
