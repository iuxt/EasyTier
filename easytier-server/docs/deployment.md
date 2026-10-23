# Web 管理服务：Docker 构建与部署

`easytier-server` 是 Go HTTP 服务，镜像内包含 Web 页面、API 和建表 SQL，不需要额外部署前端或安装 Node.js。数据库使用 MySQL 8。这里的镜像与仓库根目录 `build.sh` 构建的 Rust 客户端镜像相互独立。

## 1. 构建镜像

本机需要 Bash、Docker、Buildx；使用 Compose 部署时还需要 Docker Compose 插件。构建期间需要能够访问基础镜像仓库和 Go 模块下载源。

在仓库根目录执行，也可以进入 `easytier-server` 后执行 `./build.sh`；脚本会自动定位源码目录，从任意工作目录启动均可。

```bash
./easytier-server/build.sh
docker image inspect iuxt/easytier-server:latest
```

默认构建 Docker 服务端所在架构的 Linux 镜像并加载到本机 Docker，标签为 `iuxt/easytier-server:latest`。不会自动推送，也不会启动服务；`latest` 是本机构建的标签，不代表仓库已经发布此镜像。脚本创建并复用 `easytier-web-builder`，不修改默认 builder。

如果构建机与部署机架构不同，明确指定部署机平台：

```bash
IMAGE=iuxt/easytier-server:v1 PLATFORMS=linux/amd64 ./easytier-server/build.sh
```

推送到自己有权限的仓库，默认同时构建 `linux/amd64` 和 `linux/arm64`：

```bash
docker login registry.example.com
IMAGE=registry.example.com/team/easytier-server:v1 ./easytier-server/build.sh --push
# 只推送一种架构
IMAGE=registry.example.com/team/easytier-server:v1 PLATFORMS=linux/amd64 ./easytier-server/build.sh --push
```

Go 在构建机架构上交叉编译；Alpine 运行时阶段仍需要 builder 支持目标平台（原生节点或 QEMU）。遇到 `exec format error` 时检查 `docker buildx inspect easytier-web-builder --bootstrap` 的平台列表。

| 构建变量 | 默认值 | 说明 |
| --- | --- | --- |
| `IMAGE` | `iuxt/easytier-server:latest` | 镜像仓库、名称和标签 |
| `PLATFORMS` | 本机加载时为 Docker 服务端架构；推送时为 `linux/amd64,linux/arm64` | 支持这两种 Linux 平台；本机加载仅支持一个平台 |
| `BUILDER` | `easytier-web-builder` | 已存在则复用，不存在则创建 `docker-container` builder |

这些变量通过命令前缀或 `export` 设置。构建脚本不读取 `.env`，管理员密码、数据库密码和 token 只在运行时传入，不写入镜像。

无镜像仓库时，可以在构建后导出并复制到目标主机：

```bash
docker save -o easytier-server-v1.tar iuxt/easytier-server:v1
# 将 tar 文件传到目标主机后执行
docker load -i easytier-server-v1.tar
```

## 2. 使用 Compose 部署（自带 MySQL）

在部署机准备 `compose.yaml`、`.env.example`，放在同一目录。镜像可以在该主机构建，也可以按上一节推送或导入；只使用现有镜像时无需复制源码。

```bash
cd easytier-server
cp .env.example .env
chmod 600 .env
# 执行四次，将四个不同的结果分别填到 .env 的密码/token 字段
openssl rand -hex 32
```

编辑 `.env`，替换 `MYSQL_PASSWORD`、`MYSQL_ROOT_PASSWORD`、`ADMIN_PASSWORD`、`ENROLLMENT_TOKEN`。建议全部使用不同的随机十六进制字符串，避免 Compose 插值和 DSN 特殊字符问题。

本机 HTTP 试用设置 `COOKIE_SECURE=false`；生产 HTTPS 使用 `true`。`IMAGE` 填写实际构建或推送的完整标签，`HOST_BIND` 默认保持 `127.0.0.1`，`HTTP_PORT` 默认 `8080`。

```bash
# 若使用镜像仓库，在目标机登录后先拉取；本机已构建或导入镜像可跳过
docker compose pull server

# 使用现有镜像启动服务及 MySQL
docker compose up -d --no-build
docker compose ps
docker compose logs --tail=100 server
curl --fail http://127.0.0.1:8080/healthz
```

`/healthz` 返回 `{"status":"ok"}` 表示服务和 MySQL 连接正常；数据库不可用时返回 503。修改 `HTTP_PORT` 后，对应修改检测地址。首次初始化 MySQL 需要等待一段时间，Compose 会等 MySQL 健康检查通过再启动 Web 服务。

