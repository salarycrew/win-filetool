#!/usr/bin/env python3
"""0.3.0 W2 문서 변환 시험용 샘플 만들기(한국어, 라이선스 걱정 없는 내용).

필요: python-docx, python-pptx, openpyxl, pypdf, Pillow(모두 MIT/BSD), LibreOffice(soffice, PDF 만들기만),
      pdftoppm(poppler-utils, 스캔본 그림 만들기만 — 결과물에 들어가지 않음), 나눔고딕(SIL OFL, PDF에 일부만 들어감).
글과 그림은 모두 이 스크립트가 지어낸 것이다.

사용: make_corpus.py <soffice 경로> [나눔고딕.ttf]
만드는 것(이 폴더):
  보고서.docx      제목·부제·H1/H2/H3·글머리표(2단계)·번호 목록·표(머리글)·그림+설명·표 안 그림·머리글/바닥글·2단 구역
  보고서.odt       같은 문서를 LibreOffice로(.odt 경로 시험)
  보고서_LO.pdf    LibreOffice가 만든 PDF(머리글·바닥글·쪽 번호가 쪽마다)
  두단.pdf         두 단(column) 본문 PDF
  스캔본.pdf       쪽을 150dpi 회색 그림으로(기울기 0.7°, 잡티) — 글자 없음
  암호.pdf         여는 암호(1234)가 걸린 PDF
  회전쪽.pdf       2쪽이 /Rotate 90
  선없는표.pdf     테두리 없는 표(가운데 머리글·오른쪽 정렬 숫자), 굵게·기울임·하이퍼링크
  엣지인쇄.html    Edge "PDF로 인쇄"용 원본(엣지인쇄.pdf는 Windows에서 msedge --print-to-pdf로 만든다)
  이미지모음.pptx  같은 그림을 두 슬라이드에 + 다른 그림
  그림표.xlsx      그림 두 장이 든 시트
"""
import io
import os
import random
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from docx import Document
from docx.enum.section import WD_SECTION
from docx.oxml import OxmlElement
from docx.oxml.ns import qn
from docx.shared import Cm, Pt
from PIL import Image, ImageDraw, ImageFilter, ImageFont

HERE = Path(__file__).resolve().parent
SOFFICE = sys.argv[1] if len(sys.argv) > 1 else "soffice"
FONT_FILE = sys.argv[2] if len(sys.argv) > 2 else str(Path.home() / ".local/share/fonts/nanum/NanumGothic.ttf")
FONT = "NanumGothic"

random.seed(30)


def font(size):
    try:
        return ImageFont.truetype(FONT_FILE, size)
    except OSError:
        return ImageFont.load_default()


def chart(title, vals, color, size=(600, 360)):
    im = Image.new("RGB", size, "white")
    d = ImageDraw.Draw(im)
    d.text((20, 12), title, fill="black", font=font(26))
    w = (size[0] - 80) // len(vals)
    mx = max(v for _, v in vals)
    for i, (lab, v) in enumerate(vals):
        x = 50 + i * w
        h = int((size[1] - 120) * v / mx)
        d.rectangle([x, size[1] - 50 - h, x + w - 24, size[1] - 50], fill=color)
        d.text((x, size[1] - 42), lab, fill="black", font=font(18))
        d.text((x, size[1] - 76 - h), str(v), fill="black", font=font(18))
    return im


def png_bytes(im):
    b = io.BytesIO()
    im.save(b, "PNG", optimize=True)
    return b.getvalue()


def jpg_bytes(im, q=80):
    b = io.BytesIO()
    im.save(b, "JPEG", quality=q)
    return b.getvalue()


