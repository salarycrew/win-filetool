//! NFC 폴더 이름 바꾸기와 맥 zip 처리 — 실제 파일로 검증한다.

use std::fs;
use std::io::Read;
use std::path::Path;

use filetool_core::{nfc, samples, zipnfc};
use unicode_normalization::UnicodeNormalization;

fn nfd(s: &str) -> String {
    s.nfd().collect()
}

#[test]
fn folder_nfd_to_nfc_with_collision() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // NFD 폴더 안의 NFD 파일, NFD 파일, 그리고 NFC 이름이 이미 있는 충돌 케이스
    let dir_nfd = root.join(nfd("사진"));
    fs::create_dir(&dir_nfd).unwrap();
    fs::write(dir_nfd.join(nfd("여행.jpg")), b"jpg").unwrap();
    fs::write(root.join(nfd("보고서.txt")), b"report").unwrap();
    fs::write(root.join("메모.txt"), b"nfc original").unwrap(); // 이미 NFC
    fs::write(root.join(nfd("메모.txt")), b"nfd copy").unwrap(); // 충돌
    fs::write(root.join("plain.txt"), b"ascii").unwrap();

    assert_ne!(nfd("사진"), "사진", "테스트 전제: NFD와 NFC는 다른 바이트");

    // 1) 미리보기: 디스크는 그대로
    let plan = nfc::plan_folder(root).unwrap();
    let mut froms: Vec<String> = plan
        .renames
        .iter()
        .map(|r| r.to.strip_prefix(root).unwrap().to_string_lossy().nfc().collect())
        .collect();
    froms.sort();
    assert_eq!(froms, vec!["보고서.txt", "사진", "사진/여행.jpg"].iter().map(|s| s.replace('/', std::path::MAIN_SEPARATOR_STR)).collect::<Vec<_>>());
    assert_eq!(plan.collisions.len(), 1);
    assert_eq!(plan.collisions[0].existing, root.join("메모.txt"));
    assert!(root.join(nfd("보고서.txt")).exists(), "미리보기는 디스크를 바꾸지 않아야 함");
    // 자식이 부모보다 먼저 와야 적용 중 경로가 유효하다
    let child = plan.renames.iter().position(|r| r.from.ends_with(nfd("여행.jpg"))).unwrap();
    let parent = plan.renames.iter().position(|r| r.from.ends_with(nfd("사진"))).unwrap();
    assert!(child < parent);

    // 2) 적용
    let report = nfc::apply_plan(&plan);
    assert_eq!(report.renamed.len(), 3, "{report:?}");
    assert!(report.failed.is_empty());
    assert_eq!(report.collisions.len(), 1);

    let names: Vec<String> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(names.contains(&"사진".to_string()));
    assert!(names.contains(&"보고서.txt".to_string()));
    assert!(!names.contains(&nfd("보고서.txt")));
    // 충돌한 NFD 파일은 그대로 남고, 기존 NFC 파일 내용은 덮어쓰이지 않는다
    assert!(names.contains(&nfd("메모.txt")));
    assert_eq!(fs::read(root.join("메모.txt")).unwrap(), b"nfc original");
    assert_eq!(fs::read(root.join("사진").join("여행.jpg")).unwrap(), b"jpg");

    // 3) 다시 돌리면 충돌 1건 외에는 할 일이 없다
    let again = nfc::plan_folder(root).unwrap();
    assert!(again.renames.is_empty());
    assert_eq!(again.collisions.len(), 1);
}

fn mac_zip_fixture(dir: &Path) -> std::path::PathBuf {
    let zip_path = dir.join("mac.zip");
    let folder = nfd("자료/");
    let doc = nfd("자료/한글 문서.txt");
    let img = nfd("자료/사진.jpg");
    let junk1 = nfd("__MACOSX/자료/._한글 문서.txt");
    let junk2 = nfd("자료/._사진.jpg");
    let junk3 = nfd("자료/.DS_Store");
    samples::write_mac_style_zip(
        &zip_path,
        &[
            (folder.as_str(), b""),
            (doc.as_str(), "본문".as_bytes()),
            (img.as_str(), b"\xff\xd8jpeg"),
            (junk1.as_str(), b"appledouble"),
            (junk2.as_str(), b"appledouble"),
            (junk3.as_str(), b"ds"),
        ],
    )
    .unwrap();
    zip_path
}

