# 시험용 HEVC 타일 (HEIC 샘플 만들기)

`samples::make_grid_heic`가 아이폰식 grid HEIC 시험 파일을 만들 때 쓰는 단색 타일이다.
앱에 든 ffmpeg는 LGPL 최소 빌드라 x265 인코더가 없으므로, 타일을 한 번 만들어 저장소에 넣었다(0.3.0).

- `t<크기>-<번호>.hevc`: 크기 64·256·512px, 번호 0=빨강(red) 1=초록(lime) 2=파랑(blue) 3=흰색(white). HEVC Annex B 스트림 1장.
- 만든 법(ffmpeg 8.0 + libx265 4.1):
  `ffmpeg -f lavfi -i color=c=<색>:s=<크기>x<크기> -frames:v 1 -pix_fmt yuv420p -c:v libx265 -f hevc t<크기>-<번호>.hevc`
  뒤에 인코더 정보 SEI(NAL 39/40)를 빼고 VPS·SPS·PPS·IDR만 남겼다.
- 라이선스: 단색 그림을 이 프로젝트에서 만든 파일로, 저작물이 아니다. 저장소의 MIT 라이선스로 자유롭게 쓸 수 있다.
  (인코더 프로그램의 라이선스는 만든 결과 파일에 미치지 않는다. 시험 파일을 만들 때만 쓰며, `filetool-cli selftest`에 들어 있다.)
