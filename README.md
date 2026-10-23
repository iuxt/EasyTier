# EasyTier

[简体中文](README_CN.md) | [English](README.md)

EasyTier is a cross-platform P2P network built with Rust and Tokio. It supports encryption, NAT traversal, subnet proxies, relaying, Magic DNS and WireGuard portals.

## Build

Install Rust 1.95, the Protocol Buffers compiler and platform C/C++ build tools.

```bash
cargo build -p easytier --release --bin easytier-core
```

This produces `target/release/easytier-core`, including both the network process and management commands. The Docker image and `build.sh` exports also contain only this executable.

The source workspace requires three directories: `easytier/` contains the native program, `easytier-core/` contains its portable networking library, and `easytier-proto/` contains the shared protocol and RPC types. Platform bindings, contrib tools, and Go/JavaScript SDKs have been removed.

`easytier-core` provides both networking and diagnostic management commands. Only this executable needs to be built and installed.

## Central management

The Go management server embeds its Web interface and stores devices, approvals, configurations and logs in MySQL 8. See the [server deployment guide](easytier-server/README.md).

```sh
cd easytier-server
cp .env.example .env
# Set independent database passwords, ADMIN_PASSWORD and ENROLLMENT_TOKEN.
# For local HTTP development only, set COOKIE_SECURE=false.
docker compose up -d --build
```

After configuring an HTTPS reverse proxy, start each client with:

```sh
sudo ./target/release/easytier-core -h https://vpn.example.com -k YOUR_ENROLLMENT_TOKEN
```

New devices wait for approval in the Web interface. An administrator selects settings with switches and approves the device before its network starts. Clients synchronize every 5 seconds and stop after rejection, revocation, or expiration of their 45-second authorization lease. Runtime logs are uploaded to the server; the client console displays brief lifecycle messages. Persist `~/.easytier/managed`, or choose a persistent directory with `--state-dir`, to preserve device identity across restarts.

Device settings use switches for automatic IP assignment, accepting initial connections, IPv6, direct connections and UDP hole punching. Select a connection node from the dropdown. The server generates and persists the shared network identity, manages listener addresses, and distributes peer lists automatically; no configuration file is shown in the Web interface.

## Connect two nodes

1. Start both clients with the management server URL and enrollment token.
2. Enable the connection-node switch for a reachable device A. If registering through a reverse proxy, enter a reachable hostname or IP in its address field, then approve it.
3. Select A as device B's connection node, or keep automatic selection, then approve B.
4. Keep automatic IP assignment enabled or use the form to assign distinct static internal addresses.

The connection node uses TCP/UDP port 11010 by default. The Go management service does not act as a VPN relay. View runtime state and logs in the Web interface. Use `--help` for help; `-h` selects the server. Local files selected with `-c` are retained only for developer validation with `--check-config`.

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
