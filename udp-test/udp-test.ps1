<#
串門子的區網測試：兩台電腦都跑這支（雙擊 udp-test.cmd），兩個方向一起測。
每秒往 UDP 52525 送一輪：全網廣播（255.255.255.255）、每張網卡的網段廣播、
本機自測（127.0.0.1）；聽到另一台之後，也直接送到對方的 IP。
按 Enter 結束（最多 10 分鐘），結果印在畫面上，也存到同資料夾的
udp-test-電腦名稱.log。測完整個 udp-test 資料夾可以刪掉，不要 commit。
#>
param([double]$Minutes = 10)

$port = 52525
# 這次執行的代號：認出自己送出去又繞回來的廣播
$me = [guid]::NewGuid().ToString('N').Substring(0, 8)
$name = $env:COMPUTERNAME
$log = Join-Path $PSScriptRoot "udp-test-$name.log"
$lines = [Collections.Generic.List[string]]::new()
function Say([string]$s) { Write-Host $s; $lines.Add($s) }

Say "== $name  $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')"

# 要送的地方：全網廣播，加上每張網卡算出來的網段廣播
$to = [ordered]@{ '255.255.255.255' = '全網廣播' }
$addrs = @(Get-NetIPAddress -AddressFamily IPv4 |
  Where-Object IPAddress -NotMatch '^(127|169\.254)\.')
foreach ($a in $addrs) {
  $b = ([Net.IPAddress]$a.IPAddress).GetAddressBytes()
  foreach ($i in 0..3) {
    $keep = [Math]::Min(8, [Math]::Max(0, $a.PrefixLength - 8 * $i))
    $b[$i] = $b[$i] -bor (255 -shr $keep)
  }
  $bc = [Net.IPAddress]::new($b).ToString()
  $to[$bc] = "網段廣播 $($a.InterfaceAlias)"
  Say ('本機 {0}/{1}  {2}' -f $a.IPAddress, $a.PrefixLength, $a.InterfaceAlias)
}

# 判讀結果用的資訊；讀不到就記下原因，不影響測試
try {
  Get-NetConnectionProfile -ErrorAction Stop | ForEach-Object {
    Say ('網路類型 {0}：{1}' -f $_.InterfaceAlias, $_.NetworkCategory)
  }
} catch { Say "網路類型讀不到：$($_.Exception.Message)" }
try {
  $r = Find-NetRoute -RemoteIPAddress 255.255.255.255 -ErrorAction Stop |
    Where-Object { $_.IPAddress } | Select-Object -First 1
  Say "照路由表，全網廣播會從 $($r.InterfaceAlias)（$($r.IPAddress)）出去"
} catch { Say "查不到全網廣播走哪張網卡：$($_.Exception.Message)" }
try {
  Get-NetFirewallRule -DisplayName '*PowerShell*' -ErrorAction Stop |
    ForEach-Object {
      Say ('防火牆規則 {0}：{1} {2}，啟用 {3}，{4}' -f $_.DisplayName,
        $_.Direction, $_.Action, $_.Enabled, $_.Profile)
    }
} catch [Management.Automation.CommandNotFoundException] {
  Say "防火牆規則讀不到：$($_.Exception.Message)"
} catch {
  if ($_.CategoryInfo.Category -eq 'ObjectNotFound') {
    Say '防火牆規則：沒有 PowerShell 的規則'
  } else { Say "防火牆規則讀不到：$($_.Exception.Message)" }
}

Write-Host ''
$peer = Read-Host '另一台的 IP（不知道就直接按 Enter）'
# 直接送的對象：手動輸入的，加上之後聽到的
$direct = [Collections.Generic.HashSet[string]]::new()
if ($peer -and $peer.Trim()) {
  [void]$direct.Add($peer.Trim())
  Say "手動輸入的另一台：$($peer.Trim())"
}

try { $u = [Net.Sockets.UdpClient]::new($port) }
catch {
  Write-Host "UDP $port 開不起來（是不是已經開了一個測試視窗？）："
  Write-Host $_.Exception.InnerException.Message
  exit 1
}
$u.EnableBroadcast = $true
$from = [Net.IPEndPoint]::new([Net.IPAddress]::Any, 0)
$got = @{}       # "IP|種類" → 收到幾次
$sendErr = @{}   # 送不出去的位址 → 只記第一次
$heard = [Collections.Generic.HashSet[string]]::new()
$end = (Get-Date).AddMinutes($Minutes)
$next = Get-Date
Say '開始：每秒送一輪。另一台也跑起來後，等 30 秒左右再按 Enter 結束'
try {
  while ((Get-Date) -lt $end) {
    if (-not [Console]::IsInputRedirected -and [Console]::KeyAvailable) {
      if ([Console]::ReadKey($true).Key -eq 'Enter') { break }
    }
    if ((Get-Date) -ge $next) {
      $next = (Get-Date).AddSeconds(1)
      $targets = [Collections.Generic.List[object]]::new()
      foreach ($k in $to.Keys) { $targets.Add(@{ IP = $k; Kind = $to[$k] }) }
      foreach ($k in $direct) { $targets.Add(@{ IP = $k; Kind = '直接送' }) }
      $targets.Add(@{ IP = '127.0.0.1'; Kind = '本機自測' })
      foreach ($t in $targets) {
        $m = [Text.Encoding]::UTF8.GetBytes("yarncat|$me|$($t.Kind)")
        try { [void]$u.Send($m, $m.Length, $t.IP, $port) }
        catch {
          if (-not $sendErr.ContainsKey($t.IP)) {
            $sendErr[$t.IP] = $_.Exception.InnerException.Message
            Say "送不出去 $($t.IP)（$($t.Kind)）：$($sendErr[$t.IP])"
          }
        }
      }
    }
    if ($u.Available -eq 0) { Start-Sleep -Milliseconds 50; continue }
    # Windows 上，送到沒人在聽的位址後，下一次收會丟錯誤；跳過就好
    try { $b = $u.Receive([ref]$from) } catch { continue }
    $p = [Text.Encoding]::UTF8.GetString($b).Split('|')
    if ($p.Count -ne 3 -or $p[0] -ne 'yarncat') { continue }
    # 自己的廣播繞回來不算；本機自測是故意送給自己的
    if ($p[1] -eq $me -and $p[2] -ne '本機自測') { continue }
    $src = $from.Address.ToString()
    if ($p[1] -ne $me -and $heard.Add($src)) {
      Say "聽到另一台 $src，之後也直接送給它"
      [void]$direct.Add($src)
    }
    $key = "$src|$($p[2])"
    $got[$key] = 1 + $got[$key]
    if ($got[$key] -eq 1) {
      Say ('{0:HH:mm:ss}  收到 {1,-15}  {2}' -f (Get-Date), $src, $p[2])
    }
  }
} finally { $u.Close() }

Say ''
Say "== 結果（$name）"
$self = $got['127.0.0.1|本機自測']
if ($self) { Say "本機自測：$self 次（收的部分正常）" }
else { Say '本機自測：0 次（收的部分沒跑起來，下面的結果不準）' }
$others = @($got.Keys | Where-Object { $_ -ne '127.0.0.1|本機自測' } |
  Sort-Object)
if (-not $others) { Say '另一台：什麼都沒收到' }
foreach ($k in $others) {
  $ip, $kind = $k.Split('|')
  Say ('{0,-15}  {1}：{2} 次' -f $ip, $kind, $got[$k])
}
$lines | Set-Content -Path $log -Encoding UTF8
Write-Host ''
Write-Host "已存到 $log"
Read-Host '按 Enter 關閉視窗' | Out-Null