def photo(seed, size=(480, 320)):
    rnd = random.Random(seed)
    im = Image.new("RGB", size)
    d = ImageDraw.Draw(im)
    for y in range(size[1]):
        d.line([(0, y), (size[0], y)], fill=(int(70 + 90 * y / size[1]), int(130 + 60 * y / size[1]), 200))
    for _ in range(14):
        x, y, r = rnd.randint(0, size[0]), rnd.randint(0, size[1]), rnd.randint(15, 60)
        d.ellipse([x - r, y - r, x + r, y + r], fill=(rnd.randint(150, 255), rnd.randint(120, 220), rnd.randint(60, 160)))
    return im


def set_font(doc):
    st = doc.styles["Normal"]
    st.font.name = FONT
    st.font.size = Pt(10.5)
    st.element.rPr.rFonts.set(qn("w:eastAsia"), FONT)
    for name in ["Title", "Heading 1", "Heading 2", "Heading 3", "Caption", "List Bullet", "List Bullet 2", "List Number"]:
        s = doc.styles[name]
        s.font.name = FONT
        rpr = s.element.get_or_add_rPr()
        rpr.get_or_add_rFonts().set(qn("w:eastAsia"), FONT)
        rpr.get_or_add_rFonts().set(qn("w:ascii"), FONT)
        rpr.get_or_add_rFonts().set(qn("w:hAnsi"), FONT)


def set_cols(section, n):
    cols = section._sectPr.find(qn("w:cols"))
    if cols is None:
        cols = OxmlElement("w:cols")
        section._sectPr.append(cols)
    cols.set(qn("w:num"), str(n))
    cols.set(qn("w:space"), "425")


LONG = ("이번 분기에는 신규 고객이 크게 늘었고, 기존 고객의 재구매 비율도 지난 분기보다 높아졌습니다. "
        "특히 온라인 주문이 전체 매출의 절반을 넘으면서 배송 체계를 다시 점검할 필요가 생겼습니다. "
        "고객센터에 들어온 문의는 대부분 배송 일정과 교환 절차에 관한 것이었으며, 응답 시간은 평균 두 시간 안쪽으로 지켜졌습니다. "
        "다음 분기에는 지적된 문제를 바로잡고, 지역별 판매 차이를 줄이는 데 힘을 쏟을 계획입니다.")
COL_TEXT = ("두 단으로 나뉜 본문입니다. 왼쪽 단을 끝까지 읽은 뒤 오른쪽 단으로 넘어가야 뜻이 이어집니다. "
            "변환할 때 줄을 가로로 섞어 읽으면 문장이 엉키므로, 단을 먼저 나누고 단마다 위에서 아래로 읽어야 합니다. ")


