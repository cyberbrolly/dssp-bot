"""Shared pytest setup.

Tests must never touch the real DSSP portal, so the origin is forced to a
non-routable address; client tests patch the portal constants to a local fixture
server. The browser runs headless with a throwaway profile.
"""

import atexit
import os
import shutil
import sys
import tempfile
from pathlib import Path

PY_ROOT = Path(__file__).resolve().parent.parent
if str(PY_ROOT) not in sys.path:
    sys.path.insert(0, str(PY_ROOT))

os.environ["DSSP_ORIGIN"] = "http://127.0.0.1:1"
os.environ["DSSP_HEADLESS"] = "1"

_profile = tempfile.mkdtemp(prefix="dssp-pw-test-")
os.environ["DSSP_PROFILE_DIR"] = _profile
atexit.register(shutil.rmtree, _profile, True)
