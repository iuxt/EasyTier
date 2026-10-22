# CI

Core checks Linux, Windows and macOS on ordinary PRs and pushes. Manual dispatch and release branches build all native targets and package Core, CLI, the Chinese config.toml template and Magisk.

Test runs CI script tests, formatting, clippy, WASI, lockfile validation and network tests. Network integration requires Linux namespaces, bridges and appropriate privileges.

Nightly compares main HEAD with the latest successful manual Core and Test builds, dispatches missing builds, retries failures once and waits for matching commits. It updates the rolling nightly prerelease and SHA256SUMS. The version comes from easytier/Cargo.toml.

Release accepts a successful full Core run ID, downloads its artifacts and creates a draft release. Web, desktop GUI and mobile GUI workflows are removed.

OHOS, Go, JavaScript Hosts and Nix use independent path filters. Docker consumes artifacts from a selected Core build.
