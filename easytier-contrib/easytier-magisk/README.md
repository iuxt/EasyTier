# EasyTier Magisk 模块

模块仅使用 `config/config.toml` 配置文件。修改配置后重启模块服务。
完整中文模板见仓库根目录的 `config.toml`，所有参数、日志和管理入口均通过 TOML 设置。

`easytier_core.sh` 使用 `easytier-core -c config/config.toml` 启动并守护进程。
不再提供 WebUI、远程 Web 配置服务或启动参数文件。

构建模块：先准备 ARM64 的 `easytier-core`、`easytier-cli`，再运行 `build.sh`。
