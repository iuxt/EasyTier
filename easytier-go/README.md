# EasyTier Go

`easytier-go` runs the `wasm32-wasip1` build of `easytier-core` in a
pure-Go process through wazero. EasyTier remains the source and producer of the
embedded WASM; this repository adapts Go host capabilities to the ABI exported
and imported by that artifact.

```go
import (
    "net/netip"

    corehost "github.com/easytier/easytier/easytier-go"
)
```

## Public API

The public package owns wazero, standard WASI, the EasyTier host ABI, guest
driving, completion notification, and resource shutdown. Applications create a
host, build a typed instance configuration, and then use standard Go network
interfaces:

```go
host, err := corehost.New(ctx, corehost.Options{})
if err != nil {
    return err
}
defer host.Close(ctx)

config, err := corehost.NewInstanceConfigBuilder("office").
    NetworkSecret("secret").
    IPv4(netip.MustParsePrefix("10.144.0.10/24")).
    AddPeers("tcp://198.51.100.10:11010").
    Build()
if err != nil {
    return err
}

instance, err := host.CreateInstance(ctx, config)
if err != nil {
    return err
}
defer instance.Close(ctx)

if err := instance.Start(ctx); err != nil {
    return err
}
if err := instance.SendPacket(ctx, packet); err != nil {
    return err
}
received, err := instance.ReceivePacket(ctx)

listener, err := instance.Listen("tcp4", ":8080")
connection, err := instance.Dial(ctx, "tcp4", "10.144.0.2:8080")
packets, err := instance.ListenPacket("udp4", ":5353")
```

`CreateInstanceTOML` loads a native EasyTier TOML document. Pass an empty
`instanceID` to allocate a UUID. Existing `instance_id` and `instance_name`
keys in the document are replaced by the host.

```go
instance, err := host.CreateInstanceTOML(ctx, "office", "", configTOML)
```

`Instance.ShowNodeInfo` returns this instance's virtual IPv4 address and
advertised hostname.

### Logging

Logging belongs to the Host and is independent of instance TOML and lifecycle.
Pass an `io.Writer` when creating the Host to receive guest logs; nil discards
them. The default level is `LogLevelInfo`. Change it before creating instances
or at runtime through the Host:

```go
host, err := corehost.New(ctx, corehost.Options{LogWriter: os.Stdout})
if err != nil {
    return err
}
defer host.Close(ctx)
if err := host.SetLogLevel(ctx, corehost.LogLevelDebug); err != nil {
    return err
}
```

`host.LogLevel(ctx)` reads the current level. `LogLevelOff` disables all logs.
The level applies to every instance in this Host; other
Hosts are independent. level. Creating or closing an instance never resets it.

Writes are synchronous, so the writer must not call back into this Host.
Writer errors do not interrupt guest execution, and the Host does not close
the writer.


## Cross-platform TUN example

The TUN example joins an existing EasyTier network with a fixed virtual IPv4
address on Linux, macOS, or Windows. It creates and configures the native TUN
interface itself, then forwards raw IPv4 packets through `SendPacket` and
`ReceivePacket`:

```sh
cd examples/tun
sudo go run . \
  -p tcp://198.51.100.10:11010 \
  --network-name office \
  --network-secret secret \
  --ipv4 10.144.0.10/24
```

Repeat `-p` to configure more peers. The command creates `et-goN` on Linux and
Windows or `utunN` on macOS, assigns the requested address, and sets an MTU of
1380. Run it as root or with `CAP_NET_ADMIN` on Linux, with `sudo` on macOS, or
from an Administrator terminal on Windows. Closing the command removes the TUN
interface. The example does not install a default route or enable GSO.

On Linux and macOS, send `SIGUSR1` to print the current peer list or `SIGUSR2`
to print the current route list.

Repeat `-port-forward` to expose local TCP or UDP ports through the embedded
core's port-forward manager:

