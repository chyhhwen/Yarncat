<#
把 tauri build 編好的 exe 包成 MSIX，上架 Microsoft Store 用。說明見同一個資料夾的 README.md。

產出放在 -OutDir（預設 src-tauri\target\msix）：
  YarnCat_<版本>_x64.msix  上傳到 Partner Center 的套件。沒有簽章，商店審核通過後會幫忙簽
  test\                   同一份內容不壓縮，開「開發人員模式」就能在自己電腦上裝起來測

要用到 Windows SDK 的 makeappx.exe、makepri.exe（GitHub 的 Windows 機器已經裝好）。
#>
param(
    # tauri build 編好的 exe
    [string]$Exe,
    # 產出放哪
    [string]$OutDir,
    # 四段的版本號；不給就拿 tauri.conf.json 的版本，後面補一個 .0
    [string]$Version,
    # makeappx.exe、makepri.exe 所在的資料夾；不給就到 Windows SDK 裡找
    [string]$SdkBin
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3

if ($env:GITHUB_ACTIONS -eq 'true') {
    # 不然 GitHub Actions 紀錄裡的中文會變問號。不能有 BOM：第一行是 ::warning:: 時會認不出來
    try { [Console]::OutputEncoding = New-Object Text.UTF8Encoding $false } catch { }
}

# 還不能送審、但可以先在自己電腦測的情況，只警告不中斷。
# 在 GitHub Actions 上會變成黃色的警告，那次執行的頁面最上面就看得到
function Warn([string]$Message) {
    if ($env:GITHUB_ACTIONS -eq 'true') { Write-Host "::warning::$Message" } else { Write-Warning $Message }
}

$tauriDir = Split-Path -Parent $PSScriptRoot
if (-not $Exe) { $Exe = Join-Path $tauriDir 'target\release\yarncat.exe' }
if (-not $OutDir) { $OutDir = Join-Path $tauriDir 'target\msix' }
if (-not (Test-Path $Exe -PathType Leaf)) { throw "找不到 $Exe，先執行 npm run tauri build" }
# .NET 的函式不認得 PowerShell 的目前資料夾，相對路徑先換成完整路徑
$Exe = (Resolve-Path $Exe).Path
$OutDir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutDir)

