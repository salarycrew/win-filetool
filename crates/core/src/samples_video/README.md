# 시험용 짧은 영상

`samples::make_test_video`가 쓰는 미리 만든 시험 영상이다. 앱에 든 ffmpeg는 LGPL 최소 빌드라 `lavfi`(시험 화면 만들기)와
mkv·avi·webm 쓰기가 없어서 `filetool-cli selftest`가 Windows에서 그 ffmpeg만으로 돌도록 한 번 만들어 넣었다(0.3.0).

| 파일 | 내용 |
|---|---|
| `clip-640x360-3s.mov` | 640×360 30fps 3초, MPEG-4 Part 2 + PCM 8 kHz |
| `clip-320x240-2s.mkv`, `.avi` | 320×240 30fps 2초, MPEG-4 Part 2 + PCM 8 kHz |
| `clip-320x240-2s.webm` | 320×240 30fps 2초, VP8 + Opus |

만든 법(ffmpeg 8.0): `ffmpeg -f lavfi -i testsrc2=s=<가로>x<세로>:r=30:d=<초> -f lavfi -i sine=f=440:d=<초>:sample_rate=8000 -t <초>
-c:v mpeg4 -q:v 24 -c:a pcm_s16le -fflags +bitexact -map_metadata -1 <파일>` (webm은 `-c:v libvpx -b:v 100k -c:a libopus -b:a 24k`, 48 kHz).
ffmpeg의 시험 화면(testsrc2)과 사인파로 만든 파일이라 저작물이 아니며, 저장소의 MIT 라이선스로 쓸 수 있다. 배포 앱에는 들어가지 않는다(명령줄 점검 도구용).