```sh
sudo go run . \
  -p tcp://198.51.100.10:11010 \
  --network-name office \
  --network-secret secret \
  --ipv4 10.144.0.10/24 \
  -port-forward tcp://127.0.0.1:5202/10.144.0.20:5201 \
  -port-forward udp://127.0.0.1:5202/10.144.0.20:5201
```

For example, run `iperf3 -c 127.0.0.1 -p 5202` for TCP or add
`-u -b 0 -l 1200` for UDP. iperf3's UDP mode still needs the TCP forward for
its control connection. The TUN example only parses these rules into the
instance configuration; the core owns the host listeners and per-client
overlay flows.

## Instance.Dial example

The Dial example is a small overlay client dedicated to the public
`Instance.Dial` API. TCP mode bridges the connected stream to standard input
and output until the remote side closes or the command is interrupted:

```sh
printf 'GET / HTTP/1.0\r\nHost: 10.144.0.20\r\n\r\n' |
  go run ./examples/dial \
    -p tcp://198.51.100.10:11010 \
    --network-name office \
    --network-secret secret \
    --ipv4 10.144.0.10/24 \
    --network tcp4 \
    --address 10.144.0.20:8080
```

With `--network udp4`, standard input is sent as one datagram and one response
datagram is written to standard output. The command does not create a local
listener or implement port-forward management. It waits up to 10 seconds for a
matching overlay or proxy route before dialing; override that limit with
`--connect-timeout`.


## Performance compared with native EasyTier

