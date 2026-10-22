#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
IMAGE="${IMAGE:-iuxt/easytier:latest}"
PLATFORMS="${PLATFORMS:-linux/amd64,linux/arm64}"
OUTPUT_DIR="${OUTPUT_DIR:-$ROOT_DIR/target/dist}"
BUILDER="${BUILDER:-easytier-builder}"
RUST_VERSION="${RUST_VERSION:-$(sed -n 's/^channel = "\([^"]*\)"/\1/p' "$ROOT_DIR/rust-toolchain.toml")}"
BUILD_BINARIES=true
BUILD_IMAGE=true
PUSH=true

usage() {
    cat <<'EOF'
用法：./build.sh [选项]

默认从当前源码构建 Linux amd64/arm64 可执行文件和镜像，推送 iuxt/easytier:latest。
需要 Docker、Buildx，以及支持目标架构的 builder（本机或 QEMU）。
推送前请运行 docker login -u iuxt。

选项：
  --binaries-only   仅导出可执行文件，不构建或推送运行时镜像
  --image-only      仅构建并推送镜像，不导出可执行文件
  --no-push         构建可执行文件并将镜像导出为 OCI 归档，不推送
  -h, --help        显示帮助

环境变量：
  IMAGE            镜像名，默认 iuxt/easytier:latest
  PLATFORMS        目标平台，默认 linux/amd64,linux/arm64
  OUTPUT_DIR       输出目录，默认仓库下 target/dist
  BUILDER          Buildx builder 名称，默认 easytier-builder（不存在时创建）
  RUST_VERSION     Rust 版本，默认读取 rust-toolchain.toml

示例：
  ./build.sh
  ./build.sh --no-push
  PLATFORMS=linux/arm64 ./build.sh --binaries-only
  IMAGE=iuxt/easytier:v2.7.0 ./build.sh --image-only

二进制输出：$OUTPUT_DIR/linux_amd64/easytier-core
            $OUTPUT_DIR/linux_arm64/easytier-core
不推送时：  $OUTPUT_DIR/easytier-image.oci.tar
EOF
}

die() { echo "错误：$*" >&2; exit 1; }

for arg in "$@"; do
    case "$arg" in
        --binaries-only) BUILD_IMAGE=false ;;
        --image-only) BUILD_BINARIES=false ;;
        --no-push) PUSH=false ;;
        -h|--help) usage; exit 0 ;;
        *) die "未知参数：${arg}（使用 --help 查看帮助）" ;;
    esac
done

if [[ "$BUILD_BINARIES" == false && "$BUILD_IMAGE" == false ]]; then
    die "--binaries-only 和 --image-only 不能同时使用"
fi
[[ -n "$RUST_VERSION" ]] || die "无法读取 Rust 版本，请设置 RUST_VERSION"
# Buildx --output uses comma-separated CSV fields.
[[ "$OUTPUT_DIR" != *,* ]] || die "OUTPUT_DIR 不能包含逗号"

IFS=',' read -r -a platforms <<< "$PLATFORMS"
for platform in "${platforms[@]}"; do
    case "$platform" in
        linux/amd64|linux/arm64) ;;
        *) die "不支持的平台：${platform}（支持 linux/amd64、linux/arm64）" ;;
    esac
done
[[ "$PLATFORMS" != ,* && "$PLATFORMS" != *, && "$PLATFORMS" != *,,* ]] \
    || die "PLATFORMS 必须是逗号分隔的平台列表"

command -v docker >/dev/null 2>&1 || die "未安装 Docker"
docker info >/dev/null 2>&1 || die "Docker 服务不可用，请先启动 Docker"
docker buildx version >/dev/null 2>&1 || die "未安装 Docker Buildx 插件"

if ! docker buildx inspect "$BUILDER" >/dev/null 2>&1; then
    docker buildx create --name "$BUILDER" --driver docker-container >/dev/null
fi
docker buildx inspect "$BUILDER" --bootstrap

mkdir -p "$OUTPUT_DIR"
OUTPUT_DIR="$(cd -- "$OUTPUT_DIR" && pwd)"
build_args=(
    --builder "$BUILDER"
    --file "$ROOT_DIR/Dockerfile"
    --platform "$PLATFORMS"
    --build-arg "RUST_VERSION=$RUST_VERSION"
    --progress plain
)

if [[ "$BUILD_BINARIES" == true ]]; then
    echo "构建并导出可执行文件：$PLATFORMS -> $OUTPUT_DIR"
    docker buildx build "${build_args[@]}" \
        --target binaries \
        --output "type=local,dest=$OUTPUT_DIR,platform-split=true" \
        "$ROOT_DIR"
fi

if [[ "$BUILD_IMAGE" == true ]]; then
    if [[ "$PUSH" == true ]]; then
        echo "构建并推送多架构镜像：$IMAGE ($PLATFORMS)"
        docker buildx build "${build_args[@]}" \
            --target runtime --tag "$IMAGE" --push "$ROOT_DIR"
    else
        echo "构建多架构镜像并导出 OCI 归档：$OUTPUT_DIR/easytier-image.oci.tar"
        docker buildx build "${build_args[@]}" \
            --target runtime --tag "$IMAGE" \
            --output "type=oci,dest=$OUTPUT_DIR/easytier-image.oci.tar" \
            "$ROOT_DIR"
    fi
fi

echo "构建完成。"
if [[ "$BUILD_BINARIES" == true ]]; then
    echo "可执行文件（Linux，要求 glibc >= 2.36）：$OUTPUT_DIR/linux_*"
fi
if [[ "$BUILD_IMAGE" == true && "$PUSH" == true ]]; then
    echo "已推送镜像：$IMAGE"
fi
