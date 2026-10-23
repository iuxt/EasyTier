#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
IMAGE="${IMAGE:-iuxt/easytier-server:latest}"
BUILDER="${BUILDER:-easytier-web-builder}"
PUSH=false

usage() {
    cat <<'EOF'
用法：./easytier-server/build.sh [--push] [--help]

将 Go 管理服务及内嵌 Web 界面构建为 Docker 镜像，无需本机安装 Go 或 Node.js。
默认构建 Docker 服务端所在架构的 Linux 镜像，并加载到本机 Docker，不推送。
--push 构建并推送镜像，默认支持 linux/amd64、linux/arm64。

选项：
  --push           推送到镜像仓库（请先 docker login）
  -h, --help       显示帮助

环境变量（仅构建使用，不读取 .env）：
  IMAGE            镜像名称，默认 iuxt/easytier-server:latest
  PLATFORMS        目标平台，支持 linux/amd64、linux/arm64；多平台需要 --push
  BUILDER          Buildx builder 名称，默认 easytier-web-builder（不存在时创建）

示例：
  ./easytier-server/build.sh
  IMAGE=iuxt/easytier-server:v1 PLATFORMS=linux/amd64 ./easytier-server/build.sh
  IMAGE=registry.example.com/team/easytier-server:v1 ./easytier-server/build.sh --push

部署及运行时环境变量见 easytier-server/docs/deployment.md。
EOF
}

die() { echo "错误：$*" >&2; exit 1; }

for arg in "$@"; do
    case "$arg" in
        --push) PUSH=true ;;
        -h|--help) usage; exit 0 ;;
        *) die "未知参数：${arg}（使用 --help 查看帮助）" ;;
    esac
done

command -v docker >/dev/null 2>&1 || die "未安装 Docker"
docker info >/dev/null 2>&1 || die "Docker 服务不可用，请先启动 Docker"
docker buildx version >/dev/null 2>&1 || die "未安装 Docker Buildx 插件"

if [[ -z "${PLATFORMS:-}" ]]; then
    if [[ "$PUSH" == true ]]; then
        PLATFORMS=linux/amd64,linux/arm64
    else
        PLATFORMS="linux/$(docker version --format '{{.Server.Arch}}')"
    fi
fi
[[ "$PLATFORMS" != ,* && "$PLATFORMS" != *, && "$PLATFORMS" != *,,* ]] \
    || die "PLATFORMS 必须是逗号分隔的平台列表"
IFS=',' read -r -a platforms <<< "$PLATFORMS"
for platform in "${platforms[@]}"; do
    case "$platform" in
        linux/amd64|linux/arm64) ;;
        *) die "不支持的平台：${platform}（支持 linux/amd64、linux/arm64）" ;;
    esac
done
if [[ "$PUSH" == false && ${#platforms[@]} -ne 1 ]]; then
    die "加载本机镜像仅支持一个平台，请指定 PLATFORMS=linux/amd64 或 linux/arm64；多平台使用 --push"
fi

if ! docker buildx inspect "$BUILDER" >/dev/null 2>&1; then
    docker buildx create --name "$BUILDER" --driver docker-container >/dev/null
fi
docker buildx inspect "$BUILDER" --bootstrap

build_args=(
    --builder "$BUILDER"
    --file "$ROOT_DIR/Dockerfile"
    --platform "$PLATFORMS"
    --tag "$IMAGE"
    --progress plain
)
if [[ "$PUSH" == true ]]; then
    echo "构建并推送 Web 管理服务镜像：$IMAGE ($PLATFORMS)"
    docker buildx build "${build_args[@]}" --push "$ROOT_DIR"
    echo "已推送镜像：$IMAGE"
else
    echo "构建并加载 Web 管理服务镜像：$IMAGE ($PLATFORMS)"
    docker buildx build "${build_args[@]}" --load "$ROOT_DIR"
    echo "镜像已加载到本机 Docker：$IMAGE"
fi