def make_report_docx(path, two_col_only=False):
    doc = Document()
    set_font(doc)
    sec = doc.sections[0]
    sec.page_width, sec.page_height = Cm(21), Cm(29.7)
    sec.left_margin = sec.right_margin = Cm(2.2)
    sec.header.paragraphs[0].text = "주식회사 샘플 — 내부 보고서"
    fp = sec.footer.paragraphs[0]
    fp.text = "대외비 · 무단 배포 금지 · "
    fld = OxmlElement("w:fldSimple")
    fld.set(qn("w:instr"), "PAGE")
    r = OxmlElement("w:r")
    t = OxmlElement("w:t")
    t.text = "1"
    r.append(t)
    fld.append(r)
    fp._p.append(fld)
    fp.add_run(" 쪽")
    if two_col_only:
        doc.add_paragraph("두 단 편집 시험 문서", style="Title")
        doc.add_paragraph("제목은 한 단, 본문은 두 단입니다.")
        new = doc.add_section(WD_SECTION.CONTINUOUS)
        set_cols(new, 2)
        doc.add_paragraph("왼쪽 단 시작", style="Heading 1")
        for i in range(1, 7):
            doc.add_paragraph(f"[{i}] " + COL_TEXT * 2)
        doc.add_paragraph("오른쪽 단 끝", style="Heading 1")
        doc.add_paragraph("마지막 문단입니다. 여기까지 읽으면 두 단을 모두 읽은 것입니다.")
        doc.save(path)
        return
    doc.add_paragraph("2026년 3분기 영업 보고서", style="Title")
    doc.add_paragraph("영업기획팀")
    doc.add_paragraph("2026년 10월 15일")
    doc.add_paragraph("요약", style="Heading 1")
    doc.add_paragraph(LONG)
    doc.add_paragraph("매출은 지난 분기보다 12% 늘었습니다.", style="List Bullet")
    doc.add_paragraph("온라인 비중이 54%로 처음 절반을 넘었습니다.", style="List Bullet")
    doc.add_paragraph("수도권 매출 비중이 가장 높습니다.", style="List Bullet 2")
    doc.add_paragraph("반품률은 2.1%로 낮아졌습니다.", style="List Bullet")
    doc.add_paragraph("지역별 실적", style="Heading 1")
    doc.add_paragraph("세부 숫자", style="Heading 2")
    doc.add_paragraph("아래 표는 지역별 매출과 지난 분기 대비 증감입니다.")
    rows = [("지역", "매출(억 원)", "증감"), ("서울", "128", "+9%"), ("경기", "96", "+15%"), ("부산", "41", "+3%"), ("대전", "27", "-2%")]
    t = doc.add_table(rows=len(rows), cols=3)
    t.style = "Table Grid"
    for r, row in enumerate(rows):
        for c, v in enumerate(row):
            t.cell(r, c).text = v
    doc.add_paragraph("그래프", style="Heading 2")
    doc.add_paragraph("월별 매출 추이를 그래프로 나타냈습니다.")
    doc.add_picture(io.BytesIO(png_bytes(chart("월별 매출(억 원)", [("7월", 80), ("8월", 92), ("9월", 120)], (47, 85, 151)))), width=Cm(12))
    doc.add_paragraph("그림 1. 월별 매출 추이", style="Caption")
    doc.add_paragraph("다음 분기 계획", style="Heading 1")
    for i, s in enumerate(["배송 체계를 다시 점검합니다", "지역별 판매 차이를 줄입니다", "교환 절차를 간단하게 바꿉니다",
                           "고객센터 응답 시간을 한 시간 안으로 줄입니다", "신규 상품 세 가지를 내놓습니다", "재고 관리 프로그램을 바꿉니다",
                           "협력사 계약을 다시 맺습니다", "직원 교육을 두 번 엽니다", "분기 말에 결과를 다시 보고합니다"]):
        doc.add_paragraph(s, style="List Number")
    doc.add_paragraph("세부 일정", style="Heading 3")
    doc.add_paragraph("일정은 각 팀과 협의해 다음 달 첫 주에 확정합니다.")
    doc.add_paragraph("현장 사진", style="Heading 1")
    t2 = doc.add_table(rows=1, cols=2)
    t2.style = "Table Grid"
    t2.cell(0, 0).text = "물류 창고 모습"
    run = t2.cell(0, 1).paragraphs[0].add_run()
    run.add_picture(io.BytesIO(jpg_bytes(photo(7))), width=Cm(5))
    doc.add_paragraph("표 안에 든 사진은 따로 꺼내 보여야 합니다.")
    new = doc.add_section(WD_SECTION.CONTINUOUS)
    set_cols(new, 2)
    doc.add_paragraph("부록: 두 단 메모", style="Heading 1")
    for i in range(1, 5):
        doc.add_paragraph(f"메모 {i}. " + COL_TEXT)
    doc.save(path)


