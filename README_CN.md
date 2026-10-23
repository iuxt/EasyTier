# EasyTier

[简体中文](README_CN.md) | [English](README.md)

EasyTier 是一个使用 Rust 和 Tokio 实现的跨平台 P2P 组网工具，提供加密通信、NAT 穿透、子网代理、流量中继、Magic DNS 和 WireGuard 门户。

## 构建

需要 Rust 1.95、Protobuf 编译器以及平台对应的 C/C++ 构建工具。

```bash
cargo build -p easytier --release --bin easytier-core
```

生成 `target/release/easytier-core`（包含组网进程和诊断管理命令）。Docker 镜像和 `build.sh` 导出的文件也只包含这个程序。

源码工作区需要保留三个目录：`easytier/` 是原生程序，`easytier-core/` 是其可移植网络库，`easytier-proto/` 是共享协议及 RPC 类型。平台绑定、contrib 附加工具和 Go/JavaScript SDK 已移除。

`easytier-core` 同时提供组网和诊断管理命令，只需构建和安装这一个程序。

## 统一管理与启动

管理服务使用 Go 开发，内嵌 Web 界面，连接 MySQL 8 保存设备、审批、配置和日志。部署步骤和完整说明见 [服务端文档](easytier-server/README.md)。

```sh
cd easytier-server
cp .env.example .env
# 修改数据库密码、ADMIN_PASSWORD、ENROLLMENT_TOKEN
# 本机 HTTP 调试需设置 COOKIE_SECURE=false
docker compose up -d --build
```

配置 HTTPS 反向代理后，客户端只需：

```sh
sudo ./target/release/easytier-core -h https://vpn.example.com -k 你的注册token
```

客户端注册后在 Web 中显示为“等待审批”。管理员通过开关选择功能并点击“同意接入”后，客户端才启动网络。配置每 5 秒同步，支持在线修改、拒绝及撤销接入；失联超过 45 秒授权租约后停止网络。客户端终端只显示简短状态，运行日志集中到服务端查看。设备身份默认保存在 `~/.easytier/managed`，容器应通过 `--state-dir` 指定并持久化目录。

Web 直接提供自动分配内网 IP、作为接入节点、IPv6、设备直连、UDP 打洞等开关，并通过下拉框选择接入节点。组网密钥和监听地址由服务端自动管理，不需要查看或编辑配置文件。

## 两个节点组网

1. 两台设备均使用服务端地址和注册 token 启动，在 Web 中等待审批。
2. 为可达的设备 A 开启“作为接入节点”；如经过反向代理，在“接入地址”填写其他设备能访问的域名或 IP，然后同意接入。
3. 为设备 B 的“接入节点”选择设备 A（或保留自动选择），然后同意接入。
4. 两台设备均可开启自动分配内网 IP，也可关闭后填写不同的固定内网地址，再保存设置。

接入节点默认需要 TCP/UDP 11010 可达；管理服务本身不承担 VPN 中继。设备运行状态与日志统一从 Web 查看。客户端帮助使用 `--help`，`-h` 表示服务端地址；`-c` 仅用于开发时的离线配置校验。

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
