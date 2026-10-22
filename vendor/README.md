# Local dependency sources

EasyTier's Git dependencies are stored here as ordinary files. Cargo uses
`path` dependencies, so building the workspace does not clone or fetch these
repositories. No submodule initialization or Git LFS download is required.

The snapshots match the commits previously recorded in the root `Cargo.lock`.
Repository URLs and full commit IDs are recorded in [sources.json](sources.json).
Upstream license files are retained in each source directory.

| Directory | Upstream repository | Commit |
| --- | --- | --- |
| `kcp-sys` | `EasyTier/kcp-sys` | `268533568d734ae89dc89603078da3ca522effe1` |
| `rust-tun` | `EasyTier/rust-tun` | `5ba6be073e5b486b68c12f80fac5cfb50601f024` |
| `service-manager-rs` | `EasyTier/service-manager-rs` | `5eb28f7a686858eea4f4933534ed989d3b71dc2a` |
| `thunk` | `EasyTier/thunk` | `cbbeec75a66b7b3cf0824ae890d9d06bcfb9d1f3` |
| `tokio-websockets` | `EasyTier/tokio-websockets` | `dc9771c7c215882349c3cb328877550a3593df21` |
| `windivert-rust` | `EasyTier/windivert-rust` | `adcc56d1550f7b5377ec2b3429f413ee24a77375` |
| `kcp-sys-legacy` | `EasyTier/kcp-sys` | `d7427c22d764deb1860a7d37acc446ed5033464c` |

The original Git submodules are also stored as ordinary source files:

- Both KCP snapshots include `skywind3000/kcp` at
  `32da082e529a26730aea3eb19922f80634ee6dea` under `kcp/`.
- `windivert-rust/windivert-sys/vendor` contains `EasyTier/WinDivert` at
  `b90fad29446667c6230accd078e91a77074b7788`.

## Local adaptations

- The root workspace excludes these source trees from its members. The upstream
  workspaces, examples and tests remain available without becoming EasyTier
  workspace packages.
- KCP's compatibility-test dependency uses `../kcp-sys-legacy` instead of Git.
  The legacy package is named `kcp-sys-legacy` to distinguish the two local
  packages with otherwise identical names and versions.
- Both KCP submodule `.gitignore` files explicitly retain `LICENSE` so a normal
  `git add vendor` includes the license files.
- `thunk/thunk-rs` uses bundled Windows compatibility libraries instead of
  running `curl` and `7z`. It retains the `VC_LTL` and `YY_THUNKS` local directory
  overrides and reports an error if a required library is missing. The existing
  `subsystem_console` code path is declared as a Cargo feature.
- Docker's build context includes `vendor/`, and CI build/test path filters
  include changes under this directory. CI formatting checks select the three
  EasyTier workspace packages explicitly to preserve upstream source formatting.

## Windows compatibility libraries

`windows-compat/` contains VC-LTL 5.2.2 libraries and YY-Thunks 1.1.7 objects
for the project's Windows 7 compatibility configuration on x86 and x86_64.
The libraries and object files are extracted from the upstream release assets;
their URLs and archive SHA-256 hashes are recorded in `sources.json`.
PDB debug symbols and libraries for other target versions are omitted.

The upstream licenses are included alongside each set of libraries. To select
another Windows compatibility feature, provide the matching local installation
using `VC_LTL` and `YY_THUNKS`; the build never downloads missing libraries.

## Build and update

Build normally with `cargo build --locked`. With the Rust toolchain, protoc,
native build tools and registry dependencies already installed or cached, use:

```sh
cargo build --offline --locked -p easytier --bin easytier-core
```

Only the Git source dependencies and thunk's compatibility libraries are
bundled here. crates.io packages still use the registry; CI Actions, toolchain
setup, protoc, UPX and container images keep their existing distribution paths.

To update a snapshot, export the chosen upstream commit and its recursive
submodule contents into the corresponding directory, preserving licenses.
Reapply the local adaptations above and update `sources.json`. Refresh the root
lockfile if dependency requirements changed, then run the offline build once
the registry dependencies are cached. Do not reintroduce Git dependencies in
the manifests or `Cargo.lock`.
