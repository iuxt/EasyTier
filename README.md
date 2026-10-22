# EasyTier

[简体中文](README_CN.md) | [English](README.md)

EasyTier is a cross-platform P2P network built with Rust and Tokio. It supports encryption, NAT traversal, subnet proxies, relaying, Magic DNS and WireGuard portals.

## Build

Install Rust 1.95, the Protocol Buffers compiler and platform C/C++ build tools.

```bash
cargo build -p easytier --release --bins
```

This produces `easytier-core` and the local management tool `easytier-cli`.

## Configure and start

All core startup settings come from a TOML file. [config.toml](config.toml) is the complete template with Chinese comments, including optional network, logging, security, ACL and gateway settings.

```bash
cp config.toml target/release/config.toml
# Edit the network name, secret and peers before starting.
sudo ./target/release/easytier-core
# Select another file, or validate without starting networking.
sudo ./target/release/easytier-core -c /etc/easytier/config.toml
./target/release/easytier-core -c /etc/easytier/config.toml --check-config
```

The default is `config.toml` beside the executable, regardless of the working directory. Environment variables and network/logging startup arguments are no longer accepted. Stdin is not a configuration source. `$VAR`, `${VAR}` and `~` remain literal. A relative `-c` path uses the working directory; paths inside a file use that file's directory. Missing, unreadable or invalid files cause an error.

Process settings belong to `[core]`, with logging in `[console_logger]` and `[file_logger]`. `[core].config_dir` loads additional network files; only the main file supplies process and logging settings. The Web server, outbound Web configuration clients, desktop GUI and mobile GUI have been removed. SDKs retain TOML text interfaces for embedded hosts.

## Connect two nodes

Node A:

```toml
instance_name = "node-a"
ipv4 = "10.144.0.1/16"
listeners = ["tcp://0.0.0.0:11010", "udp://0.0.0.0:11010"]

[network_identity]
network_name = "my-network"
network_secret = "replace-with-a-long-random-secret"
```

On Node B, use the same identity, set `ipv4 = "10.144.0.2/16"`, and add:

```toml
[[peer]]
uri = "udp://NODE_A_PUBLIC_IP:11010"
```

Start both nodes with `easytier-core -c /path/to/config.toml`. Add further peers or public relay endpoints with additional `[[peer]]` sections. See the template for subnet proxies and WireGuard portals.

```bash
easytier-cli peer
easytier-cli route
ping 10.144.0.2
```

## Related Projects

- [ZeroTier](https://www.zerotier.com/): A global virtual network for connecting devices.
- [TailScale](https://tailscale.com/): A VPN solution aimed at simplifying network configuration.

### Contact Us

- 💬 **[Telegram Group](https://t.me/easytier)**
- 👥 **[QQ Group]**
  - No.1 [949700262](https://qm.qq.com/q/wFoTUChqZW)
  - No.2 [837676408](https://qm.qq.com/q/4V33DrfgHe)
  - No.3 [957189589](https://qm.qq.com/q/YNyTQjwlai)

## License

EasyTier is released under the [LGPL-3.0](https://github.com/EasyTier/EasyTier/blob/main/LICENSE).

## Responsible Use

Use EasyTier only for lawful purposes and in compliance with applicable laws
and regulations. You are responsible for ensuring that you are authorized to
connect to and administer the networks and devices involved.

## Sponsor

CDN acceleration and security protection for this project are sponsored by Tencent EdgeOne.

<p align="center">
  <a href="https://edgeone.ai/?from=github" target="_blank">
    <img src="assets/edgeone.png" width="200" alt="EdgeOne Logo">
  </a>
</p>

Special thanks to [Langlang Cloud](https://langlangy.cn/?i26c5a5)  and [RainCloud](https://www.rainyun.com/NjM0NzQ1_) for sponsoring our public servers.
We also thank [Linfeng Cloud (林枫云)](https://www.dkdun.cn/aff/RZGBFYSW) for supporting EasyTier.

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


If you find EasyTier helpful, please consider sponsoring us. Software development and maintenance require a lot of time and effort, and your sponsorship will help us better maintain and improve EasyTier.

<p align="center">
<img src="assets/wechat.png" width="200">
<img src="assets/alipay.png" width="200">
</p>
