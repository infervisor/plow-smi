# plows-exporter

Prometheus metrics exporter backed by **`plows-gpu`** (NVML / amdgpu sysfs / Level Zero at runtime).

```bash
cargo run -p plows-exporter -- --all
# http://0.0.0.0:9835/metrics
```

Flags: `--nvidia`, `--amd`, `--intel`, `--apple`, `--system`, `--tpu`, `--network`,
`--wireguard`, `--probe <IPV4>`, `--probe-wireguard-peers`, `--probe-interval-ms`,
`--network-include <GLOB>`, `--all`, `--port`, `--bind`, `--interval`.

`--all` turns on every collector except probes. Probes send traffic, so they
run only when `--probe` or `--probe-wireguard-peers` asks for them.

## Network, WireGuard and probes

These three collectors build their output from the latest reading on each
scrape. Counters hold the kernel's absolute value, so `rate()` survives
restarts, and an interface, peer or target that goes away leaves the output.

### `--network` (Linux and macOS)

Read from `/sys/class/net` on Linux and the native `NET_RT_IFLIST2` routing
sysctl on macOS at scrape time. Both report absolute kernel counters; macOS
uses 64-bit byte/packet totals and requires no root privileges.

`plower0`, `inferos*`, and macOS `utun*` tunnel interfaces are included by
default, along with physical interfaces. Collection is enabled with no
collector flags, `--all`, or `--network`. When choosing specific collectors
(for example `--apple --system`), add `--network` for these metrics.

Skipped unless named by `--network-include`: `lo`, `lo0`, `veth*`, `docker*`, `br-*`, `virbr*`, `cni*`,
`flannel*`, `cali*`, `vxlan.calico`, `tunl0`, `kube-ipvs*`.

| Series | Type | Labels |
|---|---|---|
| `network_{receive,transmit}_bytes_total` | counter | `hostname`, `interface` |
| `network_{receive,transmit}_packets_total` | counter | same |
| `network_{receive,transmit}_errors_total` | counter | same |
| `network_{receive,transmit}_drops_total` | counter | same |
| `network_up` | gauge | same. Linux: up, or unknown with carrier; macOS: both `IFF_UP` and `IFF_RUNNING` |
| `network_carrier`, `network_mtu_bytes`, `network_speed_bytes` | gauge | same. Each only when the kernel exposes it |
| `network_info` | gauge, always 1 | `hostname`, `interface`, `mac`, `kind` (`wireguard`, `wlan`, `ether`…) |

sysfs shows the interfaces of the network namespace it was mounted in. A
container with its own namespace needs its own sysfs mount.

On macOS, drops are input/output queue drops. Physical carrier is not
available from this snapshot, so `network_carrier` is omitted; link speed
is omitted when the driver reports zero. Tunnels are labelled `kind="tunnel"`
and retain their OS interface name (usually `utunN` on macOS).

The older `system_network_*` gauges from `--system` are kept for now and will
be removed in a later release. Use the counters above for new dashboards.

### `--wireguard`

Runs `wg show all dump` every `--interval`, killed after 10 s, output capped at
1 MiB. Needs `CAP_NET_ADMIN` (root, or the NixOS module's `wireguard` option).
The private key and preshared keys are dropped while parsing. Peer endpoint
addresses are not exported.

| Series | Type | Labels |
|---|---|---|
| `wireguard_read_success` | gauge | `hostname`. 0 if `wg` failed, for example without permission |
| `wireguard_interface_info` | gauge, always 1 | `hostname`, `interface`, `public_key`, `listen_port` |
| `wireguard_peers` | gauge | `hostname`, `interface` |
| `wireguard_peer_latest_handshake_seconds` | gauge, Unix time, 0 = never | `hostname`, `interface`, `peer` (public key) |
| `wireguard_peer_{receive,transmit}_bytes_total` | counter | same |
| `wireguard_peer_persistent_keepalive_seconds` | gauge, only when set | same |
| `wireguard_peer_has_endpoint` | gauge | same |
| `wireguard_peer_allowed_ip_info` | gauge, always 1 | same plus `allowed_ip` |

### `--probe <IPV4>` and `--probe-wireguard-peers`

One ICMP echo per target every `--probe-interval-ms` (default 1000), with up to
1 s to answer. `--probe-wireguard-peers` adds each peer's first `/32` allowed
IP, re-read every interval, and implies `--wireguard`. At most 256 targets.
Uses a ping socket when `net.ipv4.ping_group_range` allows it, otherwise a raw
socket, which needs `CAP_NET_RAW`.

| Series | Type | Labels |
|---|---|---|
| `probe_targets` | gauge | `hostname` |
| `probe_sent_total`, `probe_received_total` | counter | `hostname`, `target` |
| `probe_rtt_seconds` | summary (`_sum`, `_count`) | same |
| `probe_last_rtt_seconds` | gauge, after the first reply | same |
| `probe_consecutive_failures` | gauge | same |

### Queries

```promql
# Packet loss over 5 minutes, per target
1 - rate(probe_received_total[5m]) / rate(probe_sent_total[5m])

# Mean round-trip time over 5 minutes
rate(probe_rtt_seconds_sum[5m]) / rate(probe_rtt_seconds_count[5m])

# Seconds since each peer's last handshake; above about 180 s the tunnel is down
time() - wireguard_peer_latest_handshake_seconds > 0

# Interface throughput and errors
rate(network_receive_bytes_total[1m])
rate(network_receive_errors_total[5m]) + rate(network_receive_drops_total[5m])
```
