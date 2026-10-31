//! PDF 페이지 작업 — 테스트 안에서 PDF(2단 페이지 트리 + 상속 MediaBox)를 만들어 실제 결과(쪽수·회전값·순서)를 확인한다.

use std::path::{Path, PathBuf};

use filetool_core::pdf::{self, SplitMode};
use filetool_core::samples;
use lopdf::Document;

fn labels(path: &Path) -> Vec<String> {
    samples::pdf_labels(path).unwrap()
}

fn rotations(path: &Path) -> Vec<i64> {
    samples::pdf_rotations(path).unwrap()
}

fn mediabox_ok(path: &Path) -> bool {
    let doc = Document::load(path).unwrap();
    doc.get_pages().values().all(|&pid| doc.get_dictionary(pid).unwrap().has(b"MediaBox"))
}

fn fixture(dir: &Path, name: &str, n: u32, label: &str) -> PathBuf {
    let p = dir.join(name);
    samples::make_pdf(&p, n, label).unwrap();
    p
}

#[test]
fn merge_in_order() {
    let t = tempfile::tempdir().unwrap();
    let a = fixture(t.path(), "a.pdf", 3, "A");
    let b = fixture(t.path(), "b.pdf", 2, "B");
    let c = fixture(t.path(), "c.pdf", 1, "C");
    let out = t.path().join("merged.pdf");
    let n = pdf::merge(&[b.clone(), a.clone(), c.clone()], &out).unwrap();
    assert_eq!(n, 6);
    assert_eq!(pdf::page_count(&out).unwrap(), 6);
    assert_eq!(labels(&out), ["B p1", "B p2", "A p1", "A p2", "A p3", "C p1"]);
    assert!(mediabox_ok(&out), "상속 MediaBox가 페이지로 내려와야 함");
    // 출력이 이미 있으면 덮어쓰지 않는다
    assert!(pdf::merge(&[a, b], &out).is_err());
}

#[test]
fn split_every_page_and_ranges() {
    let t = tempfile::tempdir().unwrap();
    let src = fixture(t.path(), "doc.pdf", 6, "D");

    let each = pdf::split(&src, &SplitMode::EveryPage, None).unwrap();
    assert_eq!(each.len(), 6);
    assert_eq!(each[0].file_name().unwrap(), "doc_p1.pdf");
    for (i, p) in each.iter().enumerate() {
        assert_eq!(pdf::page_count(p).unwrap(), 1);
        assert_eq!(labels(p), [format!("D p{}", i + 1)]);
    }

    let out_dir = t.path().join("out");
    let parts = pdf::split(&src, &SplitMode::Ranges("1-3,5".into()), Some(&out_dir)).unwrap();
    let names: Vec<_> = parts.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["doc_p1-3.pdf", "doc_p5.pdf"]);
    assert_eq!(labels(&parts[0]), ["D p1", "D p2", "D p3"]);
    assert_eq!(labels(&parts[1]), ["D p5"]);
    assert!(mediabox_ok(&parts[0]));

    // 같은 나누기를 다시 하면 기존 파일을 두고 ` (1)` 이름으로 만든다
    let again = pdf::split(&src, &SplitMode::Ranges("5".into()), Some(&out_dir)).unwrap();
    assert_eq!(again[0].file_name().unwrap(), "doc_p5 (1).pdf");

    assert!(pdf::split(&src, &SplitMode::Ranges("7".into()), None).is_err());
}

#[test]
fn rotate_pages() {
    let t = tempfile::tempdir().unwrap();
    let src = fixture(t.path(), "r.pdf", 3, "R");
    let out = t.path().join("r_rot.pdf");
    pdf::rotate(&src, "2", 90, &out).unwrap();
    assert_eq!(rotations(&out), [0, 90, 0]);

    let out2 = t.path().join("r_rot2.pdf");
    pdf::rotate(&out, "all", 270, &out2).unwrap();
    assert_eq!(rotations(&out2), [270, 0, 270]);

    let out3 = t.path().join("r_rot3.pdf");
    pdf::rotate(&src, "1,3", 180, &out3).unwrap();
    assert_eq!(rotations(&out3), [180, 0, 180]);
    assert_eq!(labels(&out3), ["R p1", "R p2", "R p3"]);

    assert!(pdf::rotate(&src, "1", 45, &t.path().join("bad.pdf")).is_err());
    // 원본은 그대로
    assert_eq!(rotations(&src), [0, 0, 0]);
}

#[test]
fn delete_pages() {
    let t = tempfile::tempdir().unwrap();
    let src = fixture(t.path(), "d.pdf", 5, "X");
    let out = t.path().join("d_del.pdf");
    let left = pdf::delete_pages(&src, "2,4-5", &out).unwrap();
    assert_eq!(left, 2);
    assert_eq!(pdf::page_count(&out).unwrap(), 2);
    assert_eq!(labels(&out), ["X p1", "X p3"]);
    assert!(pdf::delete_pages(&src, "1-5", &t.path().join("none.pdf")).is_err());
    assert_eq!(pdf::page_count(&src).unwrap(), 5);
}

#[test]
fn reorder_pages() {
    let t = tempfile::tempdir().unwrap();
    let src = fixture(t.path(), "o.pdf", 4, "O");
    let out = t.path().join("o_re.pdf");
    pdf::reorder(&src, "3,1,4,2", &out).unwrap();
    assert_eq!(labels(&out), ["O p3", "O p1", "O p4", "O p2"]);

    let rev = t.path().join("o_rev.pdf");
    pdf::reorder(&src, "4-1", &rev).unwrap();
    assert_eq!(labels(&rev), ["O p4", "O p3", "O p2", "O p1"]);

    let err = pdf::reorder(&src, "1,2", &t.path().join("bad.pdf")).unwrap_err();
    assert!(err.to_string().contains("빠진 페이지"), "{err}");
    assert!(pdf::reorder(&src, "1,1,2,3,4", &t.path().join("bad2.pdf")).is_err());
}
