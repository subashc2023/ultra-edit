import re

LOG_DIR = r"C:\ProgramData\Acme\logs"
CACHE_DIR = "C:\\ProgramData\\Acme\\cache"
SHARE_ROOT = r"\\fileserver\acme$\drops"
LINE_RE = re.compile(r"^(\d{4}-\d{2}-\d{2}) \[(\w+)\] (.*)$")
