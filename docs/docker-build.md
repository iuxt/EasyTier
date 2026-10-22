# 构建可执行文件和多架构容器镜像

在仓库根目录执行，脚本始终构建当前工作区的源码（包括未提交的修改）：

```bash
docker login -u iuxt
./build.sh
```

默认生成 Linux `amd64`、`arm64` 两种架构的 release 可执行文件，并将多架构镜像推送到 Docker Hub 的 `iuxt/easytier:latest`。本机只需要 Bash、Docker 和 Buildx，不需要安装 Rust 或 protoc。脚本会创建并复用名为 `easytier-builder` 的 `docker-container` builder，不改变当前默认 builder。

构建过程在对应目标架构的 Debian 容器中执行，跨架构时需要 builder 支持 QEMU 或配置对应架构的原生节点；模拟编译 Rust 可能耗时较长。若遇到 `exec format error`，检查 `docker buildx inspect easytier-builder --bootstrap` 的平台列表。Docker Desktop 通常已经配置好模拟支持；Linux 主机可按 [Docker 多平台构建文档](https://docs.docker.com/build/building/multi-platform/)配置 QEMU。

产物目录：

```text
target/dist/
├── linux_amd64/
│   ├── easytier-core
│   └── easytier-cli
└── linux_arm64/
    ├── easytier-core
    └── easytier-cli
```

这些可执行文件使用 GNU libc，适用于 glibc 2.36 及以上的对应架构 Linux，例如 Debian 12；macOS、Windows 和使用 musl 的 Alpine 不能直接运行。镜像已包含所需运行库。

```bash
# 只构建可执行文件
./build.sh --binaries-only

# 构建可执行文件及镜像，不推送；镜像保存为 target/dist/easytier-image.oci.tar
./build.sh --no-push

# 只构建并推送镜像
./build.sh --image-only

# 指定平台、输出路径、镜像标签或已有 builder
PLATFORMS=linux/arm64 OUTPUT_DIR=./target/arm64 ./build.sh --binaries-only
IMAGE=iuxt/easytier:v2.7.0 BUILDER=my-builder ./build.sh

./build.sh --help
```

二进制和镜像使用相同的构建阶段与缓存。第二次构建运行时镜像会复用第一次编译的结果。`--no-push` 输出的是包含多架构清单的 OCI 归档，不会自动加载到本机镜像列表。

镜像不包含本地 `config.toml`。先修改仓库的配置模板，再挂载启动（以下是 Linux 主机示例）：

```bash
docker run -d --name easytier --restart unless-stopped \
  --network host \
  --cap-add NET_ADMIN \
  --device /dev/net/tun \
  -v "$PWD/config.toml:/etc/easytier/config.toml:ro" \
  iuxt/easytier:latest

docker exec easytier easytier-cli peer
```

镜像默认执行 `easytier-core -c /etc/easytier/config.toml`，可在镜像名后追加参数以覆盖默认参数。脚本不保存密码，使用 `docker login` 管理的登录凭据；默认运行会更新 Docker Hub 上的 `latest` 标签。