In this A/B benchmark two nodes on one i7-14700KF host (Linux 6.11) each run
in their own network namespace, joined by a veth pair. Node A always runs a
native `easytier-core` build of EasyTier master (`2.6.4-6a186167`) with
overlay address `10.144.0.1/24`; node B runs either the same native binary or
this Go host (commit `78889d12`, embedded EasyTier `af640d49`) with
`10.144.0.2/24`. A master build is used as the native baseline because the
2.6.4 release predates several native data-plane throughput fixes (EasyTier
#2451, #2452). The underlay tunnel between the nodes is either `tcp://` or
`udp://`. Encryption is enabled and the overlay MTU is 1360 on both ends.
Node A and the iperf3 server are pinned to CPUs `0,2,4,6`, node B to
`8,10,12,14`. Each iperf3 run lasts 15 seconds and excludes the first
3 seconds. Forward means node B sends to node A; reverse uses `iperf3 -R`.
Measured 2026-07-28.

The forwarding rows deliberately compare native Core port forwarding with the
Go host's benchmark-only `cmd/dial-forward-bench`, which carries traffic
through the public `Instance.Dial` API.

TCP, one stream:

| Scenario | Direction | `tcp://` native | `tcp://` Go host | `udp://` native | `udp://` Go host |
| --- | --- | ---: | ---: | ---: | ---: |
| TUN | forward | 6.06 Gbit/s | 2.12 Gbit/s | 3.71 Gbit/s | 1.57 Gbit/s |
| TUN | reverse | 6.03 Gbit/s | 2.57 Gbit/s | 3.69 Gbit/s | 1.48 Gbit/s |
| Native port forward / Go Dial | forward | 1.25 Gbit/s | 1.29 Gbit/s | 1.19 Gbit/s | 1.20 Gbit/s |
| Native port forward / Go Dial | reverse | 6.49 Gbit/s | 1.95 Gbit/s | 4.26 Gbit/s | 1.43 Gbit/s |

UDP native port forward / Go Dial, 1 Gbit/s offered with 1200-byte datagrams
(received / lost):

| Direction | `tcp://` native | `tcp://` Go host | `udp://` native | `udp://` Go host |
| --- | ---: | ---: | ---: | ---: |
| forward | 996 Mbit/s / 0.3% | 546 Mbit/s / 45% | 996 Mbit/s / 0.3% | 699 Mbit/s / 30% |
| reverse | 950 Mbit/s / 5.0% | 490 Mbit/s / 51% | 983 Mbit/s / 1.7% | 350 Mbit/s / 65% |

Reading the numbers:

- On TUN the Go host reaches roughly 35-45% of native single-stream
  throughput. Both sides run the same EasyTier core logic, so the gap is the
  WASM/Go data-plane boundary rather than routing or cryptography.
- TCP forwarding is a tie at about 1.2 Gbit/s: both paths are bounded by the
  virtual TCP send path inside the shared EasyTier core, not by the host.
- TCP reverse forwarding favors native by about 3x (4.3-6.5 versus
  1.4-2.0 Gbit/s); the Go host benchmark's per-operation receive path is the
  limit.
- Native sustains the offered 1 Gbit/s UDP nearly loss-free in both
  directions, while the Go host saturates at 350-700 Mbit/s with significant
  loss, consistent with the one-operation-per-datagram data-plane ABI
  documented in `PERFORMANCE.md`.

## Platform capabilities

The default platform implementation uses Go's standard `net` and
`net.Resolver` packages. Applications that need netns, socket marks, device
binding, reuse policy, or custom DNS can inject capabilities through
`platform.Services`:

```go
host, err := corehost.New(ctx, corehost.Options{
    Platform: platform.Services{
        Sockets:     socketFactory,
        DNS:         dnsResolver,
        Environment: connectorEnvironment,
        Snapshot:    environmentSnapshot,
    },
})
```

`platform.SocketFactory` owns only TCP connect, UDP bind, and TCP listen
creation. Once a standard Go network resource is returned, the host runtime
owns its reads, writes, accepts, cancellation, and close path. EasyTier retains
all routing, peer admission, protocol, retry, and connection policy.

The implementation is split by responsibility:

- `platform` defines public capability ports; `platform/netstd` implements
  their portable defaults.
- `proto` contains generated Go bindings for the existing EasyTier management
  protobuf definitions.
- `internal/reactor` owns typed asynchronous operations, resources, operation
  IDs, backpressure, and completion signals without depending on wazero.
- `internal/hostabi` implements the custom `easytier_host` imports, guest
  memory copying, wire codecs, and ABI status translation.
- `internal/coreabi` owns guest memory, the big-endian data-plane wire codec,
  ABI discovery, and typed `easytier_instance_*`, `easytier_data_plane_*`, and
  `easytier_rpc_*` export calls.
- `internal/engine` composes standard WASI, both EasyTier ABI directions, the
  single-owner driver, operation cancellation, deadlines, standard Go network
  resources, and instance shutdown.
- `internal/artifact` contains only the embedded core and its provenance.

## Embedded artifact

The committed WASM lets downstream Go builds and tests run without a Rust
toolchain. Refresh it from a clean EasyTier checkout whenever the guest ABI or
core implementation changes:

```sh
EASYTIER_SOURCE=/path/to/EasyTier go generate ./...
```

Generation runs EasyTier's `script/build-wasi-core.sh`, which builds release
`easytier_core.wasm` with the Go-host features and writes an optimized
`easytier_core_go_host.wasm` with the pinned, SHA-256-verified Binaryen release.
The generator supplies a fixed source path remap and source-date epoch, then
records the EasyTier commit and optimized artifact SHA-256. Tracked EasyTier
changes block generation; unrelated untracked files do not.
`corehost.CoreInfo()` exposes that provenance without exposing the artifact
bytes.

The same generator rebuilds the Go protobuf bindings from that exact clean
EasyTier commit and records their source commit and schema SHA-256. Host
creation rejects an artifact/binding commit mismatch. Generation requires
`protoc` 35.1 and `protoc-gen-go` 1.36.11 on `PATH`.

The test-only socket probe is retained from
[EasyTier commit 6a3d15f](https://github.com/EasyTier/EasyTier/tree/6a3d15f8758eed759d55401ff4ed7c47021b0819/tools/wasi-socket-poc/guest);
its full commit and checksum are recorded in
`testdata/wasi_socket_guest.source`.

Run all reactor, ABI conformance, lifecycle, and two-instance network tests
with:

```sh
go test -count=1 ./...
```
