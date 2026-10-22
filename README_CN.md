# EasyTier

[简体中文](README_CN.md) | [English](README.md)

EasyTier 是一个使用 Rust 和 Tokio 实现的跨平台 P2P 组网工具，提供加密通信、NAT 穿透、子网代理、流量中继、Magic DNS 和 WireGuard 门户。

## 构建

需要 Rust 1.95、Protobuf 编译器以及平台对应的 C/C++ 构建工具。

```bash
cargo build -p easytier --release --bins
```

生成 `easytier-core`（组网进程）和 `easytier-cli`（本地诊断与管理工具）。

## 配置与启动

所有 `easytier-core` 启动设置只从 TOML 文件读取。完整中文模板位于 [config.toml](config.toml)，包含网络、进程、日志、安全模式、ACL、端口转发等配置及可选示例。

```bash
# 将模板复制到可执行文件同级目录，然后修改网络名称、密钥和对端地址
cp config.toml target/release/config.toml
# 默认读取可执行文件旁的 config.toml，不依赖当前工作目录
sudo ./target/release/easytier-core
# 或显式指定文件
sudo ./target/release/easytier-core -c /etc/easytier/config.toml
# 只检查配置，不启动网络
./target/release/easytier-core -c /etc/easytier/config.toml --check-config
```

不再支持通过环境变量或网络/日志启动参数设置配置，也不支持 stdin 配置。文件中的 `$VAR`、`${VAR}` 和 `~` 按字面值读取。`-c` 的相对路径相对于启动工作目录；文件中的相对路径相对于该文件所在目录。文件缺失、无法读取或 TOML 无效时退出报错。

进程参数位于 `[core]`，日志位于 `[console_logger]` 和 `[file_logger]`。额外网络文件可通过 `[core].config_dir` 配置；只有主配置文件的进程和日志设置生效。Web 配置服务、远程 Web 配置客户端、桌面和移动 GUI 已移除。SDK 中的 TOML 文本接口继续用于嵌入式宿主。

## 两个节点组网

在节点 A 的配置文件中写入：

```toml
instance_name = "node-a"
ipv4 = "10.144.0.1/16"
listeners = ["tcp://0.0.0.0:11010", "udp://0.0.0.0:11010"]

[network_identity]
network_name = "my-network"
network_secret = "replace-with-a-long-random-secret"
```

节点 B 使用相同网络名称和密钥，将地址改为 `10.144.0.2/16`，并加入对端：

```toml
[[peer]]
uri = "udp://NODE_A_PUBLIC_IP:11010"
```

两台机器均使用 `easytier-core -c /path/to/config.toml` 启动。公共中继节点也通过 `[[peer]]` 指定，可重复添加。需要子网代理、静态路由或 WireGuard 门户时，参照模板对应章节。

```bash
# 检查本地运行状态
./target/release/easytier-cli peer
./target/release/easytier-cli route
ping 10.144.0.2
```

## 相关项目

- [ZeroTier](https://www.zerotier.com/)：用于连接设备的全球虚拟网络。
- [TailScale](https://tailscale.com/)：旨在简化网络配置的 VPN 解决方案。

### 联系我们

- 💬 **[Telegram 群组](https://t.me/easytier)**
- 👥 **QQ 群**
  - 一群 [949700262](https://qm.qq.com/q/wFoTUChqZW)
  - 二群 [837676408](https://qm.qq.com/q/4V33DrfgHe)
  - 三群 [957189589](https://qm.qq.com/q/YNyTQjwlai)

## 许可证

EasyTier 在 [LGPL-3.0](https://github.com/EasyTier/EasyTier/blob/main/LICENSE) 许可下发布。

## 使用规范

请仅将 EasyTier 用于合法用途，并遵守适用的法律法规。使用者有责任确保其已获授权连接和管理相关网络与设备。

## 赞助

本项目的 CDN 加速和安全防护由腾讯云 EdgeOne 赞助。

<p align="center">
<a href="https://edgeone.ai/?from=github" target="_blank">
<img src="assets/edgeone.png" width="200">
</a>
</p>

特别感谢 [浪浪云](https://langlangy.cn/?i26c5a5) 和 [雨云](https://www.rainyun.com/NjM0NzQ1_) 赞助我们的公共服务器。
也感谢 [林枫云](https://www.dkdun.cn/aff/RZGBFYSW) 对 EasyTier 的支持。

<p align="center">
<a href="https://langlangy.cn/?i26c5a5" target="_blank">
<img src="assets/langlang.png" width="200">
</a>
<a href="https://langlangy.cn/?i26c5a5" target="_blank">
<img src="assets/raincloud.png" width="200">
</a>
<a href="https://www.dkdun.cn/aff/RZGBFYSW" target="_blank">
<img src="assets/linfengyun.png" width="200" alt="林枫云">
</a>
</p>

如果您觉得 EasyTier 有帮助，请考虑赞助我们。软件开发和维护需要大量的时间和精力，您的赞助将帮助我们更好地维护和改进 EasyTier。

<p align="center">
<img src="assets/wechat.png" width="200">
<img src="assets/alipay.png" width="200">
</p>