def make_noborder_docx(path):
    from docx.enum.text import WD_ALIGN_PARAGRAPH
    from docx.opc.constants import RELATIONSHIP_TYPE as RT
    doc = Document()
    set_font(doc)
    doc.add_paragraph("선 없는 표 시험", style="Heading 1")
    p = doc.add_paragraph("이 문단에는 ")
    p.add_run("굵은 글씨").bold = True
    p.add_run("와 ")
    p.add_run("기울인 글씨").italic = True
    p.add_run(", 그리고 ")
    rid = doc.part.relate_to("https://example.org/report", RT.HYPERLINK, is_external=True)
    h = OxmlElement("w:hyperlink")
    h.set(qn("r:id"), rid)
    r = OxmlElement("w:r")
    t = OxmlElement("w:t")
    t.text = "누리집 링크"
    r.append(t)
    h.append(r)
    p._p.append(h)
    p.add_run("가 들어 있습니다.")
    rows = [("품목", "1월", "2월", "합계"), ("사과", "1,200", "980", "2,180"), ("배", "640", "1,020", "1,660"), ("포도", "75", "90", "165")]
    t = doc.add_table(rows=len(rows), cols=4)  # 기본 표 스타일: 테두리 없음
    for ri, row in enumerate(rows):
        for ci, v in enumerate(row):
            cell = t.cell(ri, ci)
            cell.text = v
            par = cell.paragraphs[0]
            par.alignment = WD_ALIGN_PARAGRAPH.CENTER if ri == 0 else (WD_ALIGN_PARAGRAPH.RIGHT if ci > 0 else WD_ALIGN_PARAGRAPH.LEFT)
            if ri == 0:
                par.runs[0].bold = True
    doc.add_paragraph("표 아래 문단입니다.")
    doc.save(path)


def soffice_convert(src, fmt, outdir):
    with tempfile.TemporaryDirectory() as prof:
        subprocess.run([SOFFICE, f"-env:UserInstallation=file://{prof}", "--headless", "--norestore", "--convert-to", fmt, "--outdir", str(outdir), str(src)],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=300)


