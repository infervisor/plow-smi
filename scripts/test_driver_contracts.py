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


if __name__ == '__main__':
    unittest.main()
