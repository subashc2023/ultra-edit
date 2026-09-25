$LogDir = 'C:\ProgramData\Acme\logs'
$CacheDir = 'C:\ProgramData\Acme\cache'
$Pattern = '^(\d{4}-\d{2}-\d{2}) \[(\w+)\] (.*)$'

Get-ChildItem -LiteralPath $LogDir -Filter '*.log' |
    Where-Object { $_.Name -match '^agent-\d+\.log$' } |
    Remove-Item -WhatIf