def make_scan(pdf, out):
    with tempfile.TemporaryDirectory() as tmp:
        subprocess.run(["pdftoppm", "-r", "100", "-gray", "-png", "-l", "2", str(pdf), f"{tmp}/p"], check=True)
        pages = []
        rnd = random.Random(3)
        for p in sorted(Path(tmp).glob("p*.png")):
            im = Image.open(p).convert("L").rotate(0.7, resample=Image.BICUBIC, expand=False, fillcolor=255)
            px = im.load()
            for _ in range(im.width * im.height // 400):
                px[rnd.randrange(im.width), rnd.randrange(im.height)] = rnd.randint(0, 120)
            pages.append(im.filter(ImageFilter.GaussianBlur(0.4)))
        pages[0].save(out, "PDF", resolution=100.0, save_all=True, append_images=pages[1:], quality=60)


def make_encrypted(src, out):
    from pypdf import PdfReader, PdfWriter
    w = PdfWriter(clone_from=PdfReader(str(src)))
    w.encrypt(user_password="1234", owner_password="owner-1234", algorithm="RC4-128")
    with open(out, "wb") as f:
        w.write(f)


def make_rotated(src, out):
    from pypdf import PdfReader, PdfWriter
    r = PdfReader(str(src))
    w = PdfWriter()
    for i, p in enumerate(r.pages[:2]):
        if i == 1:
            p.rotate(90)
        w.add_page(p)
    with open(out, "wb") as f:
        w.write(f)


EDGE_HTML = """<!doctype html><html lang="ko"><head><meta charset="utf-8"><title>신제품 출시 안내</title>
<style>
@font-face {{ font-family: 'NanumGothicLocal'; src: url('{font}'); }}
body {{ font-family: 'NanumGothicLocal', sans-serif; font-size: 11pt; margin: 2cm; }}
h1 {{ font-size: 22pt; }} h2 {{ font-size: 15pt; margin-top: 18pt; }}
table {{ border-collapse: collapse; width: 100%; }} td, th {{ border: 1px solid #888; padding: 4px 8px; }}
th {{ background: #dce6f2; }}
</style></head><body>
<h1>신제품 출시 안내</h1>
<p>이번 달 말에 새 제품을 내놓습니다. 자세한 내용은 <a href="https://example.com/new">제품 안내 누리집</a>에서 볼 수 있습니다.</p>
<h2>주요 특징</h2>
<ul><li>배터리가 하루 종일 갑니다</li><li>무게가 지난 모델보다 20% 가볍습니다</li><li>색상은 세 가지입니다</li></ul>
<h2>가격표</h2>
<table><tr><th>모델</th><th>저장 용량</th><th>가격</th></tr>
<tr><td>기본형</td><td>128GB</td><td>59만 원</td></tr>
<tr><td>고급형</td><td>256GB</td><td>79만 원</td></tr>
<tr><td>전문가형</td><td>512GB</td><td>99만 원</td></tr></table>
<h2>제품 사진</h2>
<p><img src="edge_photo.jpg" width="320"></p>
<p>문의는 고객센터로 해 주세요. 상담 시간은 평일 오전 9시부터 오후 6시까지입니다. 주말과 공휴일에는 누리집 게시판에 글을 남겨 주시면 다음 근무일에 답을 드립니다.</p>
</body></html>"""


def make_pptx(out, img_a, img_b):
    from pptx import Presentation
    from pptx.util import Inches
    p = Presentation()
    for i, img in enumerate([img_a, img_a, img_b]):
        s = p.slides.add_slide(p.slide_layouts[6])
        s.shapes.add_picture(io.BytesIO(img), Inches(1), Inches(1), width=Inches(4))
    p.save(out)


def make_xlsx(out, img_a, img_b):
    from openpyxl import Workbook
    from openpyxl.drawing.image import Image as XImage
    wb = Workbook()
    ws = wb.active
    ws.title = "매출"
    ws.append(["월", "매출"])
    for m, v in [("7월", 80), ("8월", 92), ("9월", 120)]:
        ws.append([m, v])
    ws.add_image(XImage(io.BytesIO(img_a)), "D2")
    ws2 = wb.create_sheet("사진")
    ws2.add_image(XImage(io.BytesIO(img_b)), "B2")
    wb.save(out)


def main():
    out = HERE
    make_report_docx(out / "보고서.docx")
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        make_report_docx(tmp / "두단.docx", two_col_only=True)
        make_noborder_docx(tmp / "선없는표.docx")
        soffice_convert(tmp / "선없는표.docx", "pdf", tmp)
        shutil.copy(tmp / "선없는표.pdf", out / "선없는표.pdf")
        soffice_convert(out / "보고서.docx", "pdf", tmp)
        soffice_convert(tmp / "두단.docx", "pdf", tmp)
        soffice_convert(out / "보고서.docx", "odt", tmp)
        shutil.copy(tmp / "보고서.pdf", out / "보고서_LO.pdf")
        shutil.copy(tmp / "두단.pdf", out / "두단.pdf")
        shutil.copy(tmp / "보고서.odt", out / "보고서.odt")
    make_scan(out / "보고서_LO.pdf", out / "스캔본.pdf")
    make_encrypted(out / "보고서_LO.pdf", out / "암호.pdf")
    make_rotated(out / "보고서_LO.pdf", out / "회전쪽.pdf")
    (out / "엣지인쇄.html").write_text(EDGE_HTML.format(font="NanumGothic.ttf"), encoding="utf-8")
    (out / "edge_photo.jpg").write_bytes(jpg_bytes(photo(11)))
    a = png_bytes(chart("분기 매출", [("1분기", 70), ("2분기", 85), ("3분기", 100)], (237, 125, 49)))
    b = jpg_bytes(photo(5))
    make_pptx(out / "이미지모음.pptx", a, b)
    make_xlsx(out / "그림표.xlsx", a, b)
    for p in sorted(out.iterdir()):
        if p.suffix != ".py":
            print(f"{p.name}\t{p.stat().st_size}")


if __name__ == "__main__":
    main()
