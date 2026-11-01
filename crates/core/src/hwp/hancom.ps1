# win-filetool 0.3.0 W6: 설치된 한글(HWPFrame.HwpObject)로 한 파일을 PDF/DOCX로 저장한다.
# 앱(Rust, hwp::hancom)이 작업 임시 폴더에 이 스크립트를 쓰고 실행한다. 입력은 원본의 복사본, 출력도 임시 폴더.
# 한 줄씩 상태를 내보낸다: PID n / REG / OPEN / PAGES n / SAVE / ERROR 메시지 / DONE
param([string]$In, [string]$Out, [string]$Fmt, [string]$Module)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$hwp = $null
$ours = $false
$code = 0
try {
  do {
    $before = @(Get-Process Hwp -ErrorAction SilentlyContinue | ForEach-Object Id)
    $hwp = New-Object -ComObject HWPFrame.HwpObject
    $new = @(Get-Process Hwp -ErrorAction SilentlyContinue | Where-Object { $before -notcontains $_.Id } | ForEach-Object Id)
    foreach ($p in $new) { [Console]::Out.WriteLine("PID $p") }
    [Console]::Out.Flush()
    if ($new.Count -eq 0) {
        # 새 한글 프로세스가 생기지 않았다 = 사용자가 쓰는 한글에 붙었을 수 있다. 건드리지 않고 그만둔다.
        'NOPID'; $code = 7; break
    }
    $ours = $true
    # 파일 경로 보안 확인 창 대신 앱이 등록한 보안 모듈(HKCU\Software\HNC\HwpAutomation\Modules)을 쓴다
    if (-not $hwp.RegisterModule('FilePathCheckDLL', $Module)) { 'REG False'; $code = 3; break }
    'REG True'
    try { $hwp.XHwpWindows.Item(0).Visible = $false } catch {}
    # 메시지 상자 자동 응답: 확인 / 확인 / 무시 / 예 / 예 / 취소 (작업 대상은 임시 복사본뿐)
    [void]$hwp.SetMessageBoxMode(0x211411)
    if (-not $hwp.Open($In, '', 'forceopen:true;suspendpassword:true;versionwarning:false;lock:false')) { 'OPEN False'; $code = 4; break }
    'OPEN True'
    "PAGES $($hwp.PageCount)"
    [Console]::Out.Flush()
    if (-not $hwp.SaveAs($Out, $Fmt, '')) { 'SAVE False'; $code = 5; break }
    'SAVE True'
  } while ($false)
}
catch {
    "ERROR $($_.Exception.Message)"
    $code = 6
}
finally {
    if ($hwp) {
        if ($ours) {
            try { [void]$hwp.Clear(1) } catch {}
            try { $hwp.Quit() } catch {}
        }
        try { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($hwp) } catch {}
    }
}
'DONE'
exit $code
