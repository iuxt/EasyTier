# Contributing to EasyTier

[中文](CONTRIBUTING_zh.md)

Install Rust 1.95, LLVM/Clang and the Protocol Buffers compiler. Linux network integration tests also need bridge-utils, iproute2 and network administration privileges. The Linux linker configuration uses mold.

```bash
cargo build -p easytier --bins
cargo fmt --all -- --check
cargo test -p easytier --test config_logging
```

`easytier-core/` owns the portable network library; `easytier/` owns native Core/CLI; `easytier-proto/` owns protocols. Platform bindings and tools live in `easytier-contrib/`, with Go and JavaScript hosts in `easytier-go/` and `easytier-js/`.

Copy the root `config.toml` template beside the executable or select it with `-c FILE.toml`. All native Core startup configuration comes from files.

## Development Workflow

1. Create a feature branch from `develop`:
   ```bash
   git checkout develop
   git checkout -b feature/your-feature-name
   ```

2. Make your changes following our coding standards

3. Write or update tests as needed

4. Use conventional commit messages:
   ```
   feat: add new feature
   fix: resolve bug
   docs: update documentation
   test: add tests
   chore: update dependencies
   ```

5. Submit a pull request to `develop`

## Testing Guidelines

### Running Tests

```bash
# Configure system (Linux)
sudo modprobe br_netfilter
sudo sysctl net.bridge.bridge-nf-call-iptables=0
sudo sysctl net.bridge.bridge-nf-call-ip6tables=0

# Run tests
cargo test --no-default-features --features=full --verbose
```

### Test Requirements

- Write tests for new features
- Maintain existing test coverage
- Tests should be isolated and repeatable
- Include both unit and integration tests

## Pull Request Guidelines

1. Target the `develop` branch
2. Ensure all tests pass
3. Include clear description and purpose
4. Reference related issues
5. Keep changes focused and atomic
6. Update documentation as needed

## Additional Resources

- [Issue Tracker](https://github.com/EasyTier/EasyTier/issues)
- [Project Documentation](https://github.com/EasyTier/EasyTier/wiki)

## Questions or Need Help?

Feel free to:
- Open an issue for questions
- Join our community discussions
- Reach out to maintainers

Thank you for contributing to EasyTier! 
