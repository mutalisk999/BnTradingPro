Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Cap {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
}
"@
$p = Get-Process bn-trading-pro -ErrorAction SilentlyContinue | Select-Object -First 1
if ($null -eq $p) { Write-Output "NO_PROC"; exit 1 }
$h = $p.MainWindowHandle
if ($h -eq [IntPtr]::Zero) { Write-Output "NO_HWND"; exit 1 }
if ([Cap]::IsIconic($h)) { [Cap]::ShowWindow($h, 9) | Out-Null; Start-Sleep -Milliseconds 500 }
[Cap]::SetForegroundWindow($h) | Out-Null
Start-Sleep -Milliseconds 700
$r = New-Object Cap+RECT
$ok = [Cap]::GetWindowRect($h, [ref]$r)
$w = $r.R - $r.L
$ht = $r.B - $r.T
Write-Output ("RECT L={0} T={1} R={2} B={3} SIZE={4}x{5}" -f $r.L,$r.T,$r.R,$r.B,$w,$ht)
if ($w -le 0 -or $ht -le 0) { Write-Output "BAD_SIZE"; exit 1 }
$bmp = New-Object System.Drawing.Bitmap $w, $ht
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen([System.Drawing.Point]::new($r.L, $r.T), [System.Drawing.Point]::Empty, $bmp.Size)
$out = "E:\ai_code\BnTradingPro\screenshot_unlock.png"
$bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose()
$bmp.Dispose()
Write-Output ("SAVED {0} ({1} bytes)" -f $out, (Get-Item $out).Length)
