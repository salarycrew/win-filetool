# 앱에 넣을 LibreOffice를 줄인다(문서 → PDF 변환에 필요한 것만 남김). Windows PowerShell 5.1에서 실행.
#
# 입력: LibreOffice 관리자 설치본 폴더(msiexec /a LibreOffice_x.y.z_Win_x86-64.msi /qn TARGETDIR=<폴더>)
# 출력: -Dest 폴더(있으면 지우고 새로 만든다). 원본은 건드리지 않는다.
#
# 목록은 0.3.0 기술 스파이크의 bisect(그룹 단위 → 파일 단위로 하나씩 지워 보고 docx→pdf가 되는지 확인) 결과다.
#  - 지우면 변환이 깨지는 것(남김): presets, ifbclient.dll, CoinMP.dll, clucene.dll, libcurl.dll, skialo.dll, share\config\soffice.cfg
#  - Apple 형식(mwaw.dll, etonyek.dll, revenge.dll, odfgen.dll, staroffice.dll, wps.dll)과 한글 97(HWP) 가져오기는 남긴다.
#  - 라이선스: poppler(GPL)를 쓰는 PDF 가져오기 부품(xpdfimport.exe, share\xpdfimport, pdfimportlo.dll)은 뺀다.
#    (앱의 PDF → Word/PPT는 Rust로 하므로 필요 없다. 문서 → PDF 내보내기는 영향 없음)
#  - 라이선스 파일(license.txt, LICENSE.html, NOTICE, CREDITS.fodt)은 남긴다.
#
# 사용: powershell -ExecutionPolicy Bypass -File scripts\trim-libreoffice.ps1 -Source C:\lo-admin -Dest C:\out\lo
param(
  [Parameter(Mandatory = $true)][string]$Source,
  [Parameter(Mandatory = $true)][string]$Dest
)
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$ErrorActionPreference = "Stop"
if (-not (Test-Path "$Source\program\soffice.exe")) { throw "LibreOffice 관리자 설치본이 아닙니다(program\soffice.exe 없음): $Source" }
function Size($p) { (Get-ChildItem -Recurse -File $p -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum }
function Count($p) { (Get-ChildItem -Recurse -File $p -ErrorAction SilentlyContinue).Count }

if (Test-Path $Dest) { Remove-Item -Recurse -Force $Dest }
robocopy $Source $Dest /E /NFL /NDL /NJH /NJS /NP /XF *.msi | Out-Null
if ($LASTEXITCODE -ge 8) { throw "복사 실패(robocopy $LASTEXITCODE)" }
"원본(msi 제외): {0} B, 파일 {1}개" -f (Size $Dest), (Count $Dest)

$del = @(
  # 도움말·안내·글꼴(Windows 글꼴을 씀)
  "help", "readmes", "Fonts",
  # 쓰지 않는 공유 데이터
  "share\extensions", "share\gallery", "share\template", "share\wizards", "share\autotext", "share\fingerprint", "share\firebird",
  "share\classification", "share\labels", "share\tipoftheday", "share\Scripts", "share\basic", "share\wordbook", "share\toolbarmode",
  "share\numbertext", "share\xslt", "share\palette", "share\autocorr", "share\liblangtag",
  # GPL(poppler) PDF 가져오기
  "share\xpdfimport", "program\xpdfimport.exe", "program\pdfimportlo.dll",
  # 파이썬·자바·마법사
  "program\python-core-*", "program\__pycache__", "program\classes", "program\wizards", "program\opencl", "program\opengl", "program\shlxthdl", "program\intl",
  "program\python312.dll", "program\python3.dll", "program\python.exe", "program\pythonw.exe", "program\pyuno.pyd", "program\pythonloaderlo.dll",
  "program\pythonscript.py", "program\uno.py", "program\unohelper.py", "program\officehelper.py", "program\msgbox.py", "program\mailmerge.py", "program\pythonloader.py",
  # 데이터베이스 엔진(Firebird 서버 부품. ifbclient.dll은 남김)
  "program\Engine12.dll", "program\firebird_sdbclo.dll",
  # 실행기·도구 exe(soffice.exe/.com/.bin만 씀)
  "program\sbase.exe", "program\smath.exe", "program\sweb.exe", "program\sdraw.exe", "program\simpress.exe", "program\scalc.exe", "program\swriter.exe",
  "program\unopkg.exe", "program\unopkg.com", "program\unopkg.bin", "program\quickstart.exe", "program\senddoc.exe", "program\twain32shim.exe",
  "program\spsupp_x64.dll", "program\spsupp_x86.dll", "program\updatecheckui*", "program\gengal*", "program\uno.bin", "program\regview.exe",
  "program\ui-previewer.exe", "program\opencltest.exe", "program\minidump_upload.exe"
)
foreach ($d in $del) { Get-Item "$Dest\$d" -ErrorAction SilentlyContinue | Remove-Item -Recurse -Force }
# 화면 번역: ko만 남긴다(계산표 머리글·바닥글의 "페이지 1" 같은 문서 안 기본 글자가 여기서 나옴)
Get-ChildItem "$Dest\program\resource" -Directory -ErrorAction SilentlyContinue | Where-Object { $_.Name -ne "ko" } | Remove-Item -Recurse -Force
# 언어팩 레지스트리: en-US, ko만
Get-ChildItem "$Dest\share\registry\res" -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -notmatch "_(en-US|ko)\." } | Remove-Item -Force
# 아이콘 테마: 모두(headless)
Get-ChildItem "$Dest\share\config" -File -Filter "images_*.zip" -ErrorAction SilentlyContinue | Remove-Item -Force

# Visual C++ 런타임: 관리자 설치본은 System64\(64비트)·System\(32비트)에 풀어 둘 뿐 program\에서 찾지 못한다.
# VC++ 재배포 패키지가 없는 PC에서도 돌도록 64비트 DLL을 program\ 옆에 두고(앱 로컬 배포) 두 폴더는 지운다.
if (Test-Path "$Dest\System64") {
  Get-ChildItem "$Dest\System64" -File -Filter *.dll | ForEach-Object { Copy-Item $_.FullName "$Dest\program\" -Force }
}
Remove-Item -Recurse -Force "$Dest\System", "$Dest\System64" -ErrorAction SilentlyContinue

foreach ($must in @("program\soffice.exe", "program\soffice.com", "program\soffice.bin", "program\mergedlo.dll", "presets", "share\config\soffice.cfg", "program\mwaw.dll", "program\etonyek.dll", "program\vcruntime140.dll", "program\msvcp140.dll")) {
  if (-not (Test-Path "$Dest\$must")) { throw "남겨야 할 부품이 없습니다: $must" }
}
foreach ($gpl in @("program\xpdfimport.exe", "share\xpdfimport", "program\pdfimportlo.dll")) {
  if (Test-Path "$Dest\$gpl") { throw "GPL 부품이 남아 있습니다: $gpl" }
}
"줄인 결과: {0} B, 파일 {1}개" -f (Size $Dest), (Count $Dest)
