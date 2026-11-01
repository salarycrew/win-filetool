# win-filetool 0.3.3: 설치된 Microsoft Office(Word·Excel·PowerPoint)로 한 파일을 PDF(또는 docx·xlsx·pptx)로 저장한다.
# 앱(Rust, msoffice)이 작업 임시 폴더에 이 스크립트를 쓰고 실행한다. 입력은 원본의 복사본, 출력도 임시 폴더.
# 한 줄씩 상태를 내보낸다: PID n / NOPID / SEC n / OPEN True|False / OPENERR 메시지 / PAGES n / SAVE True|False / ERROR 메시지 / DONE
# 보안: 문서를 열기 전에 AutomationSecurity = 3(msoAutomationSecurityForceDisable, 매크로·자동 실행 안 함),
#       알림 끔, 읽기 전용, 최근 파일에 넣지 않음, 연결 갱신 안 함(Excel UpdateLinks 0), 원본에는 저장하지 않음.
#       암호 문서는 묻지 않도록 틀린 암호를 주어 바로 실패시킨다.
param([string]$In, [string]$Out, [string]$App, [string]$Fmt)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$procName = @{ Word = 'WINWORD'; Excel = 'EXCEL'; PowerPoint = 'POWERPNT' }[$App]
$missing = [Reflection.Missing]::Value
$nopass = 'wft-no-password-' + [Guid]::NewGuid().ToString('N')
$o = $null; $doc = $null; $ours = $false; $code = 0
$restore = @{}
try {
  do {
    if (-not $procName) { 'ERROR unknown app'; $code = 2; break }
    $before = @(Get-Process $procName -ErrorAction SilentlyContinue | ForEach-Object Id)
    $o = New-Object -ComObject "$App.Application"
    $new = @(Get-Process $procName -ErrorAction SilentlyContinue | Where-Object { $before -notcontains $_.Id } | ForEach-Object Id)
    foreach ($p in $new) { [Console]::Out.WriteLine("PID $p") }
    [Console]::Out.Flush()
    if ($new.Count -eq 0) {
      # 새 프로세스가 생기지 않았다 = 사용자가 쓰는 Office에 붙었다(PowerPoint는 하나만 뜬다). 건드리지 않고 그만둔다.
      'NOPID'; $code = 7; break
    }
    $ours = $true
    # 문서를 열기 전에: 매크로·자동 실행 끄기
    $o.AutomationSecurity = 3
    "SEC $($o.AutomationSecurity)"
    [Console]::Out.Flush()
    if ($o.AutomationSecurity -ne 3) { 'ERROR AutomationSecurity'; $code = 3; break }
    if ($App -eq 'Word') {
      $o.Visible = $false
      $o.DisplayAlerts = 0                      # wdAlertsNone
      try { $o.WordBasic.DisableAutoMacros(1) } catch {}
      # 연결 자동 갱신 끄기(Word 옵션은 저장되므로 끝나면 되돌린다)
      try { $restore.UpdateLinksAtOpen = $o.Options.UpdateLinksAtOpen; $o.Options.UpdateLinksAtOpen = $false } catch {}
      try { $restore.ConfirmConversions = $o.Options.ConfirmConversions; $o.Options.ConfirmConversions = $false } catch {}
      try {
        # FileName, ConfirmConversions, ReadOnly, AddToRecentFiles, PasswordDocument, PasswordTemplate, Revert,
        # WritePasswordDocument, WritePasswordTemplate, Format, Encoding, Visible, OpenAndRepair, DocumentDirection, NoEncodingDialog
        $doc = $o.Documents.Open($In, $false, $true, $false, $nopass, $nopass, $false, $missing, $missing, 0, $missing, $false, $false, $missing, $true)
      } catch { "OPENERR $($_.Exception.Message)"; 'OPEN False'; $code = 4; break }
      'OPEN True'
      try { "PAGES $($doc.ComputeStatistics(2))" } catch {}
      [Console]::Out.Flush()
      if ($Fmt -eq 'PDF') { $doc.ExportAsFixedFormat($Out, 17) }   # wdExportFormatPDF
      else { $doc.SaveAs2($Out, 16) }                              # wdFormatXMLDocument(매크로 없는 docx)
      'SAVE True'
    }
    elseif ($App -eq 'Excel') {
      $o.Visible = $false
      $o.DisplayAlerts = $false
      $o.ScreenUpdating = $false
      $o.EnableEvents = $false
      try { $restore.AskToUpdateLinks = $o.AskToUpdateLinks; $o.AskToUpdateLinks = $false } catch {}
      try {
        # FileName, UpdateLinks(0 = 갱신 안 함), ReadOnly, Format, Password, WriteResPassword, IgnoreReadOnlyRecommended,
        # Origin, Delimiter, Editable, Notify, Converter, AddToMru
        $doc = $o.Workbooks.Open($In, 0, $true, $missing, $nopass, $missing, $true, $missing, $missing, $false, $false, $missing, $false)
      } catch { "OPENERR $($_.Exception.Message)"; 'OPEN False'; $code = 4; break }
      'OPEN True'
      "PAGES $($doc.Worksheets.Count)"
      [Console]::Out.Flush()
      if ($Fmt -eq 'PDF') { $doc.ExportAsFixedFormat(0, $Out) }    # xlTypePDF
      else { $doc.SaveAs($Out, 51) }                               # xlOpenXMLWorkbook
      'SAVE True'
    }
    else {
      $o.DisplayAlerts = 1                      # ppAlertsNone
      try {
        # FileName, ReadOnly(msoTrue), Untitled(msoFalse), WithWindow(msoFalse = 창 없음)
        $doc = $o.Presentations.Open($In, -1, 0, 0)
      } catch { "OPENERR $($_.Exception.Message)"; 'OPEN False'; $code = 4; break }
      'OPEN True'
      "PAGES $($doc.Slides.Count)"
      [Console]::Out.Flush()
      if ($Fmt -eq 'PDF') { $doc.SaveAs($Out, 32) }                # ppSaveAsPDF
      else { $doc.SaveAs($Out, 24) }                               # ppSaveAsOpenXMLPresentation
      'SAVE True'
    }
  } while ($false)
}
catch {
  "ERROR $($_.Exception.Message)"
  $code = 6
}
finally {
  if ($doc) {
    try {
      if ($App -eq 'Word') { $doc.Close(0) } elseif ($App -eq 'Excel') { $doc.Close($false) } else { $doc.Close() }
    } catch {}
    try { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($doc) } catch {}
  }
  if ($o) {
    if ($ours) {
      try { if ($restore.ContainsKey('UpdateLinksAtOpen')) { $o.Options.UpdateLinksAtOpen = $restore.UpdateLinksAtOpen } } catch {}
      try { if ($restore.ContainsKey('ConfirmConversions')) { $o.Options.ConfirmConversions = $restore.ConfirmConversions } } catch {}
      try { if ($restore.ContainsKey('AskToUpdateLinks')) { $o.AskToUpdateLinks = $restore.AskToUpdateLinks } } catch {}
      try { $o.Quit() } catch {}
    }
    try { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($o) } catch {}
  }
}
'DONE'
exit $code
