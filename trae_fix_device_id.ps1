$ErrorActionPreference = 'Stop'

$dirs = @(
    (Join-Path $env:APPDATA 'TRAE SOLO CN_YOUR_ACCOUNT_1'),
    (Join-Path $env:APPDATA 'TRAE SOLO CN_YOUR_ACCOUNT_2')
)

foreach ($d in $dirs) {
    $name = Split-Path $d -Leaf
    $storagePath = Join-Path $d 'User\globalStorage\storage.json'
    $midPath = Join-Path $d 'machineid'
    if (-not (Test-Path $storagePath)) { Write-Output "[SKIP] $name : no storage.json"; continue }

    # 1. backup
    $bak = "$storagePath.bak-$([DateTime]::Now.ToString('yyyyMMddHHmmss'))"
    Copy-Item $storagePath $bak -Force
    if (Test-Path $midPath) { Copy-Item $midPath "$midPath.bak-$([DateTime]::Now.ToString('yyyyMMddHHmmss'))" -Force }

    # 2. new independent identities
    $newDev = [System.Guid]::NewGuid().ToString()
    $newMid = [System.Guid]::NewGuid().ToString()

    # 3. replace or inject devDeviceId in storage.json (keep everything else untouched)
    $content = Get-Content -Raw -Encoding UTF8 $storagePath
    if ($content -match '"telemetry\.devDeviceId"\s*:\s*"[^"]*"') {
        $content = [regex]::Replace($content, '"telemetry\.devDeviceId"\s*:\s*"[^"]*"', '"telemetry.devDeviceId": "' + $newDev + '"')
    } else {
        # 字段缺失(客户端迁移到 aha 后已移除):在根对象首个 "{" 后注入
        $idx = $content.IndexOf('{')
        if ($idx -ge 0) {
            $content = $content.Substring(0, $idx + 1) + "`r`n    `"telemetry.devDeviceId`": `"$newDev`"," + $content.Substring($idx + 1)
        }
    }
    [System.IO.File]::WriteAllText($storagePath, $content, (New-Object System.Text.UTF8Encoding($false)))

    # 4. overwrite machineid file
    [System.IO.File]::WriteAllText($midPath, $newMid, (New-Object System.Text.UTF8Encoding($false)))

    Write-Output ("[OK] {0}`n  new_dev={1}`n  new_machineid={2}`n  backup={3}" -f $name, $newDev, $newMid, (Split-Path $bak -Leaf))
}
