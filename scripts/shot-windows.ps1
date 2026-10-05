# 截取 tingyi 程序的所有頂層視窗(即使被其他視窗蓋住),存成 PNG。開發期驗證 UI 用。
# 用法:powershell -ExecutionPolicy Bypass -File scripts\shot-windows.ps1 [-OutDir bench-out\shots] [-ProcessName tingyi]
param(
    [string]$OutDir = "bench-out\shots",
    [string]$ProcessName = "tingyi"
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class Win {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  public static List<IntPtr> Find(uint pid) {
    var list = new List<IntPtr>();
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); if (p == pid && IsWindowVisible(h)) list.Add(h); return true; }, IntPtr.Zero);
    return list;
  }
  public static string Title(IntPtr h) { var sb = new StringBuilder(256); GetWindowTextW(h, sb, 256); return sb.ToString(); }
}
"@
New-Item -ItemType Directory -Force $OutDir | Out-Null
$procs = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue
if (-not $procs) { Write-Host "找不到程序 $ProcessName"; exit 1 }
$i = 0
foreach ($p in $procs) {
  foreach ($h in [Win]::Find([uint32]$p.Id)) {
    $r = New-Object Win+RECT
    [Win]::GetWindowRect($h, [ref]$r) | Out-Null
    $w = $r.R - $r.L; $hh = $r.B - $r.T
    if ($w -le 0 -or $hh -le 0) { continue }
    $bmp = New-Object System.Drawing.Bitmap $w, $hh
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    [Win]::PrintWindow($h, $hdc, 2) | Out-Null   # 2 = PW_RENDERFULLCONTENT(WebView2 需要)
    $g.ReleaseHdc($hdc)
    $title = [Win]::Title($h)
    $safe = ($title -replace '[^\w\-]', '_')
    if (-not $safe) { $safe = "win$i" }
    $path = Join-Path $OutDir ("{0}-{1}.png" -f $safe, $i)
    $bmp.Save($path)
    $g.Dispose(); $bmp.Dispose()
    Write-Host ("saved {0}  [{1}] {2}x{3} at ({4},{5})" -f $path, $title, $w, $hh, $r.L, $r.T)
    $i++
  }
}
