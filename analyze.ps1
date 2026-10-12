Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
public class Cap {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
}
"@
$p = Get-Process bn-trading-pro -ErrorAction SilentlyContinue | Select-Object -First 1
if ($null -eq $p) { Write-Output "NO_PROC"; exit 1 }
$h = $p.MainWindowHandle
[Cap]::SetForegroundWindow($h) | Out-Null
Start-Sleep -Milliseconds 400
$r = New-Object Cap+RECT
[Cap]::GetWindowRect($h, [ref]$r) | Out-Null
$w = $r.R - $r.L; $ht = $r.B - $r.T
$bmp = New-Object System.Drawing.Bitmap $w, $ht
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen([System.Drawing.Point]::new($r.L, $r.T), [System.Drawing.Point]::Empty, $bmp.Size)
$path = "E:\ai_code\BnTradingPro\screenshot_unlock.png"
$bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()

# 分析内容分布：按行扫描，找出非背景色的行（卡片/文字所在区域）
$bmp2 = [System.Drawing.Bitmap]::FromFile($path)
$bg = $bmp2.GetPixel(5, 5)
Write-Output ("bg_color at (5,5): {0}" -f $bg.ToString())
$firstRow = -1; $lastRow = -1
$rowCounts = @()
for ($y = 0; $y -lt $bmp2.Height; $y += 4) {
  $nonbg = 0
  for ($x = 0; $x -lt $bmp2.Width; $x += 8) {
    $c = $bmp2.GetPixel($x, $y)
    if ([Math]::Abs($c.R - $bg.R) -gt 12 -or [Math]::Abs($c.G - $bg.G) -gt 12 -or [Math]::Abs($c.B - $bg.B) -gt 12) { $nonbg++ }
  }
  $rowCounts += ,@($y, $nonbg)
  if ($nonbg -gt 3) {
    if ($firstRow -lt 0) { $firstRow = $y }
    $lastRow = $y
  }
}
Write-Output ("content vertical span: y={0} .. y={1} (window height {2})" -f $firstRow, $lastRow, $bmp2.Height)
# 找出内容最宽的行（卡片宽度线索）以及卡片左边界
$cardL = 99999; $cardR = -1
for ($y = $firstRow; $y -le $lastRow; $y += 2) {
  for ($x = 0; $x -lt $bmp2.Width; $x += 2) {
    $c = $bmp2.GetPixel($x, $y)
    # 卡片色 #1F2632 = (31,38,50)
    if ([Math]::Abs($c.R - 31) -lt 10 -and [Math]::Abs($c.G - 38) -lt 10 -and [Math]::Abs($c.B - 50) -lt 10) {
      if ($x -lt $cardL) { $cardL = $x }
      if ($x -gt $cardR) { $cardR = $x }
    }
  }
}
Write-Output ("card fill color span: x={0} .. x={1} (window width {2})" -f $cardL, $cardR, $bmp2.Width)
# 卡片垂直范围
$cardT = -1; $cardB = -1
for ($y = 0; $y -lt $bmp2.Height; $y += 2) {
  for ($x = $cardL; $x -le $cardR; $x += 6) {
    $c = $bmp2.GetPixel($x, $y)
    if ([Math]::Abs($c.R - 31) -lt 10 -and [Math]::Abs($c.G - 38) -lt 10 -and [Math]::Abs($c.B - 50) -lt 10) {
      if ($cardT -lt 0) { $cardT = $y }
      $cardB = $y
      break
    }
  }
}
Write-Output ("card vertical span: y={0} .. y={1}" -f $cardT, $cardB)
$bmp2.Dispose()
