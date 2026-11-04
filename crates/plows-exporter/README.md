# plows-exporter

Prometheus metrics exporter backed by **`plows-gpu`** (NVML / amdgpu sysfs / Level Zero at runtime).

```bash
cargo run -p plows-exporter -- --all
# http://0.0.0.0:9835/metrics
```

Flags: `--nvidia`, `--amd`, `--intel`, `--system`, `--tpu`, `--all`, `--port`, `--bind`.
