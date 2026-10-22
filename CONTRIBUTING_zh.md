# EasyTier 贡献指南

[English](CONTRIBUTING.md)

需要 Rust 1.95、LLVM/Clang 和 Protobuf 编译器。Linux 网络集成测试还需要 bridge-utils、iproute2 和网络管理权限；`.cargo/config.toml` 使用 mold 链接器。

```bash
cargo build -p easytier --bins
cargo fmt --all -- --check
cargo test -p easytier --test config_logging
```

项目结构：`easytier-core/` 为可移植网络库，`easytier/` 为原生 Core/CLI，`easytier-proto/` 为协议，`easytier-contrib/` 为各平台绑定及工具，`easytier-go/`、`easytier-js/` 为宿主集成。

运行前将根目录 `config.toml` 模板复制到可执行文件旁，或通过 `-c FILE.toml` 指定。所有 Core 启动配置只从文件读取。

## 开发工作流

1. 从 `develop` 分支创建特性分支：
   ```bash
   git checkout develop
   git checkout -b feature/your-feature-name
   ```

2. 按照代码规范进行修改

3. 编写或更新测试

4. 使用规范的提交信息：
   ```
   feat: 添加新功能
   fix: 修复问题
   docs: 更新文档
   test: 添加测试
   chore: 更新依赖
   ```

5. 提交 Pull Request 到 `develop` 分支

## 测试指南

### 运行测试

```bash
# 配置系统（Linux）
sudo modprobe br_netfilter
sudo sysctl net.bridge.bridge-nf-call-iptables=0
sudo sysctl net.bridge.bridge-nf-call-ip6tables=0

# 运行测试
cargo test --no-default-features --features=full --verbose
```

### 测试要求

- 为新功能编写测试
- 维护现有测试覆盖率
- 测试应该是独立且可重复的
- 包含单元测试和集成测试

## Pull Request 规范

1. 目标分支为 `develop`
2. 确保所有测试通过
3. 包含清晰的描述和目的
4. 关联相关的 issues
5. 保持变更的原子性和聚焦性
6. 及时更新相关文档

## 其他资源

- [问题追踪](https://github.com/EasyTier/EasyTier/issues)
- [项目文档](https://github.com/EasyTier/EasyTier/wiki)

## 需要帮助？

欢迎：
- 提出问题
- 参与社区讨论
- 联系维护者

感谢您为 EasyTier 做出贡献！ 