如果部署机已有源码，也可以直接 `docker compose up -d --build`，由 Compose 构建镜像。使用独立构建脚本生成的镜像时，使用 `--no-build`。

HTTP 试用通过浏览器访问 `http://127.0.0.1:8080`，输入 `ADMIN_PASSWORD` 登录。服务只使用密码，没有单独的用户名。远程试用可通过 SSH 转发：

```bash
ssh -L 8080:127.0.0.1:8080 user@部署机
```

然后在本机浏览器打开上述回环地址。远程客户端正式接入仍需要 HTTPS。

## 3. HTTPS 与对外访问

生产环境让域名（例如 `vpn.example.com`）解析到部署机，并在宿主机运行 Caddy 或 Nginx，将同一域名的根路径反向代理到 `127.0.0.1:8080`。Web、API、客户端都使用 `https://vpn.example.com`，不支持在 `/easytier/` 等子路径下部署。

宿主机 Caddy 的 `Caddyfile` 示例：

```caddyfile
vpn.example.com {
    reverse_proxy 127.0.0.1:8080 {
        header_up Host {host}
    }
}
```

确保 Caddy 已启动，域名正确解析，公网 TCP 80/443 能到达代理，以便签发证书及访问。此配置假设代理运行在宿主机；若代理也在容器中，应将其接入 Compose 网络并代理到 `server:8080`。

`.env` 中设置 `COOKIE_SECURE=true` 后执行 `docker compose up -d --no-build` 应用配置。服务端本身提供 HTTP，由代理终止 TLS；代理必须保留原始 Host，以通过登录及写入接口的同源校验。默认 Cookie 为 Secure、HttpOnly、SameSite=Strict。

默认只将 Web 端口绑定到宿主机回环地址，不发布 MySQL 端口。若需要更改绑定地址，通过 `HOST_BIND` 配置；管理服务无需 `/dev/net/tun`、`NET_ADMIN` 或 host 网络。它也不承担 VPN 中继；组网接入设备的 TCP/UDP 11010 需单独放通。

## 4. 环境变量

### 服务进程使用的变量

| 变量 | 必填 / 默认值 | 说明 |
| --- | --- | --- |
| `MYSQL_DSN` | 必填 | Go MySQL 驱动 DSN，例如 `easytier:密码@tcp(mysql:3306)/easytier?charset=utf8mb4`；Compose 根据数据库密码自动生成 |
| `ADMIN_PASSWORD` | 必填 | 管理员登录密码，至少 24 字节，必须与 `ENROLLMENT_TOKEN` 不同；建议使用 64 字符随机十六进制串 |
| `ENROLLMENT_TOKEN` | 必填 | 客户端首次注册时通过 `-k` 使用的共享 token，至少 24 字节；后续通信使用独立设备凭据 |
| `LISTEN_ADDR` | 二进制默认 `127.0.0.1:8080`，镜像默认 `0.0.0.0:8080` | 容器内部监听地址；现有 Compose 固定映射容器 8080，无需修改此变量 |
| `COOKIE_SECURE` | `true` | 只有精确设置为小写 `false` 才关闭 Secure Cookie；仅用于本机 HTTP 调试 |
| `LOG_RETENTION_DAYS` | `7` | 大于等于 1 的整数，服务每分钟分批删除过期设备日志 |

服务不会自动读取 `.env` 文件；Compose 将其中配置注入容器，直接 `docker run` 时需要 `--env-file` 或 `-e`。日志输出到标准输出/错误，可通过 `docker compose logs` 查看。

### Compose 使用的变量

| 变量 | 必填 / 默认值 | 说明 |
| --- | --- | --- |
| `MYSQL_PASSWORD` | 必填 | 内置 MySQL 的 `easytier` 用户密码，同时用于生成服务端 `MYSQL_DSN` |
| `MYSQL_ROOT_PASSWORD` | 必填 | 内置 MySQL root 密码，不用于管理服务的数据库连接 |
| `IMAGE` | `iuxt/easytier-server:latest` | Compose 构建和运行的镜像标签，与构建脚本的 `IMAGE` 保持一致 |
| `HOST_BIND` | `127.0.0.1` | 宿主机 Web 端口绑定地址 |
| `HTTP_PORT` | `8080` | 宿主机 Web 端口，容器内仍为 8080 |

Compose 中数据库名和普通用户名均为 `easytier`，MySQL 数据保存到项目命名卷 `mysql-data`。`ADMIN_PASSWORD`、`ENROLLMENT_TOKEN`、`COOKIE_SECURE`、`LOG_RETENTION_DAYS` 也从 `.env` 注入服务。

