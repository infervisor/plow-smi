"""Linux NVML contract tests with a fake driver; opt in with PLOW_TEST_BIN_DIR.

The mock never delegates to a real driver. Run after building the binaries.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest


@unittest.skipUnless(sys.platform == 'linux' and os.environ.get('PLOW_TEST_BIN_DIR'),
                     'requires Linux, a C compiler, and PLOW_TEST_BIN_DIR')
class NvmlControlTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='plow-nvml-contract-')
        self.root = Path(self.temp.name)
        self.marker = self.root / 'writes'
        self.binary = str(Path(os.environ['PLOW_TEST_BIN_DIR']).resolve() / 'plows-ctl')
        ffi = (Path(__file__).resolve().parents[1] / 'crates/plows-gpu/src/ffi/nvml.rs').read_text()
        symbols = set(re.findall(r'b"(\w+)\\0"', ffi))
        definitions = {
            'nvmlInit_v2': 'int nvmlInit_v2(void) { return 0; }',
            'nvmlShutdown': 'int nvmlShutdown(void) { return 0; }',
            'nvmlDeviceGetCount_v2': 'int nvmlDeviceGetCount_v2(unsigned *n) { *n=1; return 0; }',
            'nvmlDeviceGetHandleByIndex_v2': 'int nvmlDeviceGetHandleByIndex_v2(unsigned i, void **h) { *h=(void*)1; return i ? 2 : 0; }',
            'nvmlDeviceGetPowerManagementLimit': 'int nvmlDeviceGetPowerManagementLimit(void *h, unsigned *v) { *v=100000; return 0; }',
            'nvmlDeviceGetPowerManagementDefaultLimit': 'int nvmlDeviceGetPowerManagementDefaultLimit(void *h, unsigned *v) { *v=100000; return 0; }',
            'nvmlDeviceGetPowerManagementLimitConstraints': 'int nvmlDeviceGetPowerManagementLimitConstraints(void *h, unsigned *lo, unsigned *hi) { *lo=50000; *hi=300000; return 0; }',
            'nvmlDeviceSetPowerManagementLimit': 'int nvmlDeviceSetPowerManagementLimit(void *h, unsigned v) { FILE *f=fopen(getenv("PLOW_MOCK_WRITES"),"a"); if(f) { fprintf(f,"%u\\n",v); fclose(f); } return 0; }',
        }
        code = '#include <stdio.h>\n#include <stdlib.h>\n'
        code += '\n'.join(definitions.get(name, f'int {name}(void) {{ return 3; }}') for name in sorted(symbols))
        source = self.root / 'nvml.c'
        source.write_text(code)
        subprocess.run(['cc', '-shared', '-fPIC', str(source), '-o', str(self.root / 'libnvidia-ml.so.1')], check=True)
        self.env = {**os.environ, 'LD_LIBRARY_PATH': str(self.root), 'PLOW_MOCK_WRITES': str(self.marker)}

    def tearDown(self):
        self.temp.cleanup()

    def run_ctl(self, *args, request=None):
        return subprocess.run([self.binary, *args], input=json.dumps(request) if request else None,
                              capture_output=True, text=True, env=self.env, timeout=15)

    def test_capabilities_and_dry_run_never_write(self):
        caps = self.run_ctl('capabilities', '--format', 'json')
        self.assertEqual(caps.returncode, 0, caps.stderr)
        self.assertEqual(json.loads(caps.stdout)['contract_version'], 2)
        self.assertTrue(json.loads(caps.stdout)['gpus'][0]['power_limit_settable'])
        self.assertFalse(self.marker.exists())
        result = self.run_ctl('apply', '--dry-run', '--format', 'json',
                              request={'settings': {'gpus': {'0': {'power_limit_w': 120}}}})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)['applied'][0]['status'], 'planned')
        self.assertFalse(self.marker.exists())

    def test_invalid_limit_never_writes(self):
        result = self.run_ctl('nvidia', 'set-power-limit', '0', '--watts', '4294967295')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.marker.exists())

    def test_all_failure_has_nonzero_exit_and_json(self):
        result = self.run_ctl('nvidia', 'reset', '--all', '--format', 'json')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(json.loads(result.stdout)[0]['status'], 'failed')
        self.assertFalse(self.marker.exists())


@unittest.skipUnless(sys.platform == 'linux' and os.environ.get('PLOW_TEST_BIN_DIR'),
                     'requires Linux and PLOW_TEST_BIN_DIR')
class AmdgpuControlTests(unittest.TestCase):
    """plows-ctl against a fake amdgpu sysfs tree; writes land in its files."""

    def setUp(self):
        from fake_amdgpu import make_gpu
        self.temp = tempfile.TemporaryDirectory(prefix='plow-amdgpu-contract-')
        self.root = Path(self.temp.name)
        self.hwmon = make_gpu(self.root)
        self.perf = self.root / 'sys/bus/pci/devices/0000:03:00.0/power_dpm_force_performance_level'
        self.binary = str(Path(os.environ['PLOW_TEST_BIN_DIR']).resolve() / 'plows-ctl')
        self.env = {**os.environ, 'PLOWS_HOST_ROOT': str(self.root)}

    def tearDown(self):
        self.temp.cleanup()

    def run_ctl(self, *args):
        return subprocess.run([self.binary, 'amd', *args], capture_output=True, text=True,
                              env=self.env, timeout=15)

    def cap(self):
        return (self.hwmon / 'power1_cap').read_text().strip()

    def test_list_reports_driver_telemetry(self):
        result = self.run_ctl('list', '--format', 'json')
        self.assertEqual(result.returncode, 0, result.stderr)
        [gpu] = json.loads(result.stdout)
        self.assertEqual(gpu['uuid'], '2d007551-0000-1000-80e8-238c16749fb8')
        self.assertEqual((gpu['temperature_c'], gpu['power_w'], gpu['power_limit_w']), (31, 13, 300))

    def test_power_limit_writes_microwatts_within_range(self):
        result = self.run_ctl('set-power-limit', '0', '--watts', '250', '--format', 'json')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.cap(), '250000000')
        self.assertEqual(json.loads(result.stdout)['actual'], 250)
        for watts in ('100', '301'):
            self.assertNotEqual(self.run_ctl('set-power-limit', '0', '--watts', watts).returncode, 0)
        self.assertEqual(self.cap(), '250000000')

    def test_perf_level_and_reset(self):
        self.assertEqual(self.run_ctl('set-perf', '--all', '--level', 'high').returncode, 0)
        self.assertEqual(self.perf.read_text(), 'high')
        self.assertEqual(self.run_ctl('reset', '0').returncode, 0)
        self.assertEqual(self.perf.read_text(), 'auto')

    def test_missing_device_fails_without_writing(self):
        self.assertNotEqual(self.run_ctl('set-power-limit', '1', '--watts', '250').returncode, 0)
        self.assertEqual(self.cap(), '300000000')


if __name__ == '__main__':
    unittest.main()
