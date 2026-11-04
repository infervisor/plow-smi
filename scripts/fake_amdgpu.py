"""Fake amdgpu host root for binary-level tests (PLOWS_HOST_ROOT=<root>)."""
from pathlib import Path


def make_gpu(root, addr='0000:03:00.0', node=1, gpu_id=53865, unique_id='2de8238c16749fb8'):
    """Lay out one Radeon AI PRO R9700-like GPU; return its hwmon directory."""
    root = Path(root)
    (root / 'sys/bus/pci/drivers/amdgpu' / addr).mkdir(parents=True, exist_ok=True)
    dev = root / 'sys/bus/pci/devices' / addr
    hwmon = dev / 'hwmon' / f'hwmon{node}'
    hwmon.mkdir(parents=True, exist_ok=True)
    files = {
        dev: {
            'class': '0x030000', 'vendor': '0x1002', 'device': '0x7551',
            'unique_id': unique_id, 'gpu_busy_percent': '42', 'mem_busy_percent': '7',
            'mem_info_vram_used': '59912192', 'mem_info_vram_total': '34208743424',
            'power_dpm_force_performance_level': 'auto',
        },
        hwmon: {
            'name': 'amdgpu', 'temp1_input': '29000', 'temp1_label': 'edge',
            'temp2_input': '31000', 'temp2_label': 'junction',
            'power1_average': '13000000', 'power1_cap': '300000000',
            'power1_cap_default': '300000000', 'power1_cap_min': '210000000',
            'power1_cap_max': '300000000', 'freq1_input': '0', 'freq1_label': 'sclk',
            'freq2_input': '96000000', 'freq2_label': 'mclk',
        },
    }
    for directory, values in files.items():
        for name, value in values.items():
            (directory / name).write_text(value + '\n')
    domain, bus, rest = addr.split(':')
    slot, func = rest.split('.')
    location = int(bus, 16) << 8 | int(slot, 16) << 3 | int(func, 16)
    kfd = root / 'sys/class/kfd/kfd/topology/nodes' / str(node)
    kfd.mkdir(parents=True, exist_ok=True)
    (kfd / 'gpu_id').write_text(f'{gpu_id}\n')
    (kfd / 'properties').write_text(
        f'gfx_target_version 120001\nlocation_id {location}\ndomain {int(domain, 16)}\n')
    return hwmon
