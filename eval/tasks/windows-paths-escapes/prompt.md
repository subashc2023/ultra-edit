The Acme agent's log directory, file server, and log-line pattern are spelled in three files with different escaping. Update all three files.

Every backslash below is a literal character in the file, so reproduce each one exactly. `config/agent.json` stores each backslash as two characters (`\\`); `scripts/Rotate-Logs.ps1` and `agent/paths.py` use single backslashes. `scripts/Rotate-Logs.ps1` starts with a UTF-8 byte order mark and uses CRLF line endings; keep both.

1. `config/agent.json`
   - Replace `"logDir": "C:\\ProgramData\\Acme\\logs",` with `"logDir": "D:\\AcmeData\\logs\\current",`
   - Replace `"shareRoot": "\\\\fileserver\\acme$\\drops",` with `"shareRoot": "\\\\fs02\\acme$\\drops",`
   - In `linePattern`, replace `\\[(\\w+)\\]` with `\\[([A-Z]+)\\]`
2. `scripts/Rotate-Logs.ps1`
   - Replace `$LogDir = 'C:\ProgramData\Acme\logs'` with `$LogDir = 'D:\AcmeData\logs\current'`
   - In `$Pattern`, replace `\[(\w+)\]` with `\[([A-Z]+)\]`
3. `agent/paths.py`
   - Replace `LOG_DIR = r"C:\ProgramData\Acme\logs"` with `LOG_DIR = r"D:\AcmeData\logs\current"`
   - Replace `SHARE_ROOT = r"\\fileserver\acme$\drops"` with `SHARE_ROOT = r"\\fs02\acme$\drops"`
   - In `LINE_RE`, replace `\[(\w+)\]` with `\[([A-Z]+)\]`

The cache directory entries, the JSON `banner` value, and every other line stay exactly as they are.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