`MYSQL_PASSWORD` 和 `MYSQL_ROOT_PASSWORD` 只在 MySQL 数据卷首次初始化时设置数据库账户密码。对已有数据库仅修改 `.env` 不会修改数据库密码，必须先在数据库中同步修改账户密码，再更新配置并重建容器。修改 `.env` 后用 `docker compose up -d --no-build`，单纯 `restart` 不会更新容器环境变量。

## 5. 使用已有 MySQL（docker run）

先创建 `utf8mb4` 数据库和专用账号。程序会创建缺失的表、索引及外键，但不会创建数据库；账号需要该库的建表及数据读写等权限。

创建权限为 600 的环境文件（示例保存在仓库外），将占位内容替换为实际值：

```bash
mkdir -p "$HOME/.config/easytier-server"
chmod 700 "$HOME/.config/easytier-server"
```

`$HOME/.config/easytier-server/server.env` 内容：

```dotenv
MYSQL_DSN=easytier:替换为数据库密码@tcp(db.example.com:3306)/easytier?charset=utf8mb4&tls=true
ADMIN_PASSWORD=替换为独立随机管理员密码至少24字符
ENROLLMENT_TOKEN=替换为另一个独立随机注册token至少24字符
COOKIE_SECURE=true
LOG_RETENTION_DAYS=7
```

```bash
chmod 600 "$HOME/.config/easytier-server/server.env"
docker run -d --name easytier-server --restart unless-stopped \
  --env-file "$HOME/.config/easytier-server/server.env" \
  -p 127.0.0.1:8080:8080 \
  iuxt/easytier-server:latest
docker logs --tail=100 easytier-server
```

`docker run --env-file` 的值不要加包裹引号。容器里的 `127.0.0.1` 指容器自身，不能用来连接宿主机或另一容器的 MySQL；请填写容器可达的数据库地址。示例 `tls=true` 要求数据库配置受信任的 TLS 证书；按实际数据库的 TLS 配置填写 DSN。

MySQL 保存设备、审批、配置、共享组网身份和设备日志；Web 容器本身无需数据卷。管理员会话保存在内存，重启后需要重新登录。

## 6. 更新、备份和排错

生产环境建议使用固定版本标签。升级前备份数据库、保存 `.env`，然后构建或推送新标签，在部署机更新 `.env` 的 `IMAGE`：

```bash
docker compose pull server
docker compose up -d --no-build server
curl --fail http://127.0.0.1:8080/healthz
```

若镜像在本机构建或通过 tar 导入，跳过 `pull`。停止服务使用 `docker compose down`，会保留 MySQL 命名卷；`docker compose down -v` 会删除数据库卷及其数据，不用于常规升级。

备份内置 MySQL（在 Compose 目录执行，密码在容器内读取）：

```bash
docker compose exec -T mysql sh -c \
  'MYSQL_PWD="$MYSQL_ROOT_PASSWORD" exec mysqldump -uroot --single-transaction --no-tablespaces --databases easytier' \
  > easytier-backup.sql
```

备份包含设备凭据和组网密钥，应安全存储。恢复前先停止管理服务写入，再导入到已就绪的 MySQL：

```bash
docker compose stop server
docker compose exec -T mysql sh -c \
  'MYSQL_PWD="$MYSQL_ROOT_PASSWORD" exec mysql -uroot' < easytier-backup.sql
docker compose up -d --no-build server
```

| 现象 | 检查项 |
| --- | --- |
| 构建或拉取失败 | 检查镜像仓库、Go 模块源的网络连接，以及仓库登录权限 |
| `exec format error` | 确认构建平台与部署机一致；跨架构构建检查 builder 的目标平台支持 |
| MySQL 健康检查失败或服务循环重启 | `docker compose logs --tail=100 mysql server`；核对密码、数据库名、数据库网络和账号权限 |
| HTTP 下登录后仍未登录 | 本机调试设置 `COOKIE_SECURE=false`；生产通过 HTTPS 访问并保持 `true` |
| 写入接口返回同源校验错误 | 保持 Web、API 使用同一域名，代理保留原始 Host |
| 修改 `.env` 没生效 | 执行 `docker compose up -d --no-build` 重建受影响容器；确认 shell 中导出的同名变量没有覆盖 `.env` |
| 客户端无法接入 | 远程管理地址必须 HTTPS；使用注册 token 启动并在 Web 审批，组网接入节点还需放通 TCP/UDP 11010 |

客户端启动和 Web 功能说明见 [服务端 README](../README.md)。