#[test]
fn mac_zip_extract_nfc() {
    let tmp = tempfile::tempdir().unwrap();
    let zip_path = mac_zip_fixture(tmp.path());

    // 전제 확인: 플래그 없는 NFD 이름이라 일반 해석(CP437)으로는 깨진다
    let mut ar = zip::ZipArchive::new(fs::File::open(&zip_path).unwrap()).unwrap();
    let raw = ar.by_index(1).unwrap().name_raw().to_vec();
    assert_eq!(raw, nfd("자료/한글 문서.txt").into_bytes());

    let out = zipnfc::default_extract_dir(&zip_path);
    assert_eq!(out, tmp.path().join("mac"));
    let report = zipnfc::extract_nfc(&zip_path, &out).unwrap();
    assert_eq!(report.files.len(), 2, "{report:?}");
    assert_eq!(report.skipped_mac.len(), 3);
    assert_eq!(report.normalized, 3);
    assert!(report.collisions.is_empty());

    let base = out.join("자료");
    let mut names: Vec<String> =
        fs::read_dir(&base).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    assert_eq!(names, vec!["사진.jpg", "한글 문서.txt"]);
    assert_eq!(fs::read_to_string(base.join("한글 문서.txt")).unwrap(), "본문");
    assert!(!out.join("__MACOSX").exists());

    // 같은 폴더에 다시 풀면 덮어쓰지 않고 충돌로 보고
    let again = zipnfc::extract_nfc(&zip_path, &out).unwrap();
    assert_eq!(again.files.len(), 0);
    assert_eq!(again.collisions.len(), 2);
    // 기본 폴더 이름은 겹치지 않게 새로 고른다
    assert_eq!(zipnfc::default_extract_dir(&zip_path), tmp.path().join("mac (1)"));
}

#[test]
fn mac_zip_rezip_nfc() {
    let tmp = tempfile::tempdir().unwrap();
    let zip_path = mac_zip_fixture(tmp.path());
    let out = zipnfc::default_rezip_path(&zip_path, None);
    assert_eq!(out, tmp.path().join("mac_NFC.zip"));
    let report = zipnfc::rezip_nfc(&zip_path, &out).unwrap();
    assert_eq!(report.entries, 3, "{report:?}");
    assert_eq!(report.skipped_mac.len(), 3);

    let mut ar = zip::ZipArchive::new(fs::File::open(&out).unwrap()).unwrap();
    let mut names = Vec::new();
    for i in 0..ar.len() {
        let mut f = ar.by_index(i).unwrap();
        let name = f.name().unwrap().to_string();
        // 다시 만든 zip은 NFC 바이트이며, 표준 해석(name())으로도 바로 읽힌다
        assert_eq!(f.name_raw(), name.as_bytes());
        if name.ends_with("한글 문서.txt") {
            let mut s = String::new();
            f.read_to_string(&mut s).unwrap();
            assert_eq!(s, "본문");
        }
        names.push(name);
    }
    names.sort();
    assert_eq!(names, vec!["자료/", "자료/사진.jpg", "자료/한글 문서.txt"]);
    // 출력이 이미 있으면 덮어쓰지 않는다
    assert!(zipnfc::rezip_nfc(&zip_path, &out).is_err());
}

#[test]
fn cp949_entry_name_is_decoded() {
    let (bytes, _, _) = encoding_rs::EUC_KR.encode("한글.txt");
    let e = zipnfc::decode_entry_name(&bytes);
    assert_eq!(e.name, "한글.txt");
    assert_eq!(e.encoding, zipnfc::NameEncoding::Cp949);
    assert!(zipnfc::is_mac_junk("__MACOSX/a/._b"));
    assert!(zipnfc::is_mac_junk("a/._b"));
    assert!(!zipnfc::is_mac_junk("a/_b"));
}