function Find-SdkTool([string]$Name) {
    if ($SdkBin) {
        $path = Join-Path $SdkBin $Name
        if (Test-Path $path) { return $path }
        throw "$SdkBin 裡沒有 $Name"
    }
    # Windows SDK 可能裝了好幾版，挑最新的
    $found = Get-ChildItem (Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin') -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '^\d+(\.\d+){3}$' } |
        Sort-Object { [version]$_.Name } -Descending |
        ForEach-Object { Join-Path $_.FullName "x64\$Name" } |
        Where-Object { Test-Path $_ } |
        Select-Object -First 1
    if (-not $found) { throw "找不到 $Name：要裝 Windows SDK，或用 -SdkBin 指定它所在的資料夾" }
    return $found
}

# 先找好工具，缺了就不用白做後面的事
$makepri = Find-SdkTool 'makepri.exe'
$makeappx = Find-SdkTool 'makeappx.exe'

# ---- 版本號 ----

if (-not $Version) {
    $conf = [IO.File]::ReadAllText((Join-Path $tauriDir 'tauri.conf.json'), [Text.Encoding]::UTF8) | ConvertFrom-Json
    if ($conf.version -notmatch '^\d+\.\d+\.\d+$') { throw "tauri.conf.json 的版本號 $($conf.version) 不是 x.y.z，包不成 MSIX" }
    $Version = "$($conf.version).0"
}
if ($Version -notmatch '^\d{1,5}(\.\d{1,5}){3}$') { throw "MSIX 的版本號要是四段數字：$Version" }
$v = @($Version.Split('.') | ForEach-Object { [int]$_ })
if (@($v | Where-Object { $_ -gt 65535 }).Count) { throw "MSIX 版本號每一段最大 65535：$Version" }
if ($v[0] -eq 0) { Warn "版本號 $Version 的第一段是 0，商店不收。送審前要把 package.json、tauri.conf.json、Cargo.toml 的版本升到 1.0.0 以上" }
if ($v[3] -ne 0) { Warn "版本號 $Version 的第四段不是 0，商店不收（第四段留給商店用）" }

# ---- AppxManifest.xml ----

$manifest = New-Object Xml.XmlDocument
$manifest.PreserveWhitespace = $true
$manifest.Load((Join-Path $PSScriptRoot 'AppxManifest.xml'))
$ns = New-Object Xml.XmlNamespaceManager $manifest.NameTable
$ns.AddNamespace('m', 'http://schemas.microsoft.com/appx/manifest/foundation/windows10')
$identity = $manifest.SelectSingleNode('/m:Package/m:Identity', $ns)
$publisherName = $manifest.SelectSingleNode('/m:Package/m:Properties/m:PublisherDisplayName', $ns).InnerText
if ("$($identity.GetAttribute('Name')) $($identity.GetAttribute('Publisher')) $publisherName" -match 'FILL-IN') {
    Warn 'AppxManifest.xml 還沒填 Partner Center 給的值（FILL-IN 的地方）。這個 .msix 可以在自己電腦測，但還不能送審'
}
$identity.SetAttribute('Version', $Version)
# 照 exe 實際的檔名寫，大小寫跟著 tauri build 產出的走
$exeName = (Get-Item $Exe).Name
$manifest.SelectSingleNode('/m:Package/m:Applications/m:Application', $ns).SetAttribute('Executable', $exeName)

# ---- 排好要包進去的檔案 ----

New-Item -ItemType Directory -Force $OutDir | Out-Null
$layout = Join-Path $OutDir 'test'
$assets = Join-Path $layout 'Assets'
$msix = Join-Path $OutDir "YarnCat_$($Version)_x64.msix"
# 只清掉這支腳本上次產出的東西，-OutDir 指到別的資料夾也不會誤刪
if (Test-Path $layout) { Remove-Item $layout -Recurse -Force }
Get-ChildItem $OutDir -Filter 'YarnCat_*_x64.msix' | Remove-Item
New-Item -ItemType Directory $assets | Out-Null
Copy-Item $Exe (Join-Path $layout $exeName)
$manifest.Save((Join-Path $layout 'AppxManifest.xml'))

# ---- 圖示 ----

Add-Type -AssemblyName System.Drawing

# icon.png 是 16×16 的像素畫放大 32 倍。先還原成 16×16，之後只放大整數倍，邊緣才不會糊
$Grid = 16
$src = New-Object System.Drawing.Bitmap (Join-Path $tauriDir 'icons\icon.png')
try {
    if ($src.Width -ne $src.Height -or $src.Width % $Grid) { throw "icon.png 要是邊長為 $Grid 的倍數的正方形" }
    $cell = [int]($src.Width / $Grid)
    $art = New-Object System.Drawing.Bitmap $Grid, $Grid, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    for ($y = 0; $y -lt $Grid; $y++) {
        for ($x = 0; $x -lt $Grid; $x++) {
            $l = $x * $cell; $t = $y * $cell; $r = $l + $cell - 1; $b = $t + $cell - 1
            $c = $src.GetPixel($l, $t)
            # 每一格的四個角都同色才是像素畫（全透明的不管顏色）。換成一般的圖示的話，這裡要改成平滑縮放
            foreach ($p in @(@($r, $t), @($l, $b), @($r, $b))) {
                $o = $src.GetPixel($p[0], $p[1])
                if ($o.ToArgb() -ne $c.ToArgb() -and ($o.A -ne 0 -or $c.A -ne 0)) {
                    throw "icon.png 不是 $Grid×$Grid 的像素畫放大來的，build.ps1 的縮放方式要改"
                }
            }
            $art.SetPixel($x, $y, $c)
        }
    }
} finally { $src.Dispose() }

# 畫一張 Size×Size 的透明圖：圖案放大到 16 的整數倍、不超過邊長的 Fill 比例，放在正中間
function Save-Logo([string]$Name, [int]$Size, [double]$Fill) {
    $px = $Grid * [Math]::Max(1, [Math]::Floor($Size * $Fill / $Grid))
    $at = [int][Math]::Floor(($Size - $px) / 2)
    $bmp = New-Object System.Drawing.Bitmap $Size, $Size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    try {
        $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
        # 沒有這行，放大後會往左上偏半格，最上面一排、最左邊一行被切掉一半
        $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::Half
        $g.DrawImage($art, $at, $at, [int]$px, [int]$px)
        $bmp.Save((Join-Path $assets $Name), [System.Drawing.Imaging.ImageFormat]::Png)
    } finally { $g.Dispose(); $bmp.Dispose() }
}

# 照縮放比例換算邊長，.5 一律進位（150 的 125% 是 188）；[Math]::Round 預設是銀行家捨入，會變成 62
function Scale([int]$Size, [int]$Percent) {
    [int][Math]::Round($Size * $Percent / 100, [MidpointRounding]::AwayFromZero)
}

# 檔名裡的 scale-200、targetsize-24 讓 Windows 照螢幕縮放比例挑尺寸（下面的 makepri 會建索引）
foreach ($scale in 100, 125, 150, 200, 400) {
    # 開始功能表、搜尋、設定裡的 app 清單
    Save-Logo "Square44x44Logo.scale-$scale.png" (Scale 44 $scale) 1
    # Windows 10 開始功能表的磚，下面會壓著 app 名稱，所以圖案小一點
    Save-Logo "Square150x150Logo.scale-$scale.png" (Scale 150 $scale) 0.66
    # 安裝畫面
    Save-Logo "StoreLogo.scale-$scale.png" (Scale 50 $scale) 1
}
foreach ($size in 16, 24, 32, 48, 256) {
    Save-Logo "Square44x44Logo.targetsize-$size.png" $size 1
    # 不墊底色的版本，開始功能表、工作列優先用
    Save-Logo "Square44x44Logo.targetsize-$($size)_altform-unplated.png" $size 1
}
$art.Dispose()

# ---- 打包 ----

# 這兩個工具很囉唆，成功時不印，失敗才把輸出印出來
function Invoke-Tool([string]$Tool, [string[]]$Arguments) {
    $log = & $Tool @Arguments
    if ($LASTEXITCODE -ne 0) {
        $log | Write-Host
        throw "$(Split-Path -Leaf $Tool) 失敗（結束代碼 $LASTEXITCODE）"
    }
}

# resources.pri：圖示的索引，Windows 靠它找到 scale-200 這些不同尺寸的檔案。
# 設定檔放在暫存資料夾，不然會被當成資源一起包進去
$work = Join-Path ([IO.Path]::GetTempPath()) "yarncat-msix-$([guid]::NewGuid())"
New-Item -ItemType Directory $work | Out-Null
try {
    $priConfig = Join-Path $work 'priconfig.xml'
    Invoke-Tool $makepri @('createconfig', '/cf', $priConfig, '/dq', 'zh-TW', '/pv', '10.0.0', '/o')
    # 預設的設定會把 scale-200 這些圖示拆到另外的 resources.scale-200.pri，那是給商店分開下載的資源套件用的。
    # 只包一個 .msix 時 Windows 只讀 resources.pri，拆出去的就找不到，高解析度螢幕會拿 100% 的圖硬放大
    $config = New-Object Xml.XmlDocument
    $config.Load($priConfig)
    $packaging = $config.SelectSingleNode('/resources/packaging')
    if ($packaging) { [void]$config.DocumentElement.RemoveChild($packaging) }
    $config.Save($priConfig)
    Invoke-Tool $makepri @('new', '/pr', $layout, '/cf', $priConfig, '/mn', (Join-Path $layout 'AppxManifest.xml'), '/of', (Join-Path $layout 'resources.pri'), '/o')
} finally { Remove-Item $work -Recurse -Force }

Invoke-Tool $makeappx @('pack', '/d', $layout, '/p', $msix, '/o')

Write-Host "送審用：$msix"
Write-Host "測試用：$layout"
