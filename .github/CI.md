# CI

Every push to `main` runs the full Core build matrix and automatically publishes
the completed artifacts to this repository's Releases. Tags use
`v<package-version>-build.<run-number>`, for example `v2.7.0-build.42`.
Each release contains platform ZIPs and `SHA256SUMS`, and is marked Latest if
its commit is still the current main HEAD. Uploads complete before publication;
failed uploads leave a resumable draft. No existing version release, manual tag,
SignPath secret or personal access token is required for push releases.
Consecutive pushes cancel unfinished older builds on the same branch.

PRs and development-branch pushes keep their compilation checks. Manual and
`releases/**` builds remain available. See [the Chinese guide](CI.zh-CN.md) for
the release contents and signing options.

Core checks Linux, Windows and macOS on PRs and development-branch pushes.
Main pushes, manual dispatch and release branches build all configured native
targets and package `easytier-core` with the Chinese config.toml template.

Test runs CI script tests, formatting, clippy, WASI, lockfile validation and network tests. Network integration requires Linux namespaces, bridges and appropriate privileges.

Nightly compares main HEAD with the latest successful manual Core and Test builds, dispatches missing builds, retries failures once and waits for matching commits. It updates the rolling nightly prerelease and SHA256SUMS. The version comes from easytier/Cargo.toml.

Release accepts a successful full Core run ID, downloads its artifacts and creates a draft release. Web, desktop GUI and mobile GUI workflows are removed.

OHOS, Go, JavaScript Hosts and Nix use independent path filters. Docker consumes artifacts from a selected Core build.
