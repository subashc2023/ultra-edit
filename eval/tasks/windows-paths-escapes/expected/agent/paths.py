import re

LOG_DIR = r"D:\AcmeData\logs\current"
CACHE_DIR = "C:\\ProgramData\\Acme\\cache"
SHARE_ROOT = r"\\fs02\acme$\drops"
LINE_RE = re.compile(r"^(\d{4}-\d{2}-\d{2}) \[([A-Z]+)\] (.*)$")
